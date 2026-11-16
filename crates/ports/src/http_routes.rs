//! `swoole::http_server`'s request and frame routing from `src/protocol/http.cc`:
//! the callbacks `init_protocol` puts on an HTTP port. A connection past the WebSocket handshake goes to WebSocket, an HTTP/2 one to
//! HTTP/2, and any other is a protocol error. WebSocket's own length and frame
//! dispatch from `src/protocol/websocket.cc` live here too.

use std::ffi::{c_int, c_void};

use swoole_rs_clients::protocol::Protocol;
use swoole_rs_corebase::log::LogLevel::SW_LOG_WARNING as LOG_WARNING;
use swoole_rs_corebase::net_types::{PacketLength, SocketAddress};
use swoole_rs_corebase::{WebSocketStatus, log};
use swoole_rs_error::ErrorCode;
use swoole_rs_server::Connection;

/// `SW_WEBSOCKET_FRAME_HEADER_SIZE`: header, mask and a 64-bit length.
const WEBSOCKET_FRAME_HEADER_SIZE: u8 = 2 + 4 + 8;
/// `SW_HTTP2_FRAME_HEADER_SIZE`.
const HTTP2_FRAME_HEADER_SIZE: u8 = 9;

/// `conn`'s branch: `websocket` past the handshake, else `http2` for an HTTP/2
/// stream, else upstream's `protocol_status_error` warning and `error`.
fn route<T>(
    conn: Connection<'_>,
    websocket: impl FnOnce() -> T,
    http2: impl FnOnce() -> T,
    error: T,
) -> T {
    if conn.websocket_status() >= WebSocketStatus::Handshake {
        return websocket();
    }
    if conn.http2_stream() {
        return http2();
    }
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
    error
}

/// The host and port of an address that holds its socket type's member, as
/// upstream's warnings print them.
pub(crate) fn host_port(address: &SocketAddress) -> (String, c_int) {
    use swoole_rs_clients::address::{swr_v1_clients_address_port, with_host};
    let host = with_host(address, |host| host.to_string_lossy().into_owned());
    (host, swr_v1_clients_address_port(address))
}

/// `websocket::get_package_length` as a protocol's length callback: it reads only `pl`.
pub extern "C" fn websocket_length(
    _proto: *const Protocol,
    _socket: *mut c_void,
    pl: &mut PacketLength,
) -> isize {
    swoole_rs_websocket_ffi::frame_api::package_length(pl)
}

/// `get_package_length_size`'s result for `conn`.
fn length_size(conn: Connection<'_>) -> u8 {
    route(
        conn,
        || WEBSOCKET_FRAME_HEADER_SIZE,
        || HTTP2_FRAME_HEADER_SIZE,
        0,
    )
}

/// A server session as a port's package handlers see it: the port's server, the
/// session's connection and its socket, live for the read that lends them. The
/// server's reader makes one from the connection its read looked up; an `onPackage`
/// callback, from the protocol and socket it is called with.
#[derive(Clone, Copy)]
pub(crate) struct Session<'a> {
    pub(crate) server: swoole_rs_server::view::Server<'a>,
    pub(crate) conn: Connection<'a>,
    pub(crate) socket: swoole_rs_reactor::Socket<'a>,
}

impl Session<'_> {
    /// `Server::dispatch_task` of `data` to the workers; whether it went.
    pub(crate) fn task(self, data: swoole_rs_server::reactor_entry::RecvData<'_>) -> bool {
        swoole_rs_server::reactor_entry::dispatch_data(self.server, self.conn, data)
    }
}

pub mod exports {
    use std::ffi::{c_int, c_void};
    use std::mem::MaybeUninit;
    use swoole_rs_corebase::CCast;

    use swoole_rs_buffer::string::boxed_string;
    use swoole_rs_clients::protocol::Protocol;
    use swoole_rs_corebase::DataHead;
    use swoole_rs_corebase::net_types::{PacketLength, RecvData};
    use swoole_rs_corebase::{WebSocketStatus, log};
    use swoole_rs_reactor::{Drain, Socket, SocketRecord};
    use swoole_rs_server::{Connection, view};
    use swoole_rs_websocket::{
        Action, Context, FrameFlags, Opcode, PolicyError, decide_frame, encode_parts,
    };
    use swoole_rs_websocket_ffi::frame_api::decode_frame;

    use super::{Session, WEBSOCKET_FRAME_HEADER_SIZE, length_size, route};
    use crate::framing_recv::{Package, Packages, Recv, paused};

    /// `swoole::http2::get_frame_length(const Protocol *, Socket *, PacketLength *)`:
    /// the length callback of an HTTP/2 port, and of crates/phphttp2's client and
    /// crates/phphttp's coroutine server.
    pub extern "C" fn http2_length(
        _proto: *const Protocol,
        _socket: *mut c_void,
        pl: &mut PacketLength,
    ) -> isize {
        swoole_rs_http2::frame_length(pl.bytes()).map_or(0, usize::cast_signed)
    }

