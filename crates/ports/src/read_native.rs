//! The native effects of `ListenPort`'s readable callbacks ([`crate::read`]), over
//! the records Rust lays out: the reactor's event and server, the connection, its
//! socket and receive buffer, the port's protocol and the server's settings.
//! Logs name the upstream function that wrote them.
use core::ffi::{CStr, c_int, c_void};
use core::ptr;

use swoole_rs_buffer::string::{CppString, delete_string, new_sized_string};
use swoole_rs_corebase::constants::{
    SW_BUFFER_SIZE_BIG, SW_HTTP_100_CONTINUE_PACKET, SW_HTTP_BAD_REQUEST_PACKET,
    SW_HTTP_REQUEST_ENTITY_TOO_LARGE_PACKET, SW_HTTP_SERVICE_UNAVAILABLE_PACKET,
};
use swoole_rs_corebase::net_types::Protocol;
use swoole_rs_corebase::{DataHead, log};
use swoole_rs_http::HttpRequest;
use swoole_rs_reactor::reactor_upstream::{prepared, trigger_close_event};
use swoole_rs_reactor::{Event, SocketRecord};
use swoole_rs_server::port_config::{self, PortConfig};
use swoole_rs_server::static_select::swr_v1_server_static_select;
use swoole_rs_server::{ConnectionRecord, ServerConfig, view};
use swoole_rs_tls::upstream::{socket_recv, socket_send};

use crate::http_routes::exports::{RecvData, dispatch_request, dispatch_task};
use crate::read::{
    ON_READ_HTTP, OP_ACTIVE, OP_BUF_EXTEND, OP_BUF_HEADER_END, OP_BUF_REDUCE, OP_CATCH, OP_CLIENT,
    OP_CLOSE, OP_CLOSE_ERRNO, OP_DISPATCH_REQUEST, OP_DISPATCH_TASK, OP_HTTP2_ENABLED,
    OP_HTTP2_START, OP_MAX_LENGTH, OP_RECV, OP_RECV_BUFFER, OP_RECV_EOF, OP_RECV_LENGTH,
    OP_RECV_REDIS, OP_RELEASE, OP_REQ_DESTROY, OP_REQ_INIT, OP_REQ_MULTIPART_INIT, OP_SEND,
    OP_SKIP_RECV, OP_STATIC, OP_UPLOADS, OP_WARN_RECV, OP_WEBSOCKET_ACTIVATE, SEND_BAD_REQUEST,
    SEND_TOO_LARGE, SEND_UNAVAILABLE, Shared, swr_v1_ports_on_read,
};

type Receive = unsafe extern "C" fn(*const Protocol, *mut c_void, *mut CppString) -> c_int;

unsafe extern "C" {
    #[link_name = "_ZNK6swoole8Protocol25recv_with_length_protocolEPNS_7network6SocketEPNS_6StringE"]
    fn recv_with_length_protocol(
        protocol: *const Protocol,
        socket: *mut c_void,
        buffer: *mut CppString,
    ) -> c_int;
    #[link_name = "_ZNK6swoole8Protocol22recv_with_eof_protocolEPNS_7network6SocketEPNS_6StringE"]
    fn recv_with_eof_protocol(
        protocol: *const Protocol,
        socket: *mut c_void,
        buffer: *mut CppString,
    ) -> c_int;
    #[link_name = "_ZN6swoole5redis11recv_packetEPNS_8ProtocolEPNS_10ConnectionEPNS_6StringE"]
    fn redis_recv_packet(
        protocol: *const Protocol,
        conn: *mut c_void,
        buffer: *mut CppString,
    ) -> c_int;
}

/// `websocket::STATUS_HANDSHAKE` and `STATUS_ACTIVE`.
const STATUS_HANDSHAKE: u8 = 2;
const STATUS_ACTIVE: u8 = 3;
/// `SW_HTTP2_SETTING_FRAME_SIZE`: a frame header and six settings.
const SETTING_FRAME_SIZE: usize = 9 + 6 * 6;

