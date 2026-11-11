//! What `Coroutine\Http\Client` needs from the coroutine socket (methods on
//! the typed [`Co`] handle: its proxy, buffers and timeouts), files and
//! callables. WebSocket framing is crates/phphttp's, which registers it here
//! ([`WebSocket`]); the name resolver's context is still C++.
use crate::native::{Co, Socket};
use libc::{c_char, c_int, c_void, size_t};
use std::{ffi::CStr, sync::OnceLock};
use swoole_rs_buffer::string::{CppString, swr_string_drop};
use swoole_rs_corebase::{ProxyState, log};
use swoole_rs_hooks::hooked::{swoole_coroutine_close, swoole_coroutine_write};
use swoole_rs_hooks::socket_members::{get_read_buffer, get_write_buffer};
use swoole_rs_hooks::stdio::{
    swoole_coroutine_ftruncate, swoole_coroutine_lseek, swoole_coroutine_open,
};
use swoole_rs_phpruntime::{callable_call, callable_free, co};
use swoole_rs_websocket_ffi::frame_api::get_package_length;
use swoole_rs_zend::{
    Array, Call, Value, bailout, set_object, string,
    sys::{zend_object, zval, zval_ptr_dtor},
    value,
};

/// `Coroutine\Http\Client`'s WebSocket state: its `WebSocketSettings` and
/// the message it is assembling. Framing is crates/phphttp's, which depends
/// on this crate, so it registers the state's constructor at MINIT
/// ([`register_websocket`]).
pub trait WebSocket {
    /// `WebSocket::apply_setting(settings, vht, false)`.
    fn apply(&mut self, settings: &Array);
    /// Whether the client offers `permessage-deflate`.
    fn compression(&self) -> bool;
    /// Pack a frame of `data` and send it on `socket`: 0, 1 if packing failed
    /// (a bailout may be pending, for the handler to raise), 2 if sending did.
    fn push(
        &self,
        socket: Co,
        data: &Value,
        opcode: i64,
        flags: c_int,
        code: i64,
        compress: bool,
    ) -> c_int;
    /// `websocket::recv_frame` on `socket` into the call's return slot; a
    /// bailout it defers is the handler's to raise.
    fn recv(&mut self, socket: Co, call: &Call, timeout: f64);
}

static WEBSOCKET: OnceLock<fn() -> Box<dyn WebSocket>> = OnceLock::new();

/// crates/phphttp's [`WebSocket`] constructor, registered at MINIT; the first
/// registration stands.
pub fn register_websocket(new: fn() -> Box<dyn WebSocket>) {
    WEBSOCKET.get_or_init(|| new);
}

/// A new client's WebSocket state (`WebSocketSettings`' defaults), once
/// crates/phphttp has registered it.
pub(crate) fn websocket() -> Option<Box<dyn WebSocket>> {
    WEBSOCKET.get().map(|new| new())
}

unsafe extern "C" {
    #[link_name = "_ZdlPv"]
    fn operator_delete(storage: *mut c_void);
}

/// `SW_TIMEOUT_CONNECT` and `SW_TIMEOUT_READ`.
const TIMEOUT_CONNECT: c_int = 1 << 1;
pub(crate) const TIMEOUT_READ: c_int = 1 << 2;
/// `SW_WEBSOCKET_HEADER_LEN`.
const WEBSOCKET_HEADER_LEN: u8 = 2;

/// `php_swoole_check_reactor()`, then a `Coroutine\Socket` of `kind`, or
/// `errno`.
pub(crate) fn swoole_http_client_coro_create_socket(kind: i64) -> Result<*mut zend_object, c_int> {
    swoole_rs_phpruntime::check_reactor();
    #[expect(clippy::cast_possible_truncation, reason = "a swSocketType")]
    let object = crate::socket::create_socket(kind as i32);
    if object.is_null() {
        return Err(std::io::Error::last_os_error().raw_os_error().unwrap_or(0));
    }
    Ok(object)
}

/// The close hook: hands the popped write buffer to the client's conn.
unsafe extern "C" fn socket_dtor(conn: *mut c_void, socket: *mut c_void) {
    // SAFETY: the socket being destroyed.
    let record = unsafe { co::record(socket) };
    let written = std::mem::replace(&mut record.write_buffer, std::ptr::null_mut());
    // SAFETY: the conn the client registered, and the buffer it takes over.
    unsafe { crate::http_client::socket_closed(conn, written) };
}

/// `delete buffer`.
pub(crate) unsafe fn swoole_http_client_coro_buffer_free(buffer: *mut c_void) {
    if buffer.is_null() {
        return;
    }
    // SAFETY: a popped `new String`, destroyed once.
    swr_string_drop(unsafe { &mut *buffer.cast() });
    // SAFETY: its storage, from `operator new`.
    unsafe { operator_delete(buffer) };
}