    /// The connection a server socket's `object` holds.
    ///
    /// # Safety
    /// `socket` is a live server connection socket for `'a`.
    unsafe fn connection<'a>(socket: *mut SocketRecord) -> Connection<'a> {
        // SAFETY: as the caller guarantees.
        let conn = unsafe { (*socket).fields.object };
        // SAFETY: a connection socket's object is its live connection slot, shared
        // through the slot's handle, as its fields are cells and atomics.
        Connection::from(unsafe { &*conn.cast::<swoole_rs_server::ConnectionRecord>() })
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
        // Out of the connection while this frame works on it: a close the dispatch
        // runs finds none to free.
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
                s.task(swoole_rs_server::reactor_entry::RecvData::new(
                    head,
                    buffer.view(),
                ));
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
                Some(payload) => swoole_rs_server::reactor_entry::RecvData::new(head, payload),
                None => swoole_rs_server::reactor_entry::RecvData::empty(head),
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
                let size = payload.and_then(|payload| {
                    encode_parts(
                        payload.len(),
                        payload,
                        Opcode::Close,
                        FrameFlags::default().with_final(),
                        &mut reply,
                    )
                    .ok()
                });
                if let Some(size) = size {
                    // SAFETY: the encoded reply's `size` bytes.
                    let reply = unsafe { Drain::from_raw(reply.as_ptr().cast(), size) };
                    _ = swoole_rs_tls::upstream::send_drain(s.socket, reply, 0);
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
            s.task(package.send_data(info))
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

    /// A server port protocol's `onPackage`: `H` over the session of `socket` and the
    /// package, -1 for a socket outside the server's connection table.
    ///
    /// # Safety
    /// Upstream's: the port's protocol (its `private_data_2` the server), a live
    /// connection socket of that port, and a whole package of it, which `H` may
    /// change in place.
    #[must_use]
    pub(crate) unsafe extern "C-unwind" fn on_package<H: Handler>(
        proto: *const Protocol,
        socket: Socket<'_>,
        rdata: &RecvData,
    ) -> c_int {
        // SAFETY: as the caller guarantees.
        let serv = unsafe { (*proto).private_data_2 };
        // SAFETY: a port's protocol keeps its live server.
        let server = unsafe { view::Server::from_raw(serv) };
        let Some(conn) = server.at(socket.fd()) else {
            return -1;
        };
        // SAFETY: the package's `len` bytes, as the caller guarantees.
        let package = unsafe { Package::from_raw(rdata.data, rdata.info.len as usize) };
        let s = Session {
            server,
            conn,
            socket,
        };
        if H::handle(s, package, rdata.info) {
            0
        } else {
            -1
        }
    }

    /// A session's packages, all [`Session::task`]'s: an EOF or length port's, whose
    /// protocol's `onPackage` `init_protocol` set to [`Task`]'s with its reader.
    pub(crate) struct Tasks<'a>(pub(crate) Session<'a>);

    impl Packages for Tasks<'_> {
        const HEAD: Option<u32> = None;
        #[inline(always)]
        fn dispatch(&self, _: &Recv<'_>, mut package: Package<'_>) -> bool {
            self.0.task(package.send_data(DataHead::default()))
        }
        fn paused(&self) -> bool {
            paused(self.0.conn)
        }
    }

    /// A session's packages through its port protocol's own callbacks: an HTTP/2
    /// stream's.
    pub(crate) struct Port<'a>(pub(crate) Session<'a>);

    impl Packages for Port<'_> {
        const HEAD: Option<u32> = None;
        #[inline(always)]
        fn dispatch(&self, r: &Recv<'_>, package: Package<'_>) -> bool {
            crate::framing_recv::protocol_dispatch(r, &package)
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
        fn length(&self, _: &Recv<'_>, bytes: &[u8]) -> i64 {
            match swoole_rs_websocket::frame_length(bytes) {
                Ok(frame) => i64::try_from(frame.total).unwrap_or(-1),
                Err(swoole_rs_websocket::LengthError::IncompleteHeader { .. }) => 0,
                Err(swoole_rs_websocket::LengthError::Overflow { .. }) => -1,
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

    /// `ssize_t get_package_length(const Protocol *, Socket *, PacketLength *)`.
    ///
    /// # Safety
    /// Upstream's: the port's protocol, a connection socket and its packet length.
    pub unsafe extern "C" fn get_package_length(
        proto: *const Protocol,
        socket: *mut c_void,
        pl: &mut PacketLength,
    ) -> isize {
        // SAFETY: as the caller guarantees.
        let conn = unsafe { connection(socket.cast()) };
        match route(conn, || Some(true), || Some(false), None) {
            Some(true) => super::websocket_length(proto, socket, pl),
            Some(false) => http2_length(proto, socket, pl),
            None => -1,
        }
    }

    /// `uint8_t get_package_length_size(Socket *)`.
    ///
    /// # Safety
    /// `socket` is a connection socket.
    pub unsafe extern "C" fn get_package_length_size(socket: *mut c_void) -> u8 {
        // SAFETY: as the caller guarantees.
        length_size(unsafe { connection(socket.cast()) })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
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
                // SAFETY: the record is plain integers, pointers and atomics.
                let mut conn: ConnectionRecord = unsafe { std::mem::zeroed() };
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
                    length_size(conn),
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
