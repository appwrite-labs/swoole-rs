//! What `Coroutine\Http\Client` needs from the coroutine socket, its buffers
//! and timeouts, files and callables, over `swoole_rs_phpruntime::co` and the
//! socket's record. WebSocket framing is crates/phphttp's, linked by symbol;
//! the name resolver's context is still C++.
use crate::native::Socket;
use libc::{c_char, c_int, c_void, mode_t, off_t, size_t, ssize_t};
use std::ffi::CStr;
use swoole_rs_buffer::string::{CppString, swr_string_drop};
use swoole_rs_corebase::{ProxyState, log};
use swoole_rs_phpruntime::{callable_call, callable_free, co};
use swoole_rs_zend::{
    bailout, set_object, string,
    sys::{zend_object, zend_string, zval, zval_ptr_dtor},
    value,
};

unsafe extern "C" {
    /// WebSocket settings and continuation buffer (crates/phphttp).
    pub(crate) fn swoole_http_client_coro_ws_new() -> *mut c_void;
    pub(crate) fn swoole_http_client_coro_ws_free(ws: *mut c_void);
    pub(crate) fn swoole_http_client_coro_ws_apply(ws: *mut c_void, settings: *const zval);
    pub(crate) fn swoole_http_client_coro_ws_compression(ws: *mut c_void) -> bool;
    /// Pack a frame and send it: 0, 1 if packing failed, 2 if sending did.
    pub(crate) fn swoole_http_client_coro_ws_push(
        socket: Socket,
        ws: *mut c_void,
        data: *mut zval,
        opcode: i64,
        flags: c_int,
        code: i64,
        compress: bool,
    ) -> c_int;
    pub(crate) fn swoole_http_client_coro_ws_recv(
        socket: Socket,
        ws: *mut c_void,
        return_value: *mut zval,
        timeout: f64,
    );

    #[link_name = "_ZN6swoole9coroutine6Socket15get_read_bufferEv"]
    fn get_read_buffer(socket: Socket) -> *mut CppString;
    #[link_name = "_ZN6swoole9coroutine6Socket16get_write_bufferEv"]
    fn get_write_buffer(socket: Socket) -> *mut CppString;
    #[link_name = "_ZN6swoole9websocket18get_package_lengthEPKNS_8ProtocolEPNS_7network6SocketEPNS_12PacketLengthE"]
    fn websocket_package_length();
    #[link_name = "_ZdlPv"]
    fn operator_delete(storage: *mut c_void);
    /// crates/http's `get_auth_str` codec, into 512 bytes.
    fn swr_v1_http_proxy_owned_authorization(
        owner: *const c_void,
        output: *mut u8,
        written: *mut usize,
    ) -> i32;
    fn swoole_coroutine_open(path: *const c_char, flags: c_int, mode: mode_t) -> c_int;
    fn swoole_coroutine_write(fd: c_int, data: *const c_void, length: size_t) -> ssize_t;
    fn swoole_coroutine_ftruncate(fd: c_int, length: off_t) -> c_int;
    fn swoole_coroutine_lseek(fd: c_int, offset: off_t, whence: c_int) -> off_t;
    fn swoole_coroutine_close(fd: c_int) -> c_int;
}

/// `SW_TIMEOUT_CONNECT` and `SW_TIMEOUT_READ`.
const TIMEOUT_CONNECT: c_int = 1 << 1;
const TIMEOUT_READ: c_int = 1 << 2;
/// `SW_WEBSOCKET_HEADER_LEN`.
const WEBSOCKET_HEADER_LEN: u8 = 2;

/// `php_swoole_check_reactor()`, then a `Coroutine\Socket` of `kind`; null
/// with `errno` in `error`.
pub(crate) unsafe fn swoole_http_client_coro_create_socket(
    kind: i64,
    error: *mut c_int,
) -> *mut zend_object {
    swoole_rs_phpruntime::check_reactor();
    #[expect(clippy::cast_possible_truncation, reason = "a swSocketType")]
    // SAFETY: PHP is running.
    let object = unsafe { crate::socket::php_swoole_create_socket(kind as i32) };
    if object.is_null() {
        // SAFETY: the caller's slot.
        unsafe { *error = std::io::Error::last_os_error().raw_os_error().unwrap_or(0) };
    }
    object
}

/// The HTTP proxy's state, unless TLS tunnels through it.
fn plain_proxy<'a>(socket: Socket) -> Option<&'a ProxyState> {
    // SAFETY: the caller's live socket.
    let proxy = unsafe { co::http_proxy(socket) }.cast::<ProxyState>();
    // SAFETY: as above.
    if proxy.is_null() || unsafe { co::ssl_enabled(socket) } {
        return None;
    }
    // SAFETY: the socket owns its proxy state.
    Some(unsafe { &*proxy })
}

