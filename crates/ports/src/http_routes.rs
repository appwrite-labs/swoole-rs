//! `swoole::http_server`'s request and frame routing from `src/protocol/http.cc`:
//! the callbacks `init_protocol` puts on an HTTP port. A connection past the WebSocket handshake goes to WebSocket, an HTTP/2 one to
//! HTTP/2, and any other is a protocol error. WebSocket's own length and frame
//! dispatch from `src/protocol/websocket.cc` live here too.

use std::ffi::c_int;
use swoole_rs_http2::FRAME_HEADER_SIZE as HTTP2_FRAME_HEADER_SIZE;

use swoole_rs_clients::protocol::Protocol;
use swoole_rs_corebase::CCast;
use swoole_rs_corebase::log;
use swoole_rs_corebase::log::LogLevel::SW_LOG_WARNING as LOG_WARNING;
use swoole_rs_corebase::net_types::{PacketLength, SocketAddress};
use swoole_rs_error::ErrorCode;
use swoole_rs_server::Connection;

/// `SW_WEBSOCKET_FRAME_HEADER_SIZE`: header, mask and a 64-bit length.
const WEBSOCKET_FRAME_HEADER_SIZE: u8 = 2 + 4 + 8;

/// `conn`'s branch: `websocket` past the handshake, else `http2` for an HTTP/2
/// stream, else upstream's `protocol_status_error` warning and `error`.
fn route<T>(
    conn: Connection<'_>,
    websocket: impl FnOnce() -> T,
    http2: impl FnOnce() -> T,
    error: T,
) -> T {
    match length_size(conn, exports::get_package_length_size) {
        WEBSOCKET_FRAME_HEADER_SIZE => websocket(),
        HTTP2_FRAME_HEADER_SIZE => http2(),
        _ => error,
    }
}

/// Report a session whose framing route has not been established.
fn warn_protocol_status(conn: Connection<'_>) {
    let (host, port) = host_port(&conn.info());
    log::error_log(
        LOG_WARNING,
        ErrorCode::SW_ERROR_PROTOCOL_ERROR as c_int,
        c"protocol_status_error",
        format_args!(
            "unexpected protocol status of session#{}<{host}:{port}>",
            conn.session_id()
        ),
    );
}

/// The host and port of an address that holds its socket type's member, as
/// upstream's warnings print them.
pub(crate) fn host_port(address: &SocketAddress) -> (String, c_int) {
    use swoole_rs_clients::address::{address_port, with_host};
    let host = with_host(address, |host| host.to_string_lossy().into_owned());
    (host, address_port(address).into())
}

/// `websocket::get_package_length` as a protocol's length callback: it reads only `pl`.
pub fn websocket_length(
    _proto: &Protocol,
    _: Option<u8>,
    _owner: Option<std::rc::Rc<dyn core::any::Any>>,
    pl: &mut PacketLength,
) -> Option<usize> {
    let (length, header) = match swoole_rs_websocket::frame_length(pl.bytes()) {
        // A total past `ssize_t` is upstream's negative length.
        Ok(frame) => (
            Some(frame.total).filter(|&total| isize::try_from(total).is_ok()),
            Some(frame.header_length),
        ),
        Err(error) => (
            match error {
                swoole_rs_websocket::LengthError::IncompleteHeader { .. } => Some(0),
                swoole_rs_websocket::LengthError::Overflow { .. } => None,
            },
            error.header_length(),
        ),
    };
    if let Some(header) = header {
        pl.header_len = header.c_cast::<u32>();
    }
    length
}

/// `get_package_length_size`'s result for `conn`.
pub fn length_size(
    conn: Connection<'_>,
    size: swoole_rs_clients::protocol::GetPackageLengthSize,
) -> u8 {
    let header = size(conn.websocket_status(), conn.http2_stream());
    if header == 0 {
        warn_protocol_status(conn);
    }
    header
}

/// A server session as a port's package handlers see it: the port's server, the
/// session's connection and its socket, live for the read that lends them. The
/// server's reader makes one from the connection its read looked up; an `onPackage`
/// callback, from the protocol and socket it is called with.
#[derive(Clone, Copy)]
pub(crate) struct Session<'a> {
    pub(crate) port: Option<&'a swoole_rs_server::port_config::PortConfig>,
    pub(crate) server: swoole_rs_server::view::Server<'a>,
    pub(crate) conn: Connection<'a>,
    pub(crate) socket: swoole_rs_corebase::reactor::Socket<'a>,
}

