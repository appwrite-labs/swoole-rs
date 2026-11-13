//! `swoole::http_server`'s request and frame routing from `src/protocol/http.cc`:
//! the callbacks `init_protocol` puts on an HTTP port, defined under their
//! upstream (Itanium) symbols when the hybrid links this crate (`upstream-symbols`).
//! A connection past the WebSocket handshake goes to WebSocket, an HTTP/2 one to
//! HTTP/2, and any other is a protocol error. WebSocket's own length and frame
//! dispatch from `src/protocol/websocket.cc` live here too.

use std::ffi::c_int;

use swoole_rs_corebase::log;
use swoole_rs_corebase::net_types::SocketAddress;
use swoole_rs_error::ErrorCode;
use swoole_rs_server::ConnectionRecord;

/// `SW_LOG_WARNING`.
const LOG_WARNING: c_int = 4;
/// `SW_WEBSOCKET_FRAME_HEADER_SIZE`: header, mask and a 64-bit length.
const WEBSOCKET_FRAME_HEADER_SIZE: u8 = 2 + 4 + 8;
/// `SW_HTTP2_FRAME_HEADER_SIZE`.
const HTTP2_FRAME_HEADER_SIZE: u8 = 9;

/// `conn`'s branch: `websocket` past the handshake, else `http2` for an HTTP/2
/// stream, else upstream's `protocol_status_error` warning and `error`.
fn route<T>(
    conn: &ConnectionRecord,
    websocket: impl FnOnce() -> T,
    http2: impl FnOnce() -> T,
    error: T,
) -> T {
    match swoole_rs_http::transport_route(conn.websocket_status.into(), conn.http2_stream.into()) {
        1 => websocket(),
        2 => http2(),
        _ => {
            let (host, port) = host_port(&conn.info);
            log::error_log(
                LOG_WARNING,
                ErrorCode::SW_ERROR_PROTOCOL_ERROR as c_int,
                c"protocol_status_error",
                format_args!(
                    "unexpected protocol status of session#{}<{host}:{port}>",
                    conn.session_id
                ),
            );
            error
        }
    }
}

/// The host and port of an address that holds its socket type's member, as
/// upstream's warnings print them.
pub(crate) fn host_port(address: &SocketAddress) -> (String, c_int) {
    use swoole_rs_clients::address::{swr_v1_clients_address_port, with_host};
    let host = with_host(address, |host| host.to_string_lossy().into_owned());
    (host, swr_v1_clients_address_port(address))
}

/// `get_package_length_size`'s result for `conn`.
fn length_size(conn: &ConnectionRecord) -> u8 {
    route(
        conn,
        || WEBSOCKET_FRAME_HEADER_SIZE,
        || HTTP2_FRAME_HEADER_SIZE,
        0,
    )
}

#[cfg(not(test))]
pub mod exports {
    use std::ffi::{c_int, c_void};
    use std::mem::MaybeUninit;
    use swoole_rs_corebase::CCast;

    use swoole_rs_buffer::string::{CppString, delete_string, new_string};
    use swoole_rs_corebase::DataHead;
    use swoole_rs_corebase::log;
    use swoole_rs_corebase::net_types::{PacketLength, Protocol};
    use swoole_rs_reactor::{Drain, Socket, SocketRecord};
    use swoole_rs_server::{ConnectionRecord, view};
    use swoole_rs_websocket::{Action, Context, PolicyError, decide_frame, encode_parts};
    use swoole_rs_websocket_ffi::frame_api::decode_frame;

    use super::{WEBSOCKET_FRAME_HEADER_SIZE, length_size, route};
    use crate::framing_recv::{Packages, Recv};

    pub use swoole_rs_server::reactor_entry::dispatch_task;

    /// `swoole::http2::get_frame_length(const Protocol *, Socket *, PacketLength *)`:
    /// the length callback of an HTTP/2 port, and of crates/phphttp2's client and
    /// crates/phphttp's coroutine server.
    pub extern "C" fn http2_length(
        _proto: *const Protocol,
        _socket: *mut c_void,
        pl: &mut PacketLength,
    ) -> isize {
        swoole_rs_http2::frame_length(pl.bytes())
    }

    /// The connection a server socket's `object` holds.
    ///
    /// # Safety
    /// `socket` is a live server connection socket.
    unsafe fn connection<'a>(socket: *mut SocketRecord) -> &'a mut ConnectionRecord {
        // SAFETY: as the caller guarantees.
        let conn = unsafe { (*socket).fields.object }.cast::<ConnectionRecord>();
        // SAFETY: a connection socket's object is its live connection, which only
        // its reactor thread touches.
        unsafe { &mut *conn }
    }

    /// `swoole::RecvData`.
    #[repr(C)]
    pub(crate) struct RecvData {
        pub(crate) info: DataHead,
        pub(crate) data: *const u8,
    }