/// After the settings: no proxy handshake for plain HTTP proxying.
pub(crate) unsafe fn swoole_http_client_coro_apply(socket: Socket) {
    if let Some(proxy) = plain_proxy(socket) {
        // SAFETY: the socket's proxy fields, not otherwise borrowed.
        unsafe { (*proxy.fields.get()).dont_handshake = 1 };
    }
}

/// The close hook: hands the popped write buffer to the client's conn.
unsafe extern "C" fn socket_dtor(conn: *mut c_void, socket: *mut c_void) {
    // SAFETY: the socket being destroyed.
    let record = unsafe { co::record(socket) };
    let written = std::mem::replace(&mut record.write_buffer, std::ptr::null_mut());
    // SAFETY: the conn the client registered, and the buffer it takes over.
    unsafe { crate::http_client::swr_v1_phpclients_http_client_socket_dtor(conn, written) };
}

/// Connect timeout (0: the default), resolve context, the close hook and the
/// zend string allocator.
pub(crate) unsafe fn swoole_http_client_coro_configure(
    socket: Socket,
    timeout: f64,
    resolve: *mut c_void,
    conn: *mut c_void,
) {
    let timeout = if timeout == 0.0 {
        // SAFETY: the reactor's defaults, written only by settings.
        unsafe { swoole_rs_reactor::swr_v1_socket_defaults.timeouts.connect }
    } else {
        timeout
    };
    // SAFETY: the caller's live socket.
    unsafe { co::set_timeout(socket, timeout, TIMEOUT_CONNECT) };
    // SAFETY: as above.
    let record = unsafe { co::record(socket) };
    record.resolve_context = resolve;
    record.buffer_allocator = swoole_rs_phpswoole::sw_zend_string_allocator();
    // SAFETY: the conn outlives the socket.
    unsafe { co::close_hook(socket, socket_dtor, conn) };
}

pub(crate) unsafe fn swoole_http_client_coro_connect(
    socket: Socket,
    host: *const c_char,
    length: size_t,
    port: c_int,
) -> bool {
    // SAFETY: the caller's live socket and host bytes.
    unsafe { co::connect(socket, host, length, port, 0) }
}

/// `check_bound_co(event)`: a fatal error if another coroutine holds it.
#[expect(clippy::cast_sign_loss, reason = "an event bit")]
pub(crate) unsafe fn swoole_http_client_coro_check_bound(socket: Socket, event: c_int) {
    // SAFETY: the caller's live socket.
    unsafe { co::check_bound(socket, event as u32) };
}

/// 0 without a plain HTTP proxy, 1 with one, 2 with one that authenticates.
pub(crate) unsafe fn swoole_http_client_coro_proxy(socket: Socket) -> c_int {
    plain_proxy(socket).map_or(0, |proxy| {
        // SAFETY: the proxy's password slot, unchanged meanwhile.
        if unsafe { proxy.string(2) }.length == 0 {
            1
        } else {
            2
        }
    })
}

/// `"Basic " + http_proxy->get_auth_str()`.
pub(crate) unsafe fn swoole_http_client_coro_proxy_auth(socket: Socket) -> *mut zend_string {
    // SAFETY: the caller's live socket, which has a proxy.
    let proxy = unsafe { co::http_proxy(socket) }.cast::<ProxyState>();
    let mut encoded = [0u8; 512];
    let mut length = 0;
    // SAFETY: the live proxy, 512 writable bytes and a length slot.
    let result = unsafe {
        swr_v1_http_proxy_owned_authorization(proxy.cast(), encoded.as_mut_ptr(), &raw mut length)
    };
    let mut value = b"Basic ".to_vec();
    if result >= 0 {
        // `std::string(encode_buf)`: up to the first NUL.
        let encoded = encoded.get(..length).unwrap_or(&encoded);
        value.extend_from_slice(encoded.split(|&byte| byte == 0).next().unwrap_or_default());
    }
    string::copy(&value)
}

/// The write buffer becomes `data`, then `send_all` of it: whether all went.
pub(crate) unsafe fn swoole_http_client_coro_send(
    socket: Socket,
    data: *const c_char,
    length: size_t,
) -> bool {
    // SAFETY: the caller's live socket.
    let buffer = unsafe { get_write_buffer(socket) };
    // SAFETY: its write buffer.
    let buffer = unsafe { &mut *buffer };
    buffer.length = 0;
    buffer.offset = 0;
    // SAFETY: the caller's `length` bytes.
    unsafe { buffer.append(data.cast(), length) };
    let (data, length) = (buffer.str, buffer.length);
    // SAFETY: the buffer's bytes, which the send only reads.
    unsafe { co::send_all(socket, data.cast(), length) }.cast_unsigned() == length
}

