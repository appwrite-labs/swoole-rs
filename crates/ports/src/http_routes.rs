//! `swoole::http_server`'s request and frame routing from `src/protocol/http.cc`:
//! the callbacks `init_protocol` puts on an HTTP port, defined under their
//! upstream (Itanium) symbols when the hybrid links this crate (`upstream-symbols`).
//! A connection past the WebSocket handshake goes to WebSocket, an HTTP/2 one to
//! HTTP/2, and any other is a protocol error. WebSocket's own length and frame
//! dispatch from `src/protocol/websocket.cc` live here too.

use std::ffi::c_int;

use swoole_rs_corebase::log;
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
    match swoole_rs_http::swr_v1_http_transport_route(
        conn.websocket_status.into(),
        conn.http2_stream.into(),
    ) {
        1 => websocket(),
        2 => http2(),
        _ => {
            // SAFETY: a connection's address holds its socket type's member.
            let host = unsafe {
                swoole_rs_clients::address::swr_v1_clients_address_host(&raw const conn.info)
            };
            // SAFETY: as above.
            let port = unsafe {
                swoole_rs_clients::address::swr_v1_clients_address_port(&raw const conn.info)
            };
            // SAFETY: the address text is NUL-terminated and lives until the next lookup.
            let host = unsafe { std::ffi::CStr::from_ptr(host) }.to_string_lossy();
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
    use std::sync::atomic::Ordering;

    use swoole_rs_buffer::string::{CppString, delete_string, new_string};
    use swoole_rs_corebase::{DataHead, log, net_types::Protocol};
    use swoole_rs_reactor::SocketRecord;
    use swoole_rs_server::{ConnectionRecord, view};
    use swoole_rs_websocket::frame_api::decode_frame;
    use swoole_rs_websocket::{
        Action, Context, Decision, PolicyError, swr_v1_ws_decide, swr_v1_ws_encode,
    };

    use super::{length_size, route};

    /// `SW_HTTP_SERVICE_UNAVAILABLE_PACKET`.
    const SERVICE_UNAVAILABLE: &[u8] = b"HTTP/1.1 503 Service Unavailable\r\n\r\n";

    unsafe extern "C" {
        /// `Server::dispatch_task`.
        #[link_name = "_ZN6swoole6Server13dispatch_taskEPKNS_8ProtocolEPNS_7network6SocketEPKNS_8RecvDataE"]
        pub fn dispatch_task(
            proto: *const c_void,
            socket: *mut SocketRecord,
            rdata: *const c_void,
        ) -> c_int;
        #[link_name = "_ZN6swoole5http216get_frame_lengthEPKNS_8ProtocolEPNS_7network6SocketEPNS_12PacketLengthE"]
        pub fn http2_length(
            proto: *const c_void,
            socket: *mut SocketRecord,
            pl: *mut c_void,
        ) -> isize;
    }

    /// The connection a server socket's `object` holds.
    ///
    /// # Safety
    /// `socket` is a live server connection socket.
    unsafe fn connection<'a>(socket: *mut SocketRecord) -> &'a mut ConnectionRecord {
        // SAFETY: as the caller guarantees.
        let conn = unsafe { (*socket).object }.cast::<ConnectionRecord>();
        // SAFETY: a connection socket's object is its live connection, which only
        // its reactor thread touches.
        unsafe { &mut *conn }
    }

    /// `swoole::RecvData`.
    #[repr(C)]
    struct RecvData {
        info: DataHead,
        data: *const u8,
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
        // SAFETY: a connection's address holds its socket type's member.
        let host = unsafe {
            swoole_rs_clients::address::swr_v1_clients_address_host(&raw const conn.info)
        };
        // SAFETY: as above.
        let port = unsafe {
            swoole_rs_clients::address::swr_v1_clients_address_port(&raw const conn.info)
        };
        // SAFETY: the address text is NUL-terminated and lives until the next lookup.
        let host = unsafe { std::ffi::CStr::from_ptr(host) }.to_string_lossy();
        log::warning(
            c"websocket::dispatch_frame",
            format_args!("{what}. remote_addr={host}:{port}"),
        );
    }

    /// `int websocket::dispatch_frame(const Protocol *, Socket *, const RecvData *)`:
    /// decode the frame, let [`swr_v1_ws_decide`] choose, and carry out its action on
    /// the connection's fragment buffer, the server and the socket.
    ///
    /// # Safety
    /// Upstream's: the port's protocol (its `private_data_2` the server), a
    /// connection socket and its whole frame.
    #[cfg_attr(
        feature = "upstream-symbols",
        unsafe(
            export_name = "_ZN6swoole9websocket14dispatch_frameEPKNS_8ProtocolEPNS_7network6SocketEPKNS_8RecvDataE"
        )
    )]
    pub unsafe extern "C" fn websocket_dispatch_frame(
        proto: *const c_void,
        socket: *mut SocketRecord,
        rdata: *const c_void,
    ) -> c_int {
        // SAFETY: as the caller guarantees.
        let serv = unsafe { (*proto.cast::<Protocol>()).private_data_2 };
        // SAFETY: as the caller guarantees.
        let rdata = unsafe { &*rdata.cast::<RecvData>() };
        let (data, length) = (rdata.data, rdata.info.len as usize);
        // SAFETY: `decode` unmasks the frame's bytes in place, as upstream's does.
        let Ok(frame) = (unsafe { decode_frame(data.cast_mut(), length) }) else {
            return -1;
        };
        // SAFETY: as the caller guarantees.
        let conn = unsafe { connection(socket) };
        let fragment = conn.websocket_buffer.cast::<CppString>();
        let mut context = Context {
            has_fragment: u8::from(!fragment.is_null()),
            closing: u8::from(conn.websocket_status == STATUS_CLOSING),
            ..Context::default()
        };
        if !fragment.is_null() {
            // SAFETY: the connection's live fragment buffer.
            let fragment = unsafe { &*fragment };
            context.fragment_length = fragment.length;
            #[expect(
                clippy::cast_possible_truncation,
                clippy::cast_sign_loss,
                reason = "the fragment's offset holds its uint16_t ext_flags"
            )]
            let flags = fragment.offset as u16;
            context.fragment_flags = flags;
            if frame.opcode == 0 {
                let conn: *mut ConnectionRecord = &raw mut *conn;
                // SAFETY: a live server and one of its connections.
                let max = unsafe {
                    swoole_rs_server::server_methods::package_max_length(serv, conn.cast())
                };
                context.package_max_length = max as usize;
            }
        }
        let mut decision = Decision::default();
        // SAFETY: local inputs and output.
        if unsafe {
            swr_v1_ws_decide(
                &raw const frame,
                length,
                &raw const context,
                &raw mut decision,
            )
        } != 0
        {
            return -1;
        }
        policy_warning(conn, decision.error, frame.opcode);
        let mut dispatch = RecvData {
            // SAFETY: plain integers.
            info: unsafe { std::mem::zeroed() },
            data: data.wrapping_add(decision.offset),
        };
        dispatch.info.ext_flags = decision.ext_flags;
        #[expect(
            clippy::cast_possible_truncation,
            reason = "a payload inside a uint32_t-sized frame"
        )]
        let payload = decision.length as u32;
        dispatch.info.len = payload;
        match decision.action {
            Action::Reject => return -1,
            Action::Start => {
                // SAFETY: the payload's bytes.
                let buffer = unsafe { new_string(dispatch.data, decision.length) };
                // SAFETY: just constructed.
                unsafe { (*buffer).offset = decision.ext_flags.into() };
                conn.websocket_buffer = buffer.cast();
            }
            Action::Append | Action::Finish => {
                // SAFETY: the decision appends only to a live fragment.
                let buffer = unsafe { &mut *fragment };
                // SAFETY: the payload's bytes.
                unsafe { buffer.append(dispatch.data, decision.length) };
                if decision.action == Action::Finish {
                    #[expect(
                        clippy::cast_possible_truncation,
                        reason = "bounded by package_max_length"
                    )]
                    let whole = buffer.length as u32;
                    dispatch.info.len = whole;
                    dispatch.data = buffer.str;
                    // SAFETY: upstream's arguments, with the whole message.
                    unsafe { dispatch_task(proto, socket, (&raw const dispatch).cast()) };
                    // SAFETY: `new String`'s, not used again.
                    unsafe { delete_string(fragment) };
                    conn.websocket_buffer = std::ptr::null_mut();
                }
            }
            Action::Control | Action::Dispatch => {
                if decision.action == Action::Control && decision.length == 0 {
                    dispatch.data = std::ptr::null();
                }
                // SAFETY: upstream's arguments, with the frame's payload.
                unsafe { dispatch_task(proto, socket, (&raw const dispatch).cast()) };
            }
            Action::ClientClose => {
                // SAFETY: as above.
                unsafe { dispatch_task(proto, socket, (&raw const dispatch).cast()) };
                let mut reply = [0u8; CLOSE_REPLY_MAX];
                // SAFETY: the close payload and a disjoint local output.
                let size = unsafe {
                    swr_v1_ws_encode(
                        dispatch.data,
                        decision.length,
                        8,
                        1,
                        reply.as_mut_ptr(),
                        reply.len(),
                    )
                };
                if size != 0 {
                    // SAFETY: a live socket and the encoded reply.
                    unsafe {
                        swoole_rs_tls::upstream::socket_send(socket, reply.as_ptr().cast(), size, 0)
                    };
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

    /// `int dispatch_request(Server *, const Protocol *, Socket *, const RecvData *)`:
    /// 503 while the server is at `max_concurrency`, else `Server::dispatch_task`.
    ///
    /// # Safety
    /// Upstream's: a live server, its port's protocol, a connection socket and its data.
    #[cfg_attr(
        feature = "upstream-symbols",
        unsafe(
            export_name = "_ZN6swoole11http_server16dispatch_requestEPNS_6ServerEPKNS_8ProtocolEPNS_7network6SocketEPKNS_8RecvDataE"
        )
    )]
    pub unsafe extern "C" fn dispatch_request(
        serv: *mut c_void,
        proto: *const c_void,
        socket: *mut SocketRecord,
        rdata: *const c_void,
    ) -> c_int {
        // SAFETY: a live server with its shared memory.
        let counters = unsafe { view::counters(serv) };
        if counters.concurrency.load(Ordering::Relaxed) >= counters.max_concurrency {
            // SAFETY: a live socket and a static packet.
            unsafe {
                swoole_rs_tls::upstream::socket_send(
                    socket,
                    SERVICE_UNAVAILABLE.as_ptr().cast(),
                    SERVICE_UNAVAILABLE.len(),
                    0,
                )
            };
            return -1;
        }
        // SAFETY: upstream's arguments, forwarded.
        unsafe { dispatch_task(proto, socket, rdata) }
    }

    /// `ssize_t get_package_length(const Protocol *, Socket *, PacketLength *)`.
    ///
    /// # Safety
    /// Upstream's: the port's protocol, a connection socket and its packet length.
    #[cfg_attr(
        feature = "upstream-symbols",
        unsafe(
            export_name = "_ZN6swoole11http_server18get_package_lengthEPKNS_8ProtocolEPNS_7network6SocketEPNS_12PacketLengthE"
        )
    )]
    pub unsafe extern "C" fn get_package_length(
        proto: *const c_void,
        socket: *mut SocketRecord,
        pl: *mut c_void,
    ) -> isize {
        // SAFETY: as the caller guarantees.
        let conn = unsafe { connection(socket) };
        route(
            conn,
            // SAFETY: upstream's arguments, forwarded.
            || unsafe {
                swoole_rs_websocket::frame_api::get_package_length(proto, socket.cast(), pl.cast())
            },
            // SAFETY: as above.
            || unsafe { http2_length(proto, socket, pl) },
            -1,
        )
    }

    /// `uint8_t get_package_length_size(Socket *)`.
    ///
    /// # Safety
    /// `socket` is a connection socket.
    #[cfg_attr(
        feature = "upstream-symbols",
        unsafe(
            export_name = "_ZN6swoole11http_server23get_package_length_sizeEPNS_7network6SocketE"
        )
    )]
    pub unsafe extern "C" fn get_package_length_size(socket: *mut SocketRecord) -> u8 {
        // SAFETY: as the caller guarantees.
        length_size(unsafe { connection(socket) })
    }

    /// `int dispatch_frame(const Protocol *, Socket *, const RecvData *)`.
    ///
    /// # Safety
    /// Upstream's: the port's protocol, a connection socket and its data.
    #[cfg_attr(
        feature = "upstream-symbols",
        unsafe(
            export_name = "_ZN6swoole11http_server14dispatch_frameEPKNS_8ProtocolEPNS_7network6SocketEPKNS_8RecvDataE"
        )
    )]
    pub unsafe extern "C" fn dispatch_frame(
        proto: *const c_void,
        socket: *mut SocketRecord,
        rdata: *const c_void,
    ) -> c_int {
        // SAFETY: as the caller guarantees.
        let conn = unsafe { connection(socket) };
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
    fn pinned(websocket_status: u8, http2_stream: u8) -> u32 {
        // `conn->websocket_status >= websocket::STATUS_HANDSHAKE`, then `conn->http2_stream`.
        if websocket_status >= 2 {
            1
        } else if http2_stream != 0 {
            2
        } else {
            0
        }
    }

    /// The retired dispatch gate's route cases: every WebSocket status against
    /// HTTP/2 off, on and 255, through each of the three routes.
    #[test]
    fn routes_match_the_pinned_branches() {
        let mut cases = 0;
        for websocket_status in 0..=u8::MAX {
            for http2_stream in [0, 1, 255] {
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
        assert_eq!(cases, 256 * 3 * 3);
        assert_eq!(WEBSOCKET_FRAME_HEADER_SIZE, 14);
    }
}