/// The socket frames WebSocket packets from now on.
/// # Safety
/// `socket` is a live coroutine socket.
pub unsafe fn websocket_protocol(socket: Socket) {
    // SAFETY: per the caller.
    unsafe { Co::new(socket) }.websocket_protocol();
}

/// What the HTTP client asks of its socket.
impl Co {
    /// The HTTP proxy's state, unless TLS tunnels through it.
    fn plain_proxy<'a>(self) -> Option<&'a ProxyState> {
        if !self.ssl_context().is_null() {
            return None;
        }
        // SAFETY: a live socket.
        let proxy = unsafe { co::http_proxy(self.as_ptr()) }.cast::<ProxyState>();
        // SAFETY: the socket owns its proxy state, if any.
        unsafe { proxy.as_ref() }
    }

    /// After the settings: no proxy handshake for plain HTTP proxying.
    pub(crate) fn skip_proxy_handshake(self) {
        if let Some(proxy) = self.plain_proxy() {
            // SAFETY: the socket's proxy fields, not otherwise borrowed.
            unsafe { (*proxy.fields.get()).dont_handshake = 1 };
        }
    }

    /// Whether requests go through a plain HTTP proxy.
    pub(crate) fn proxied(self) -> bool {
        self.plain_proxy().is_some()
    }

    /// `"Basic " + http_proxy->get_auth_str()`, when a plain HTTP proxy has a
    /// password.
    pub(crate) fn proxy_authorization(self) -> Option<Vec<u8>> {
        let proxy = self.plain_proxy()?;
        // SAFETY: the proxy's password slot, unchanged meanwhile.
        if unsafe { proxy.string(2) }.length == 0 {
            return None;
        }
        let mut encoded = [0u8; 512];
        let mut length = 0;
        // SAFETY: the live proxy, 512 writable bytes and a length slot.
        let result = unsafe {
            swoole_rs_http_ffi::swr_v1_http_proxy_owned_authorization(
                proxy,
                encoded.as_mut_ptr(),
                Some(&mut length),
            )
        };
        let mut value = b"Basic ".to_vec();
        if result >= 0 {
            // `std::string(encode_buf)`: up to the first NUL.
            let encoded = encoded.get(..length).unwrap_or(&encoded);
            value.extend_from_slice(encoded.split(|&byte| byte == 0).next().unwrap_or_default());
        }
        Some(value)
    }

    /// Connect timeout (0: the default), resolve context, the close hook and
    /// the zend string allocator.
    /// # Safety
    /// `resolve` and `conn` outlive the socket.
    pub(crate) unsafe fn configure_http(
        self,
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
        self.set_timeout(timeout, TIMEOUT_CONNECT);
        let record = self.record();
        record.resolve_context = resolve;
        record.buffer_allocator = swoole_rs_phpswoole::sw_zend_string_allocator();
        // SAFETY: per the caller, the conn outlives the socket.
        unsafe { self.close_hook(socket_dtor, conn) };
    }

    /// `connect(host, port)` under the timeout `configure_http` set.
    pub(crate) fn connect_http(self, host: &[u8], port: c_int) -> bool {
        // SAFETY: a live socket and the host's bytes.
        unsafe { co::connect(self.as_ptr(), host.as_ptr().cast(), host.len(), port, 0) }
    }

    /// `check_bound_co(event)`: a fatal error if another coroutine holds it.
    #[expect(clippy::cast_sign_loss, reason = "an event bit")]
    pub(crate) fn check_bound(self, event: c_int) {
        // SAFETY: a live socket.
        unsafe { co::check_bound(self.as_ptr(), event as u32) };
    }

    /// The write buffer becomes `data`, then `send_all` of it: whether all went.
    pub(crate) fn send_buffered(self, data: &[u8]) -> bool {
        let buffer = self.write_buffer();
        buffer.length = 0;
        buffer.offset = 0;
        // SAFETY: the caller's bytes.
        unsafe { buffer.append(data.as_ptr(), data.len()) };
        let (data, length) = (buffer.str, buffer.length);
        // SAFETY: the buffer's bytes, which the send only reads.
        unsafe { co::send_all(self.as_ptr(), data.cast(), length) }.cast_unsigned() == length
    }

    /// `send_all(data)`: whether all went.
    pub(crate) fn send_all(self, data: &[u8]) -> bool {
        // SAFETY: a live socket and the caller's bytes.
        unsafe { co::send_all(self.as_ptr(), data.as_ptr().cast(), data.len()) }.cast_unsigned()
            == data.len()
    }