impl Session<'_> {
    /// `Server::dispatch_task` of `data` to the workers; whether it went.
    pub(crate) fn task(self, data: swoole_rs_server::SendData<'_>) -> bool {
        swoole_rs_server::reactor_entry::dispatch_data(self.server, self.conn, data, self.port)
    }
}

pub mod exports {
    use std::mem::MaybeUninit;
    use swoole_rs_corebase::CCast;

    use crate::frame_api::decode_frame;
    use swoole_rs_clients::protocol::Protocol;
    use swoole_rs_corebase::DataHead;
    use swoole_rs_corebase::buffer::string::boxed_string;
    use swoole_rs_corebase::lent::lend_spare;
    use swoole_rs_corebase::net_types::PacketLength;
    use swoole_rs_corebase::reactor::Socket;
    use swoole_rs_corebase::{WebSocketStatus, log};
    use swoole_rs_server::{Connection, view};
    use swoole_rs_websocket::{
        Action, Context, FrameFlags, Opcode, PolicyError, decide_frame, encode_frame,
    };

    use super::{Session, WEBSOCKET_FRAME_HEADER_SIZE, length_size, route};
    use crate::framing_recv::{Packages, Recv, paused, send_data};
    use swoole_rs_clients::protocol::Package;

    /// `swoole::http2::get_frame_length(const Protocol *, Socket *, PacketLength *)`:
    /// the length callback of an HTTP/2 port, and of crates/phphttp2's client and
    /// crates/phphttp's coroutine server.
    pub fn http2_length(
        _proto: &Protocol,
        _: Option<u8>,
        _owner: Option<std::rc::Rc<dyn core::any::Any>>,
        pl: &mut PacketLength,
    ) -> Option<usize> {
        Some(swoole_rs_http2::frame_length(pl.bytes()).unwrap_or(0))
    }

    /// `SW_WEBSOCKET_HEADER_LEN + SW_WEBSOCKET_CLOSE_CODE_LEN + SW_WEBSOCKET_CLOSE_REASON_MAX_LEN`.
    const CLOSE_REPLY_MAX: usize = 2 + 2 + 125;

    /// `websocket::dispatch_frame`'s policy warnings, with the peer's address.
    fn policy_warning(conn: Connection<'_>, error: PolicyError, opcode: u8) {
        let what = match error {
            PolicyError::Orphan => "bad frame[opcode=0]".to_owned(),
            PolicyError::TooBig => "websocket frame is too big".to_owned(),
            PolicyError::Overlap => "merging incomplete frame, bad request".to_owned(),
            PolicyError::ControlTooBig => format!(
                "{} frame application data is too big",
                if opcode == swoole_rs_websocket::Opcode::Ping.code() {
                    "ping"
                } else {
                    "pong"
                }
            ),
            PolicyError::None | PolicyError::CloseTooBig => return,
        };
        let (host, port) = super::host_port(&conn.info());
        log::warning(
            c"websocket::dispatch_frame",
            format_args!("{what}. remote_addr={host}:{port}"),
        );
    }