    /// `websocket::STATUS_CLOSING`.
    const STATUS_CLOSING: u8 = 4;
    /// `SW_WEBSOCKET_HEADER_LEN + SW_WEBSOCKET_CLOSE_CODE_LEN + SW_WEBSOCKET_CLOSE_REASON_MAX_LEN`.
    const CLOSE_REPLY_MAX: usize = 2 + 2 + 125;

    /// `websocket::dispatch_frame`'s policy warnings, with the peer's address.
    fn policy_warning(conn: &ConnectionRecord, error: PolicyError, opcode: u8) {
        let what = match error {
            PolicyError::Orphan => "bad frame[opcode=0]".to_owned(),
            PolicyError::TooBig => "websocket frame is too big".to_owned(),
            PolicyError::Overlap => "merging incomplete frame, bad request".to_owned(),
            PolicyError::ControlTooBig => format!(
                "{} frame application data is too big",
                if opcode == 9 { "ping" } else { "pong" }
            ),
            PolicyError::None | PolicyError::CloseTooBig => return,
        };
        let (host, port) = super::host_port(&conn.info);
        log::warning(
            c"websocket::dispatch_frame",
            format_args!("{what}. remote_addr={host}:{port}"),
        );
    }

    /// `int websocket::dispatch_frame(const Protocol *, Socket *, const RecvData *)`:
    /// decode the frame, let [`decide_frame`] choose, and carry out its action on
    /// the connection's fragment buffer, the server and the socket.
    ///
    /// # Safety
    /// Upstream's: the port's protocol (its `private_data_2` the server), a
    /// connection socket and its whole frame.
    pub unsafe extern "C-unwind" fn websocket_dispatch_frame(
        proto: *const Protocol,
        socket: *mut c_void,
        rdata: *const c_void,
    ) -> c_int {
        let socket = socket.cast::<SocketRecord>();
        // SAFETY: as the caller guarantees.
        let serv = unsafe { (*proto).private_data_2 };
        // SAFETY: as the caller guarantees.
        let rdata = unsafe { &*rdata.cast::<RecvData>() };
        let length = rdata.info.len as usize;
        let bytes = if length == 0 {
            &mut []
        } else {
            // SAFETY: the whole frame, as the caller guarantees; `decode` unmasks it in
            // place, as upstream's does.
            unsafe { std::slice::from_raw_parts_mut(rdata.data.cast_mut(), length) }
        };
        let Ok(frame) = decode_frame(bytes) else {
            return -1;
        };
        let bytes: &[u8] = bytes;
        // SAFETY: as the caller guarantees.
        let conn = unsafe { connection(socket) };
        let fragment = conn.websocket_buffer.cast::<CppString>();
        // SAFETY: null or the connection's live fragment buffer, which only this call
        // reaches until it deletes it.
        let mut buffered = unsafe { fragment.as_mut() };
        let mut context = Context {
            has_fragment: buffered.is_some(),
            closing: conn.websocket_status == STATUS_CLOSING,
            ..Context::default()
        };
        if let Some(fragment) = buffered.as_deref() {
            context.fragment_length = fragment.len();
            let flags = fragment.offset().c_cast::<u16>();
            context.fragment_flags = flags;
            if frame.opcode == 0 {
                // SAFETY: the port's live server, as the caller guarantees.
                let serv = unsafe { view::Server::from_raw(serv) };
                let max = swoole_rs_server::server_methods::package_max_length(serv, conn.fd);
                context.package_max_length = max as usize;
            }
        }
        let Some(decision) = decide_frame(&frame, length, &context) else {
            return -1;
        };
        if decision.error != PolicyError::None {
            policy_warning(conn, decision.error, frame.opcode);
        }
        let mut dispatch = RecvData {
            info: DataHead {
                len: decision.length.c_cast::<u32>(),
                ext_flags: decision.ext_flags,
                ..DataHead::default()
            },
            data: bytes.as_ptr().wrapping_add(decision.offset),
        };
        let payload = bytes
            .get(decision.offset..)
            .and_then(|rest| rest.get(..decision.length));
        let dispatch_task = |dispatch: &RecvData| {
            // SAFETY: upstream's arguments, with this frame's payload or whole message.
            unsafe { dispatch_task(proto, socket.cast(), (&raw const *dispatch).cast()) }
        };
        match decision.action {
            Action::Reject => return -1,
            Action::Start => {
                let buffer = new_string(payload.unwrap_or_default());
                // SAFETY: just constructed.
                unsafe { (*buffer).set_offset(decision.ext_flags.into()) };
                conn.websocket_buffer = buffer.cast();
            }
            Action::Append | Action::Finish => {
                // The decision appends only to a live fragment.
                let Some(buffer) = buffered.take() else {
                    return -1;
                };
                buffer.append(payload.unwrap_or_default());
                if decision.action == Action::Finish {
                    #[expect(
                        clippy::cast_possible_truncation,
                        reason = "bounded by package_max_length"
                    )]
                    let whole = buffer.len() as u32;
                    dispatch.info.len = whole;
                    dispatch.data = buffer.as_mut_ptr();
                    dispatch_task(&dispatch);
                    // SAFETY: `new String`'s, not used again.
                    unsafe { delete_string(fragment) };
                    conn.websocket_buffer = std::ptr::null_mut();
                }
            }
            Action::Control | Action::Dispatch => {
                if decision.action == Action::Control && decision.length == 0 {
                    dispatch.data = std::ptr::null();
                }
                dispatch_task(&dispatch);
            }
            Action::ClientClose => {
                dispatch_task(&dispatch);
                let mut reply = [MaybeUninit::<u8>::uninit(); CLOSE_REPLY_MAX];
                let size = payload.map_or(0, |payload| {
                    encode_parts(payload.len(), payload, 8, 1, &mut reply)
                });
                // SAFETY: as the caller guarantees, a live connection socket.
                let socket = unsafe { Socket::from_ptr(socket) };
                if let Some(socket) = socket.filter(|_| size != 0) {
                    // SAFETY: the encoded reply's `size` bytes.
                    let reply = unsafe { Drain::from_raw(reply.as_ptr().cast(), size) };
                    _ = swoole_rs_tls::upstream::send_drain(socket, reply, 0);
                }
                return -1;
            }
            Action::ServerClose => {
                conn.websocket_status = 0;
                return -1;
            }
            Action::Unknown => {
                log::warning(
                    c"websocket::dispatch_frame",
                    format_args!("unknown opcode [{}]", frame.opcode),
                );
            }
        }
        0
    }

    /// A WebSocket connection's frames, measured by `frame_length` and handed to
    /// [`websocket_dispatch_frame`] directly: an HTTP port's own callbacks route
    /// each frame through the connection's status to the same two.
    pub(crate) struct Frames;

    impl Packages for Frames {
        const HEAD: Option<u32> = Some(WEBSOCKET_FRAME_HEADER_SIZE as u32);
        #[inline(always)]
        fn length(_: &Recv, _: &Protocol, bytes: &[u8]) -> i64 {
            swoole_rs_websocket::frame_length(bytes).0
        }
        #[inline(always)]
        fn dispatch(r: &Recv, b: &CppString, start: usize) -> c_int {
            let frame = RecvData {
                info: DataHead {
                    len: u32::try_from(b.offset()).unwrap_or(u32::MAX),
                    ..DataHead::default()
                },
                data: b.as_ptr().wrapping_add(start),
            };
            // SAFETY: the port's protocol, a connection socket and its whole frame.
            unsafe {
                websocket_dispatch_frame(
                    r.protocol,
                    r.socket.as_ptr().cast(),
                    (&raw const frame).cast(),
                )
            }
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
            Some(true) => swoole_rs_websocket_ffi::frame_api::get_package_length(proto, socket, pl),
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

    /// `int dispatch_frame(const Protocol *, Socket *, const RecvData *)`.
    ///
    /// # Safety
    /// Upstream's: the port's protocol, a connection socket and its data.
    pub unsafe extern "C-unwind" fn dispatch_frame(
        proto: *const Protocol,
        socket: *mut c_void,
        rdata: *const c_void,
    ) -> c_int {
        // SAFETY: as the caller guarantees.
        let conn = unsafe { connection(socket.cast()) };
        route(
            conn,
            // SAFETY: upstream's arguments, forwarded.
            || unsafe { websocket_dispatch_frame(proto, socket, rdata) },
            // SAFETY: as above.
            || unsafe { dispatch_task(proto, socket, rdata) },
            -1,
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Pinned `http.cc`'s branch for a connection: 1 WebSocket, 2 HTTP/2, 0 error.
    fn pinned(websocket_status: u8, http2_stream: bool) -> u32 {
        // `conn->websocket_status >= websocket::STATUS_HANDSHAKE`, then `conn->http2_stream`.
        if websocket_status >= 2 {
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
        for websocket_status in 0..=u8::MAX {
            for http2_stream in [false, true] {
                // SAFETY: the record is plain integers, pointers and atomics.
                let mut conn: ConnectionRecord = unsafe { std::mem::zeroed() };
                conn.websocket_status = websocket_status;
                conn.http2_stream = http2_stream;
                conn.info.kind = 1;
                let expected = pinned(websocket_status, http2_stream);
                let mut calls = (0, 0);
                let length = route(
                    &conn,
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
                    length_size(&conn),
                    [0, WEBSOCKET_FRAME_HEADER_SIZE, HTTP2_FRAME_HEADER_SIZE][expected as usize]
                );
                let frame = route(&conn, || -22, || 55, -1);
                assert_eq!(frame, [-1, -22, 55][expected as usize]);
                cases += 3;
            }
        }
        assert_eq!(cases, 256 * 2 * 3);
        assert_eq!(WEBSOCKET_FRAME_HEADER_SIZE, 14);
    }
}