    /// `get_write_buffer()`, made on first use.
    pub(crate) fn write_buffer<'a>(self) -> &'a mut CppString {
        // SAFETY: a live socket.
        let buffer = unsafe { get_write_buffer(self.as_ptr()) }.cast::<CppString>();
        // SAFETY: its write buffer, which lives as long.
        unsafe { &mut *buffer }
    }

    /// `get_read_buffer()`, made on first use.
    pub(crate) fn input<'a>(self) -> &'a mut CppString {
        // SAFETY: a live socket.
        let buffer = unsafe { get_read_buffer(self.as_ptr()) }.cast::<CppString>();
        // SAFETY: its read buffer, which lives as long.
        unsafe { &mut *buffer }
    }

    /// `recv` into the read buffer's room past its length.
    pub(crate) fn recv_input(self) -> isize {
        let buffer = self.input();
        let at = buffer.str.wrapping_add(buffer.length);
        let room = buffer.size - buffer.length;
        // SAFETY: a live socket and the buffer's `room` writable bytes at `at`.
        unsafe { co::recv(self.as_ptr(), at.cast(), room) }
    }

    /// A read `TimeoutController` (0: the default read timeout).
    pub(crate) fn read_timeout(self, timeout: f64) -> co::Timeout {
        let timeout = if timeout == 0.0 {
            // SAFETY: the reactor's defaults, written only by settings.
            unsafe { swoole_rs_reactor::swr_v1_socket_defaults.timeouts.read }
        } else {
            timeout
        };
        self.timeout(timeout, TIMEOUT_READ)
    }

    /// The socket frames WebSocket packets from now on.
    pub(crate) fn websocket_protocol(self) {
        let record = self.record();
        record.open_length_check = true;
        record.protocol.package_length_size = WEBSOCKET_HEADER_LEN;
        record.protocol.package_length_offset = 0;
        record.protocol.package_body_offset = 0;
        record.protocol.get_package_length = get_package_length as *mut c_void;
    }

    /// `ssl_get_peer_cert()`: empty, with the socket's error set, without one.
    pub(crate) fn peer_cert_pem(self) -> Vec<u8> {
        // SAFETY: a live socket.
        unsafe { co::peer_cert(self.as_ptr()) }
    }
}

/// An `AsyncFile` for a download, truncated or positioned: its descriptor,
/// or `None` after upstream's warning.
pub(crate) fn swoole_http_client_coro_file_open(name: &CStr, offset: i64) -> Option<c_int> {
    let path = name.to_string_lossy();
    // SAFETY: a NUL-terminated path.
    let fd = unsafe { swoole_coroutine_open(name.as_ptr(), libc::O_CREAT | libc::O_WRONLY, 0o664) };
    let origin = c"http_parser_on_body";
    let failed = if fd == -1 {
        log::sys_warning(
            origin,
            format_args!("open({path}, O_CREAT | O_WRONLY) failed"),
        );
        return None;
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
        swoole_http_client_coro_file_close(fd);
        return None;
    }
    Some(fd)
}

/// `AsyncFile::write()`: whether all of `data` was written.
pub(crate) fn swoole_http_client_coro_file_write(fd: c_int, data: &[u8]) -> bool {
    // SAFETY: a descriptor and the caller's bytes.
    unsafe { swoole_coroutine_write(fd, data.as_ptr().cast(), data.len()) }.cast_unsigned()
        == data.len()
}

/// `AsyncFile::close()`. It can yield, so a caller lets go of the descriptor
/// (and anything that refers to it) before calling it.
pub(crate) fn swoole_http_client_coro_file_close(fd: c_int) {
    // A plain close of a descriptor nothing else holds.
    swoole_coroutine_close(fd);
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
    let result = match unsafe { callable_call(callable, &mut args, std::ptr::null_mut(), false) } {
        Ok(true) => 0,
        Ok(false) => 1,
        Err(bailout::Bailout) => 2,
    };
    // SAFETY: the string argument's reference.
    unsafe { zval_ptr_dtor(&raw mut args[1]) };
    result
}

/// `mime_type::get(path)`.
pub(crate) unsafe fn swoole_http_client_coro_mime(path: *const c_char) -> *const c_char {
    // SAFETY: the caller's NUL-terminated path.
    swoole_rs_phpswoole::mime_type(unsafe { CStr::from_ptr(path) }.to_bytes()).as_ptr()
}

/// Upstream's `decompress_response` warnings.
pub(crate) fn swoole_http_client_coro_inflate_warning(message: &str) {
    log::warning(c"Client::decompress_response", format_args!("{message}"));
}