    /// `websocket::dispatch_frame` over a session's whole frame: decode it (unmasking
    /// it in place), let [`decide_frame`] choose, and carry out its action on the
    /// connection's fragment buffer, the server and the socket. Whether the connection
    /// reads on.
    pub(crate) fn websocket_frame(s: Session<'_>, mut package: Package<'_>) -> bool {
        let length = package.len();
        let bytes = package.bytes();
        let Ok(frame) = decode_frame(bytes) else {
            return false;
        };
        let bytes: &[u8] = bytes;
        let conn = s.conn;
        // Out of the connection while this frame works on it.
        let fragment = conn.take_websocket_buffer();
        let mut context = Context {
            has_fragment: fragment.is_some(),
            closing: conn.websocket_status() == WebSocketStatus::Closing,
            ..Context::default()
        };
        if let Some(fragment) = fragment.as_deref() {
            context.fragment_length = fragment.len();
            let flags = fragment.offset().c_cast::<u16>();
            let [opcode, flags] = flags.to_be_bytes();
            context.fragment = swoole_rs_websocket::FrameMetadata {
                opcode: swoole_rs_websocket::Opcode::from_code(opcode),
                flags: swoole_rs_websocket::FrameFlags::from_bits(flags),
            };
            if frame.opcode == swoole_rs_websocket::Opcode::Continuation {
                let max = swoole_rs_server::server_methods::package_max_length(s.server, conn.fd());
                context.package_max_length = max as usize;
            }
        }
        let Some(decision) = decide_frame(&frame, length, &context) else {
            if let Some(fragment) = fragment {
                conn.set_websocket_buffer(fragment);
            }
            return false;
        };
        if decision.error != PolicyError::None {
            policy_warning(conn, decision.error, frame.opcode.code());
        }
        let ext_flags = u16::from_be_bytes([
            decision.metadata.opcode.code(),
            decision.metadata.flags.bits(),
        ]);
        let head = DataHead {
            ext_flags,
            ..DataHead::default()
        };
        let payload = bytes
            .get(decision.offset..)
            .and_then(|rest| rest.get(..decision.length));
        if let Action::Append | Action::Finish = decision.action {
            // The decision appends only to a live fragment.
            let Some(mut buffer) = fragment else {
                return false;
            };
            buffer.append(payload.unwrap_or_default());
            if decision.action == Action::Finish {
                s.task(swoole_rs_server::SendData::new(head, buffer.view()));
                buffer.dispose();
            } else {
                conn.set_websocket_buffer(buffer);
            }
            return true;
        }
        if let Some(fragment) = fragment {
            conn.set_websocket_buffer(fragment);
        }
        let task = |payload: Option<&[u8]>| {
            s.task(match payload {
                Some(payload) => swoole_rs_server::SendData::new(head, payload),
                None => swoole_rs_server::SendData::empty(head),
            })
        };
        match decision.action {
            Action::Reject => return false,
            Action::Start => {
                let mut buffer = boxed_string(payload.unwrap_or_default());
                buffer.set_offset(ext_flags.into());
                conn.set_websocket_buffer(buffer);
            }
            Action::Append | Action::Finish => {}
            Action::Control | Action::Dispatch => {
                let empty = decision.action == Action::Control && decision.length == 0;
                task(if empty { None } else { payload });
            }
            Action::ClientClose => {
                task(payload);
                let mut reply = [MaybeUninit::<u8>::uninit(); CLOSE_REPLY_MAX];
                if let Some(payload) = payload {
                    lend_spare(&mut reply, |out| {
                        let close = FrameFlags::default().with_final();
                        if encode_frame(payload.len(), payload, Opcode::Close, close, out).is_ok() {
                            _ = swoole_rs_corebase::tls::upstream::send(s.socket, out.written(), 0);
                        }
                    });
                }
                return false;
            }
            Action::ServerClose => {
                conn.set_websocket_status(WebSocketStatus::None);
                return false;
            }
            Action::Unknown => {
                log::warning(
                    c"websocket::dispatch_frame",
                    format_args!("unknown opcode [{}]", frame.opcode.code()),
                );
            }
        }
        true
    }

    /// What a server port protocol's `onPackage` does with a session's package.
    pub(crate) trait Handler {
        /// Handles `package`, which came with the head `info`; whether the connection
        /// reads on.
        fn handle(s: Session<'_>, package: Package<'_>, info: DataHead) -> bool;
    }

    /// `static int Server::dispatch_task(const Protocol *, Socket *, const RecvData *)`:
    /// a data frame to the workers.
    pub(crate) struct Task;

    impl Handler for Task {
        fn handle(s: Session<'_>, mut package: Package<'_>, info: DataHead) -> bool {
            s.task(send_data(&mut package, info))
        }
    }

    /// `int websocket::dispatch_frame(const Protocol *, Socket *, const RecvData *)`:
    /// [`websocket_frame`].
    pub(crate) struct WebSocketFrame;

    impl Handler for WebSocketFrame {
        fn handle(s: Session<'_>, package: Package<'_>, _: DataHead) -> bool {
            websocket_frame(s, package)
        }
    }

    /// `int dispatch_frame(const Protocol *, Socket *, const RecvData *)`: a frame to
    /// WebSocket past the handshake, else to the workers for an HTTP/2 stream.
    pub(crate) struct HttpFrame;