/// `send_all(data)`: whether all went.
pub(crate) unsafe fn swoole_http_client_coro_send_raw(
    socket: Socket,
    data: *const c_char,
    length: size_t,
) -> bool {
    // SAFETY: the caller's live socket and bytes.
    unsafe { co::send_all(socket, data, length) }.cast_unsigned() == length
}

/// The socket's write buffer, or with no socket `saved` (a popped one).
pub(crate) unsafe fn swoole_http_client_coro_written(
    socket: Socket,
    saved: *mut c_void,
    length: *mut size_t,
) -> *const c_char {
    let buffer = if socket.is_null() {
        saved.cast::<CppString>()
    } else {
        // SAFETY: the caller's live socket.
        unsafe { get_write_buffer(socket) }
    };
    if buffer.is_null() {
        return std::ptr::null();
    }
    // SAFETY: a live buffer.
    let buffer = unsafe { &*buffer };
    // SAFETY: the caller's slot.
    unsafe { *length = buffer.length };
    buffer.str.cast()
}

/// `delete buffer`.
pub(crate) unsafe fn swoole_http_client_coro_buffer_free(buffer: *mut c_void) {
    if buffer.is_null() {
        return;
    }
    // SAFETY: a popped `new String`, destroyed once.
    unsafe { swr_string_drop(buffer.cast()) };
    // SAFETY: its storage, from `operator new`.
    unsafe { operator_delete(buffer) };
}

fn read_buffer<'a>(socket: Socket) -> &'a mut CppString {
    // SAFETY: every caller passes a live socket.
    let buffer = unsafe { get_read_buffer(socket) };
    // SAFETY: its read buffer.
    unsafe { &mut *buffer }
}

/// The read buffer's storage, length and size.
pub(crate) unsafe fn swoole_http_client_coro_read_buffer(
    socket: Socket,
    length: *mut size_t,
    size: *mut size_t,
) -> *mut c_char {
    let buffer = read_buffer(socket);
    // SAFETY: the caller's slots.
    unsafe { *length = buffer.length };
    // SAFETY: as above.
    unsafe { *size = buffer.size };
    buffer.str.cast()
}

pub(crate) unsafe fn swoole_http_client_coro_read_buffer_set(socket: Socket, length: size_t) {
    let buffer = read_buffer(socket);
    buffer.length = length;
    buffer.offset = 0;
}

/// Keep the read buffer's bytes past `offset` (`reduce`).
#[expect(clippy::cast_possible_wrap, reason = "a buffer offset")]
pub(crate) unsafe fn swoole_http_client_coro_read_buffer_keep(
    socket: Socket,
    length: size_t,
    offset: size_t,
) {
    let buffer = read_buffer(socket);
    buffer.length = length;
    buffer.offset = offset as i64;
    buffer.reduce(offset as i64);
}

/// A read `TimeoutController` (0: the default read timeout).
pub(crate) unsafe fn swoole_http_client_coro_timeout(socket: Socket, timeout: f64) -> *mut c_void {
    let timeout = if timeout == 0.0 {
        // SAFETY: the reactor's defaults, written only by settings.
        unsafe { swoole_rs_reactor::swr_v1_socket_defaults.timeouts.read }
    } else {
        timeout
    };
    // SAFETY: the caller's live socket, which outlives the controller.
    let controller = unsafe { co::Timeout::set(socket, timeout, TIMEOUT_READ) };
    Box::into_raw(Box::new(controller)).cast()
}

pub(crate) unsafe fn swoole_http_client_coro_timed_out(controller: *mut c_void) -> bool {
    // SAFETY: a controller `swoole_http_client_coro_timeout` made.
    unsafe { &mut *controller.cast::<co::Timeout>() }.timed_out(TIMEOUT_READ)
}

pub(crate) unsafe fn swoole_http_client_coro_timeout_end(controller: *mut c_void) {
    // SAFETY: as above, ended once.
    drop(unsafe { Box::from_raw(controller.cast::<co::Timeout>()) });
}

/// The socket frames WebSocket packets from now on.
/// # Safety
/// `socket` is a live coroutine socket.
pub unsafe fn websocket_protocol(socket: Socket) {
    // SAFETY: the caller's live socket.
    let record = unsafe { co::record(socket) };
    record.open_length_check = true;
    record.protocol.package_length_size = WEBSOCKET_HEADER_LEN;
    record.protocol.package_length_offset = 0;
    record.protocol.package_body_offset = 0;
    record.protocol.get_package_length = websocket_package_length as *mut c_void;
}