/// One readable event: the context [`swr_v1_ports_on_read`] runs over.
#[repr(C)]
struct Read {
    shared: Shared,
    reactor: *mut c_void,
    event: *mut Event,
    socket: *mut SocketRecord,
    conn: *mut ConnectionRecord,
    serv: *mut c_void,
    port: *mut PortConfig,
    kind: u32,
}

/// `ListenPort::readable_callback_*` of `kind` on the reactor's `event`.
///
/// # Safety
/// The reactor's readable event on a live connection of the live `port`.
pub(crate) unsafe fn on_read(
    reactor: *mut c_void,
    port: *mut c_void,
    event: *mut c_void,
    kind: u32,
) -> c_int {
    let event = event.cast::<Event>();
    // SAFETY: as the caller guarantees.
    let socket = unsafe { (*event).socket }.cast::<SocketRecord>();
    // SAFETY: a connection socket's object is its connection.
    let conn = unsafe { (*socket).object }.cast::<ConnectionRecord>();
    // SAFETY: a live reactor.
    let fields = unsafe { prepared(reactor) }.fields;
    // SAFETY: its fields, whose `ptr` is its server.
    let serv = unsafe { (*fields).ptr };
    let mut read = Read {
        shared: Shared {
            buffer: ptr::null_mut(),
            request: ptr::null_mut(),
            websocket: 0,
            upgraded: 0,
            http2_stream: 0,
            static_handler: 0,
        },
        reactor,
        event,
        socket,
        conn,
        serv,
        // SAFETY: as the caller guarantees.
        port: unsafe { port_config::of(port) },
        kind,
    };
    if kind == ON_READ_HTTP {
        // SAFETY: the live connection.
        let c = unsafe { &*conn };
        read.shared.websocket = u8::from(c.websocket_status >= STATUS_HANDSHAKE);
        read.shared.upgraded = c.http_upgrade;
        read.shared.http2_stream = c.http2_stream;
        read.shared.static_handler = u8::from(config(&read).enable_static_handler);
    }
    // SAFETY: `read` starts with its `Shared` and outlives the call.
    unsafe { swr_v1_ports_on_read(kind, (&raw mut read).cast(), call) }
}

/// A native status or byte count, as the `u64` [`crate::read`] reads back signed.
fn status(value: impl Into<i64>) -> u64 {
    value.into().cast_unsigned()
}

fn errno() -> c_int {
    std::io::Error::last_os_error().raw_os_error().unwrap_or(0)
}

fn config(c: &Read) -> &ServerConfig {
    // SAFETY: the live server; this reactor thread only reads its settings.
    unsafe { view::config(c.serv) }
}

/// The current buffer, borrowed until the next native call.
#[expect(
    clippy::mut_from_ref,
    reason = "native memory: the borrow spans no native call"
)]
fn buffer(c: &Read) -> &mut CppString {
    // SAFETY: the current buffer is live once a receive buffer or request made it current.
    unsafe { &mut *c.shared.buffer.cast::<CppString>() }
}

/// `serv->get_recv_buffer(socket)`.
fn recv_buffer(c: &Read) -> *mut CppString {
    // SAFETY: the live socket, which only this reactor thread touches.
    let socket = unsafe { &mut *c.socket };
    if socket.recv_buffer.is_null() {
        let allocator = config(c).recv_buffer_allocator.cast();
        // SAFETY: the server's allocator, null or live.
        let made = unsafe { new_sized_string(SW_BUFFER_SIZE_BIG as usize, allocator) };
        socket.recv_buffer = made.cast();
    }
    socket.recv_buffer.cast()
}

fn send(c: &Read, packet: &[u8]) -> u64 {
    // SAFETY: the live socket and `packet`'s bytes.
    status(unsafe { socket_send(c.socket, packet.as_ptr().cast(), packet.len(), 0) } as i64)
}

/// One receive through the port's protocol.
fn receive(c: &Read, how: Receive, target: *mut c_void) -> u64 {
    // SAFETY: the live port.
    let protocol = unsafe { &raw const (*c.port).protocol };
    // SAFETY: the port's protocol, a live socket or connection and the current buffer.
    status(unsafe { how(protocol, target, c.shared.buffer.cast()) })
}