    impl Handler for HttpFrame {
        fn handle(s: Session<'_>, package: Package<'_>, info: DataHead) -> bool {
            match route(s.conn, || Some(true), || Some(false), None) {
                Some(true) => WebSocketFrame::handle(s, package, info),
                Some(false) => Task::handle(s, package, info),
                None => false,
            }
        }
    }

    /// A server port protocol's `onPackage`: `H` over the session of `socket` in the
    /// server this thread's reactor serves, -1 for none. [`OnPackage`]'s contract has
    /// the caller pass a whole package of a socket the protocol reads: a session's of
    /// that server, as only a server's ports hold these, and the package `H` may
    /// change in place. The server's own reader hands its packages to `H` directly
    /// ([`Port`]).
    ///
    /// [`OnPackage`]: swoole_rs_clients::protocol::OnPackage
    #[must_use]
    pub(crate) fn on_package<H: Handler>(
        _: *const Protocol,
        socket: Socket<'_>,
        package: Package<'_>,
    ) -> bool {
        let handled = view::Server::with_reactor(|server| {
            let Some(conn) = server.at(socket.fd()) else {
                return false;
            };
            let info = DataHead {
                len: u32::try_from(package.len()).unwrap_or(u32::MAX),
                ..DataHead::default()
            };
            let s = Session {
                port: None,
                server,
                conn,
                socket,
            };
            H::handle(s, package, info)
        });
        handled == Some(true)
    }

