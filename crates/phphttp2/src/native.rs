//! What `Coroutine\Http2\Client` needs below PHP: the coroutine socket,
//! through crates/phpruntime's [`co`] and its record.
use libc::{c_char, c_int, c_void, size_t, ssize_t};
use std::ptr;
use swoole_rs_phpruntime::co;
use swoole_rs_zend::{
    Value,
    sys::{zend_object, zend_string},
};

/// A `SocketImpl *`.
pub(crate) type Socket = co::Socket;

/// `php_swoole_create_socket(convert_to_type(host))`: the socket object, its
/// C++ socket and the host as `convert_to_type` rewrote it; null with `errno`
/// in `error`.
pub(crate) unsafe fn swoole_http2_client_coro_create_socket(
    host: *const c_char,
    length: size_t,
    converted: &mut *mut zend_string,
    socket: &mut Socket,
    error: &mut c_int,
) -> *mut zend_object {
    // SAFETY: per the caller, `length` readable bytes.
    let kind = unsafe { swoole_rs_reactor::swr_v1_socket_host_type(host.cast(), length) };
    // SAFETY: as above; `erase` never exceeds the length.
    let name = unsafe { std::slice::from_raw_parts(host.cast::<u8>(), length) };
    let name = swoole_rs_zend::string::copy(name.get(kind.erase..).unwrap_or_default());
    *converted = name;
    let object = swoole_rs_phpclients::create_socket(kind.socket_type);
    if object.is_null() {
        *error = std::io::Error::last_os_error().raw_os_error().unwrap_or(0);
        return ptr::null_mut();
    }
    // SAFETY: the new object is a socket object.
    let pointer = unsafe { swoole_rs_phpclients::socket_of(object) };
    *socket = pointer;
    object
}

unsafe extern "C" fn socket_closed(conn: *mut c_void, _: *mut c_void) {
    // SAFETY: the client's conn outlives its socket.
    unsafe { crate::client::socket_dtor(conn) };
}

/// The close hook (`socket_dtor(conn)`) and zero copy.
pub(crate) unsafe fn swoole_http2_client_coro_setup(socket: Socket, conn: *mut c_void) {
    // SAFETY: per the caller.
    unsafe { co::close_hook(socket, socket_closed, conn) };
    // SAFETY: as above.
    unsafe { co::record(socket) }.zero_copy = true;
}

pub(crate) unsafe fn swoole_http2_client_coro_enable_ssl(socket: Socket) -> bool {
    // SAFETY: per the caller.
    unsafe { co::enable_ssl(socket) }
}

/// HTTP/2 framing on the socket, then `apply_setting(settings)`.
pub(crate) unsafe fn swoole_http2_client_coro_protocol(socket: Socket, settings: &Value) {
    // SAFETY: per the caller.
    let record = unsafe { co::record(socket) };
    record.http2 = true;
    record.open_length_check = true;
    // SW_HTTP2_FRAME_HEADER_SIZE.
    record.protocol.package_length_size = 9;
    record.protocol.package_length_offset = 0;
    record.protocol.package_body_offset = 0;
    record.protocol.get_package_length =
        swoole_rs_ports::http_routes::exports::http2_length as *mut c_void;
    // SAFETY: as above.
    unsafe { swoole_http2_client_coro_apply(socket, settings) };
}

/// `php_swoole_socket_set` when `settings` is an array.
pub(crate) unsafe fn swoole_http2_client_coro_apply(socket: Socket, settings: &Value) {
    if settings.kind() == swoole_rs_zend::sys::IS_ARRAY {
        // SAFETY: a live socket and array; 0 applies every setting.
        unsafe { swoole_rs_phpclients::socket_settings(socket, settings.as_ptr(), 0) };
    }
}

pub(crate) unsafe fn swoole_http2_client_coro_connect(
    socket: Socket,
    host: *const c_char,
    length: size_t,
    port: c_int,
) -> bool {
    // SAFETY: per the caller.
    unsafe { co::connect(socket, host, length, port, 0) }
}