/// The effects every read makes.
unsafe extern "C" fn call(context: *mut c_void, op: u32, arg: u64) -> u64 {
    // SAFETY: `on_read`'s live context.
    let c = unsafe { &mut *context.cast::<Read>() };
    match op {
        OP_RECV => {
            let b = buffer(c);
            let at = if arg == 0 { b.length } else { 0 };
            // SAFETY: `at` is within the live buffer.
            let start = unsafe { b.str.add(at) };
            // SAFETY: the live socket and `size - at` writable bytes at `start`.
            let received = unsafe { socket_recv(c.socket, start.cast(), b.size - at, 0) };
            status(received as i64)
        }
        OP_RECV_BUFFER => {
            c.shared.buffer = recv_buffer(c).cast();
            1
        }
        OP_RELEASE => {
            // SAFETY: the live socket.
            let socket = unsafe { &mut *c.socket };
            let held = socket.recv_buffer.cast::<CppString>();
            let release = !held.is_null() && {
                // SAFETY: the socket's receive buffer, when set, is live.
                let held = unsafe { &*held };
                (arg == 0 || held.length == 0) && held.size > SW_BUFFER_SIZE_BIG as usize * 2
            };
            if release {
                // SAFETY: `new String`'s, which nothing uses after.
                unsafe { delete_string(held) };
                socket.recv_buffer = ptr::null_mut();
            }
            u64::from(release)
        }
        OP_RECV_LENGTH => receive(c, recv_with_length_protocol, c.socket.cast()),
        OP_ACTIVE => {
            // SAFETY: the live connection.
            let active = unsafe { (*c.conn).active };
            // SAFETY: the live socket.
            let removed = unsafe { (*c.socket).removed };
            u64::from(active != 0 && removed == 0)
        }
        // SAFETY: the live port.
        OP_MAX_LENGTH => u64::from(unsafe { (*c.port).protocol.package_max_length }),
        OP_REQ_INIT => {
            // SAFETY: the live connection.
            let conn = unsafe { &mut *c.conn };
            if conn.object.is_null() {
                conn.object = swoole_rs_http::swr_v1_http_request_new().cast();
            }
            let request = conn.object.cast::<HttpRequest>();
            // SAFETY: the connection's live request.
            let r = unsafe { &mut *request };
            if r.buffer.is_null() {
                r.buffer = recv_buffer(c).cast();
            }
            c.shared.buffer = r.buffer;
            c.shared.request = request;
            0
        }
        OP_REQ_DESTROY => {
            // SAFETY: the live connection, whose request is null or Rust's.
            unsafe { crate::methods::destroy_http_request(ptr::null_mut(), c.conn) };
            c.shared.request = ptr::null_mut();
            0
        }
        _ => rare(c, op, arg),
    }
}