    /// A session's packages, all [`Session::task`]'s: an EOF or length port's, whose
    /// protocol's `onPackage` `init_protocol` set to [`Task`]'s with its reader.
    pub(crate) struct Tasks<'a>(pub(crate) Session<'a>);

    impl Packages for Tasks<'_> {
        const HEAD: Option<u32> = None;
        #[inline(always)]
        fn dispatch(&self, _: &Recv<'_>, mut package: Package<'_>) -> bool {
            self.0.task(send_data(&mut package, DataHead::default()))
        }
        fn paused(&self) -> bool {
            paused(self.0.conn)
        }
    }

    /// The handler an HTTP port protocol's `onPackage` is (`init_protocol`): `Frame`
    /// ([`HttpFrame`]) with HTTP/2 and WebSocket, `Task` with HTTP/2 alone, `WebSocket`
    /// ([`WebSocketFrame`]) with WebSocket alone, and none without either.
    #[derive(Clone, Copy, PartialEq, Eq)]
    pub(crate) enum Route {
        Frame,
        Task,
        WebSocket,
        None,
    }

    impl Route {
        /// The route of a port with HTTP/2 (`http2`) and WebSocket (`websocket`).
        pub(crate) const fn of(http2: bool, websocket: bool) -> Self {
            match (http2, websocket) {
                (true, true) => Self::Frame,
                (true, false) => Self::Task,
                (false, true) => Self::WebSocket,
                (false, false) => Self::None,
            }
        }
    }

    /// A session's packages, measured by its port protocol's callbacks and handed to
    /// the handler its `onPackage` is: an HTTP/2 stream's.
    pub(crate) struct Port<'a>(pub(crate) Session<'a>, pub(crate) Route);

    impl Packages for Port<'_> {
        const HEAD: Option<u32> = None;
        fn dynamic_header(&self, protocol: &Protocol) -> Option<u8> {
            protocol
                .get_package_length_size
                .map(|size| length_size(self.0.conn, size))
        }
        #[inline(always)]
        fn dispatch(&self, _: &Recv<'_>, package: Package<'_>) -> bool {
            let info = DataHead {
                len: u32::try_from(package.len()).unwrap_or(u32::MAX),
                ..DataHead::default()
            };
            match self.1 {
                Route::Frame => HttpFrame::handle(self.0, package, info),
                Route::Task => Task::handle(self.0, package, info),
                Route::WebSocket => WebSocketFrame::handle(self.0, package, info),
                Route::None => false,
            }
        }
        fn paused(&self) -> bool {
            paused(self.0.conn)
        }
    }

    /// A WebSocket connection's frames, measured by `frame_length` and handed to
    /// [`websocket_frame`] directly: an HTTP port's own callbacks route each frame
    /// through the connection's status to the same two.
    pub(crate) struct Frames<'a>(pub(crate) Session<'a>);

    impl Packages for Frames<'_> {
        const HEAD: Option<u32> = Some(WEBSOCKET_FRAME_HEADER_SIZE as u32);
        #[inline(always)]
        fn length(&self, _: &Recv<'_>, bytes: &[u8]) -> Option<usize> {
            match swoole_rs_websocket::frame_length(bytes) {
                // A frame longer than `ssize_t` holds is malformed.
                Ok(frame) => isize::try_from(frame.total).ok().map(isize::cast_unsigned),
                Err(swoole_rs_websocket::LengthError::IncompleteHeader { .. }) => Some(0),
                Err(swoole_rs_websocket::LengthError::Overflow { .. }) => None,
            }
        }
        #[inline(always)]
        fn dispatch(&self, _: &Recv<'_>, package: Package<'_>) -> bool {
            websocket_frame(self.0, package)
        }
        fn paused(&self) -> bool {
            paused(self.0.conn)
        }
    }

    /// `ssize_t get_package_length(const Protocol *, Socket *, PacketLength *)` over a
    /// connection socket.
    pub fn get_package_length(
        proto: &Protocol,
        header: Option<u8>,
        owner: Option<std::rc::Rc<dyn core::any::Any>>,
        pl: &mut PacketLength,
    ) -> Option<usize> {
        // The header size names the connection's route: WebSocket's, HTTP/2's, or
        // none (warned).
        match header {
            Some(WEBSOCKET_FRAME_HEADER_SIZE) => super::websocket_length(proto, header, owner, pl),
            Some(super::HTTP2_FRAME_HEADER_SIZE) => http2_length(proto, header, owner, pl),
            _ => None,
        }
    }

    /// The frame header for the live session's current protocol.
    // The native protocol table and its callers compare this callback by address.
    #[inline(never)]
    #[must_use]
    pub fn get_package_length_size(status: WebSocketStatus, http2: bool) -> u8 {
        if status >= WebSocketStatus::Handshake {
            WEBSOCKET_FRAME_HEADER_SIZE
        } else if http2 {
            super::HTTP2_FRAME_HEADER_SIZE
        } else {
            0
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use swoole_rs_corebase::WebSocketStatus;
    use swoole_rs_server::ConnectionRecord;

    /// Pinned `http.cc`'s branch for a connection: 1 WebSocket, 2 HTTP/2, 0 error.
    fn pinned(websocket_status: WebSocketStatus, http2_stream: bool) -> u32 {
        // `conn->websocket_status >= websocket::STATUS_HANDSHAKE`, then `conn->http2_stream`.
        if websocket_status.code() >= 2 {
            1
        } else if http2_stream {
            2
        } else {
            0
        }
    }

    /// The retired dispatch gate's route cases: every WebSocket status against
    /// HTTP/2 off and on, through each of the three routes. (The gate also tried 255,
    /// which the field, now a `bool`, can't hold.)
    #[test]
    fn routes_match_the_pinned_branches() {
        let mut cases = 0;
        for websocket_status in WebSocketStatus::ALL {
            for http2_stream in [false, true] {
                let mut conn = ConnectionRecord::default();
                conn.websocket_status.set(websocket_status);
                conn.http2_stream.set(http2_stream);
                conn.info.get_mut().kind = 1;
                let expected = pinned(websocket_status, http2_stream);
                let mut calls = (0, 0);
                let conn = Connection::from(&mut conn);
                let length = route(
                    conn,
                    || {
                        calls.0 += 1;
                        -11
                    },
                    || {
                        calls.1 += 1;
                        44
                    },
                    -1,
                );
                assert_eq!(
                    (length, calls),
                    match expected {
                        1 => (-11, (1, 0)),
                        2 => (44, (0, 1)),
                        _ => (-1, (0, 0)),
                    }
                );
                assert_eq!(
                    length_size(conn, exports::get_package_length_size),
                    [0, WEBSOCKET_FRAME_HEADER_SIZE, HTTP2_FRAME_HEADER_SIZE][expected as usize]
                );
                let frame = route(conn, || -22, || 55, -1);
                assert_eq!(frame, [-1, -22, 55][expected as usize]);
                cases += 3;
            }
        }
        assert_eq!(cases, WebSocketStatus::ALL.len() * 2 * 3);
        assert_eq!(WEBSOCKET_FRAME_HEADER_SIZE, 14);
    }
}