pub(crate) unsafe fn swoole_http2_client_coro_close(socket: Socket) -> bool {
    // SAFETY: per the caller.
    unsafe { co::close(socket) }
}

pub(crate) unsafe fn swoole_http2_client_coro_is_connected(socket: Socket) -> bool {
    // SAFETY: per the caller.
    unsafe { co::is_connected(socket) }
}

/// `has_bound(SW_EVENT_WRITE)`.
pub(crate) unsafe fn swoole_http2_client_coro_write_bound(socket: Socket) -> bool {
    // SAFETY: per the caller.
    unsafe { co::has_bound(socket, co::WRITE) }
}

pub(crate) unsafe fn swoole_http2_client_coro_send_all(
    socket: Socket,
    data: *const c_char,
    length: size_t,
) -> ssize_t {
    // SAFETY: per the caller.
    unsafe { co::send_all(socket, data, length) }
}

/// `errCode`, with `errMsg` in `message`.
pub(crate) unsafe fn swoole_http2_client_coro_error(
    socket: Socket,
    message: &mut *const c_char,
) -> c_int {
    // SAFETY: per the caller.
    let record = unsafe { co::record(socket) };
    *message = record.err_msg;
    record.err_code
}

pub(crate) unsafe fn swoole_http2_client_coro_set_error(
    socket: Socket,
    code: c_int,
    message: *const c_char,
) {
    // SAFETY: per the caller.
    let record = unsafe { co::record(socket) };
    record.err_code = code;
    record.err_msg = message;
}

pub(crate) unsafe fn swoole_http2_client_coro_recv_packet(socket: Socket, timeout: f64) -> ssize_t {
    // SAFETY: per the caller.
    unsafe { co::recv_packet(socket, timeout) }
}

/// The read buffer's bytes, after `recv_packet` returned a packet.
pub(crate) unsafe fn swoole_http2_client_coro_read_buffer(socket: Socket) -> *const c_char {
    // SAFETY: per the caller; a received packet left the read buffer allocated.
    let buffer = unsafe { co::record(socket) }
        .read_buffer
        .cast::<swoole_rs_buffer::string::CppString>();
    // SAFETY: as above.
    unsafe { (*buffer).as_ptr() }.cast()
}

/// `swoole_fatal_error(SW_ERROR_WRONG_OPERATION, "must call constructor first")`.
pub(crate) unsafe fn swoole_http2_client_coro_unconstructed() {
    // SAFETY: the frames it may longjmp across own nothing.
    unsafe {
        swoole_rs_corebase::fatal_error(
            swoole_rs_error::ErrorCode::SW_ERROR_WRONG_OPERATION as c_int,
            c"must call constructor first",
        );
    };
}

/// `swoole_set_last_error(code)`.
pub(crate) unsafe fn swoole_http2_client_coro_last_error(code: c_int) {
    swoole_rs_corebase::log::set_last_error(code);
}

/// `swoole_strerror(code)`.
pub(crate) unsafe fn swoole_http2_client_coro_swoole_strerror(code: c_int) -> *const c_char {
    swoole_rs_phpruntime::strerror(code).as_ptr()
}

/// Upstream's logs from `Client::parse_frame`: 0 unknown stream, else decompress failed.
pub(crate) unsafe fn swoole_http2_client_coro_log(what: c_int, stream_id: u32) {
    use swoole_rs_corebase::log;
    if what == 0 {
        // Upstream's "%d" of the unsigned stream id.
        #[expect(clippy::cast_possible_wrap, reason = "printf's %d of a uint32_t")]
        let id = stream_id as i32;
        log::notice(format_args!(
            "http2 stream#{id} belongs to an unknown type or it never registered"
        ));
    } else {
        log::warning(c"Client::parse_frame", format_args!("decompress failed"));
    }
}