/// An `AsyncFile` for a download, truncated or positioned; null after
/// upstream's warning.
pub(crate) unsafe fn swoole_http_client_coro_file_open(
    name: *const c_char,
    offset: i64,
) -> *mut c_void {
    // SAFETY: the caller's NUL-terminated path.
    let path = unsafe { CStr::from_ptr(name) }.to_string_lossy();
    // SAFETY: as above.
    let fd = unsafe { swoole_coroutine_open(name, libc::O_CREAT | libc::O_WRONLY, 0o664) };
    let origin = c"http_parser_on_body";
    let failed = if fd == -1 {
        log::sys_warning(
            origin,
            format_args!("open({path}, O_CREAT | O_WRONLY) failed"),
        );
        true
    // SAFETY: the open descriptor.
    } else if offset == 0 && unsafe { swoole_coroutine_ftruncate(fd, 0) } != 0 {
        log::sys_warning(origin, format_args!("ftruncate({path}) failed"));
        true
    // SAFETY: as above; upstream fails only on a zero result.
    } else if offset != 0 && unsafe { swoole_coroutine_lseek(fd, offset, libc::SEEK_SET) } == 0 {
        log::sys_warning(origin, format_args!("fseek({path}, {offset}) failed"));
        true
    } else {
        false
    };
    if failed {
        // SAFETY: as above.
        unsafe { swoole_rs_file_close(fd) };
        return std::ptr::null_mut();
    }
    Box::into_raw(Box::new(fd)).cast()
}

/// `AsyncFile::close()`.
unsafe fn swoole_rs_file_close(fd: c_int) {
    if fd != -1 {
        // SAFETY: an owned descriptor, closed once.
        unsafe { swoole_coroutine_close(fd) };
    }
}

pub(crate) unsafe fn swoole_http_client_coro_file_write(
    file: *mut c_void,
    data: *const c_char,
    length: size_t,
) -> bool {
    // SAFETY: an open file.
    let fd = unsafe { *file.cast::<c_int>() };
    // SAFETY: its descriptor and the caller's bytes.
    unsafe { swoole_coroutine_write(fd, data.cast(), length) }.cast_unsigned() == length
}

pub(crate) unsafe fn swoole_http_client_coro_file_free(file: *mut c_void) {
    if file.is_null() {
        return;
    }
    // SAFETY: a file `swoole_http_client_coro_file_open` made, freed once.
    let fd = *unsafe { Box::from_raw(file.cast::<c_int>()) };
    // SAFETY: its descriptor.
    unsafe { swoole_rs_file_close(fd) };
}

pub(crate) unsafe fn swoole_http_client_coro_callable_free(callable: *mut c_void) {
    if !callable.is_null() {
        // SAFETY: the client's callable, freed once.
        unsafe { callable_free(callable) };
    }
}

/// `callable($client, $data)` inside `zend_try`: 0, 1 if the call failed,
/// 2 if it bailed out.
pub(crate) unsafe fn swoole_http_client_coro_call(
    callable: *mut c_void,
    client: *mut zend_object,
    data: *const c_char,
    length: size_t,
) -> c_int {
    // SAFETY: zero is an undefined zval.
    let mut args: [zval; 2] = unsafe { std::mem::zeroed() };
    // SAFETY: the client object, borrowed for the call as upstream's `ZVAL_OBJ`.
    unsafe { set_object(&raw mut args[0], client) };
    // SAFETY: the caller's bytes.
    let data = unsafe { std::slice::from_raw_parts(data.cast::<u8>(), length) };
    // SAFETY: a fresh string the zval owns.
    unsafe { value::set_str(&raw mut args[1], string::copy(data)) };
    // SAFETY: a live callable and arguments.
    let result = match unsafe { callable_call(callable, &mut args) } {
        Ok(true) => 0,
        Ok(false) => 1,
        Err(bailout::Bailout) => 2,
    };
    // SAFETY: the string argument's reference.
    unsafe { zval_ptr_dtor(&raw mut args[1]) };
    result
}

pub(crate) unsafe fn swoole_http_client_coro_peer_cert(socket: Socket) -> *mut zend_string {
    // SAFETY: the caller's live socket.
    let cert = unsafe { co::peer_cert(socket) };
    if cert.is_empty() {
        std::ptr::null_mut()
    } else {
        string::copy(&cert)
    }
}

/// `mime_type::get(path)`.
pub(crate) unsafe fn swoole_http_client_coro_mime(path: *const c_char) -> *const c_char {
    // SAFETY: the caller's NUL-terminated path.
    swoole_rs_phpswoole::mime_type(unsafe { CStr::from_ptr(path) }.to_bytes()).as_ptr()
}

/// Upstream's `decompress_response` warnings: unknown `method`, or failed (-1).
pub(crate) unsafe fn swoole_http_client_coro_inflate_warning(method: c_int) {
    let origin = c"Client::decompress_response";
    if method < 0 {
        log::warning(
            origin,
            format_args!("HttpClient::decompress_response failed"),
        );
    } else {
        log::warning(
            origin,
            format_args!("HttpClient::decompress_response unknown compress method [{method}]"),
        );
    }
}