/// The effects some reads make.
fn rare(c: &mut Read, op: u32, arg: u64) -> u64 {
    // SAFETY: the live connection, which only this reactor thread touches; each
    // borrow ends before the next native call.
    let conn = || unsafe { &mut *c.conn };
    // SAFETY: the live port, whose settings nothing writes while it serves.
    let port = || unsafe { &*c.port };
    match op {
        // SAFETY: the live socket.
        OP_CATCH => status(unsafe { (*c.socket).catch_error(errno()) }),
        OP_WARN_RECV => {
            let origin: &CStr = if c.kind == ON_READ_HTTP {
                c"ListenPort::readable_callback_http"
            } else {
                c"ListenPort::readable_callback_raw"
            };
            // SAFETY: the live event.
            let fd = unsafe { (*c.event).fd };
            log::sys_warning(origin, format_args!("recv from connection#{fd} failed"));
            0
        }
        OP_CLOSE_ERRNO => {
            conn().close_errno = u16::try_from(errno()).unwrap_or(u16::MAX);
            0
        }
        OP_CLOSE => {
            // SAFETY: the live reactor and event.
            unsafe { trigger_close_event(c.reactor, c.event.cast()) };
            0
        }
        OP_SEND => send(
            c,
            match arg {
                SEND_BAD_REQUEST => SW_HTTP_BAD_REQUEST_PACKET,
                SEND_TOO_LARGE => SW_HTTP_REQUEST_ENTITY_TOO_LARGE_PACKET,
                SEND_UNAVAILABLE => SW_HTTP_SERVICE_UNAVAILABLE_PACKET,
                _ => SW_HTTP_100_CONTINUE_PACKET,
            }
            .as_bytes(),
        ),
        OP_RECV_EOF => receive(c, recv_with_eof_protocol, c.socket.cast()),
        OP_RECV_REDIS => receive(c, redis_recv_packet, c.conn.cast()),
        OP_DISPATCH_TASK | OP_DISPATCH_REQUEST => dispatch(c, op, arg),
        OP_WEBSOCKET_ACTIVATE => {
            let conn = conn();
            conn.websocket_status = STATUS_ACTIVE;
            conn.http_upgrade = 1;
            0
        }
        OP_HTTP2_ENABLED => u64::from(port().open_http2_protocol),
        OP_HTTP2_START => {
            conn().http2_stream = 1;
            // `http2::send_setting_frame`: the server's default settings.
            let settings = swoole_rs_http2::sw_rs_http2_default_settings();
            let mut frame = [0u8; SETTING_FRAME_SIZE];
            // SAFETY: a whole frame and initialized settings.
            let length = unsafe {
                swoole_rs_http2::pack_setting_frame(frame.as_mut_ptr(), &raw const settings, true)
            };
            send(c, frame.get(..length).unwrap_or_default());
            0
        }
        OP_SKIP_RECV => {
            // SAFETY: the live socket.
            unsafe { (*c.socket).skip_recv = 1 };
            0
        }
        OP_UPLOADS => u64::from(config(c).upload_max_filesize > 0),
        OP_STATIC => {
            let request = conn().object.cast();
            // SAFETY: the live server, the connection's request and the connection.
            u64::from(unsafe { swr_v1_server_static_select(c.serv, request, c.conn) })
        }
        OP_BUF_REDUCE => {
            buffer(c).reduce(arg.cast_signed());
            0
        }
        OP_BUF_EXTEND => {
            buffer(c).extend(usize::try_from(arg).unwrap_or(usize::MAX));
            0
        }
        OP_BUF_HEADER_END => {
            let b = buffer(c);
            // SAFETY: the buffer's `length` bytes.
            let bytes = unsafe { core::slice::from_raw_parts(b.str, b.length) };
            u64::from(bytes.windows(4).any(|w| w == b"\r\n\r\n"))
        }
        OP_REQ_MULTIPART_INIT => {
            let request = conn().object.cast::<HttpRequest>();
            // SAFETY: the live server.
            let dir = unsafe { view::upload_tmp_dir(c.serv) };
            let max = config(c).upload_max_filesize;
            // SAFETY: the connection's live request, with a form.
            unsafe { swoole_rs_http::init_multipart_parser(request, dir.as_bytes(), max) };
            // SAFETY: as above.
            c.shared.buffer = unsafe { (*request).buffer };
            0
        }
        OP_CLIENT => match arg {
            0 => status(conn().session_id),
            1 => port().host().data as u64,
            _ => status(port().port),
        },
        _ => std::process::abort(),
    }
}

/// `Server::dispatch_task` or `http_server::dispatch_request` of `length` bytes
/// from the buffer's start.
fn dispatch(c: &mut Read, op: u32, length: u64) -> u64 {
    let data = RecvData {
        info: DataHead {
            len: u32::try_from(length).unwrap_or(u32::MAX),
            ..DataHead::default()
        },
        data: buffer(c).str,
    };
    // SAFETY: the live port.
    let proto = unsafe { &raw const (*c.port).protocol }.cast();
    let rdata = (&raw const data).cast();
    if op == OP_DISPATCH_TASK {
        // SAFETY: the port's protocol, the live socket and its data.
        return status(unsafe { dispatch_task(proto, c.socket, rdata) });
    }
    // SAFETY: as above, on the live server.
    let result = unsafe { dispatch_request(c.serv, proto, c.socket, rdata) };
    // SAFETY: the connection, which dispatch may have given a new request.
    c.shared.request = unsafe { (*c.conn).object }.cast();
    status(result)
}
