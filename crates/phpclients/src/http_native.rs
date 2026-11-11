//! What `Coroutine\Http\Client` needs from the coroutine socket (methods on
//! the typed [`Co`] handle: its proxy, buffers and timeouts), files and
//! callables. WebSocket framing is crates/phphttp's, which registers it here
//! ([`WebSocket`]); the name resolver's context is still C++.
use crate::native::{Co, Socket};
use libc::{c_char, c_int, c_void, size_t};
use std::{ffi::CStr, sync::OnceLock};
use swoole_rs_buffer::string::{CppString, delete_string};
use swoole_rs_corebase::{ProxyState, log};
use swoole_rs_hooks::hooked::{swoole_coroutine_close, write};
use swoole_rs_hooks::socket_extra::{CloseHook, set_close_hook};
use swoole_rs_hooks::stdio::{
    swoole_coroutine_ftruncate, swoole_coroutine_lseek, swoole_coroutine_open,
};
use swoole_rs_phpruntime::co;
use swoole_rs_websocket_ffi::frame_api::get_package_length;
use swoole_rs_zend::host::callable::Callable;
use swoole_rs_zend::{
    Array, Call, Owned, UNDEF, Value, ZString, bailout, set_object,
    sys::{zend_object, zval_ptr_dtor},
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

/// `SW_TIMEOUT_CONNECT` and `SW_TIMEOUT_READ`.
const TIMEOUT_CONNECT: c_int = 1 << 1;
pub(crate) const TIMEOUT_READ: c_int = 1 << 2;
/// `SW_WEBSOCKET_HEADER_LEN`.
const WEBSOCKET_HEADER_LEN: u8 = 2;

/// `php_swoole_check_reactor()`, then a `Coroutine\Socket` of `kind`, or
/// `errno`.
pub(crate) fn swoole_http_client_coro_create_socket(kind: i64) -> Result<Owned, c_int> {
    swoole_rs_phpruntime::check_reactor();
    #[expect(clippy::cast_possible_truncation, reason = "a swSocketType")]
    match crate::socket::owner::create_kind(kind as i32) {
        Ok(Some(object)) => Ok(object),
        Ok(None) => Err(std::io::Error::last_os_error().raw_os_error().unwrap_or(0)),
        Err(bailout) => {
            bailout.defer();
            Err(0)
        }
    }
}

/// `delete buffer`.
pub(crate) unsafe fn swoole_http_client_coro_buffer_free(buffer: *mut c_void) {
    if buffer.is_null() {
        return;
    }
    // SAFETY: a popped `new String`, deleted once.
    unsafe { delete_string(buffer.cast()) };
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
        // SAFETY: the socket owns its proxy state, if any.
        unsafe { self.handle().http_proxy().as_ref() }
    }

    /// After the settings: no proxy handshake for plain HTTP proxying.
    pub(crate) fn skip_proxy_handshake(self) {
        if let Some(proxy) = self.plain_proxy() {
            proxy.update(|fields| fields.dont_handshake = 1);
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
        if proxy.string(2).is_empty() {
            return None;
        }
        let mut value = b"Basic ".to_vec();
        value.extend(swoole_rs_http_ffi::authorization(proxy));
        Some(value)
    }

    /// Connect timeout (0: the default), resolve context, the close hook and
    /// the zend string allocator.
    /// # Safety
    /// `resolve` outlives the socket.
    pub(crate) unsafe fn configure_http(self, timeout: f64, resolve: *mut c_void, hook: CloseHook) {
        let timeout = if timeout == 0.0 {
            // SAFETY: the reactor's defaults, written only by settings.
            unsafe { swoole_rs_reactor::swr_v1_socket_defaults.timeouts.connect }
        } else {
            timeout
        };
        self.set_timeout(timeout, TIMEOUT_CONNECT);
        // SAFETY: the caller's context, which outlives the socket.
        unsafe { self.handle().set_resolve_context(resolve) };
        self.use_zend_strings();
        set_close_hook(self.handle(), hook);
    }

    /// `connect(host, port)` under the timeout `configure_http` set.
    pub(crate) fn connect_http(self, host: &[u8], port: c_int) -> bool {
        self.handle().connect_host(host, port)
    }

    /// `check_bound_co(event)`: a fatal error if another coroutine holds it.
    #[expect(clippy::cast_sign_loss, reason = "an event bit")]
    pub(crate) fn check_bound(self, event: c_int) {
        self.handle().check_bound(event as u32);
    }

    /// The write buffer becomes `data`, then `send_all` of it: whether all went.
    pub(crate) fn send_buffered(self, data: &[u8]) -> bool {
        let buffer = self.write_buffer();
        buffer.clear();
        buffer.append(data);
        self.send_all(buffer.view())
    }

    /// `send_all(data)`: whether all went.
    pub(crate) fn send_all(self, data: &[u8]) -> bool {
        self.handle().send_all(data).cast_unsigned() == data.len()
    }

    /// `get_write_buffer()`, made on first use.
    pub(crate) fn write_buffer<'a>(self) -> &'a mut CppString {
        // SAFETY: its write buffer, which lives as long.
        unsafe { &mut *self.handle().buffer(true) }
    }

    /// `get_read_buffer()`, made on first use.
    pub(crate) fn input<'a>(self) -> &'a mut CppString {
        // SAFETY: its read buffer, which lives as long.
        unsafe { &mut *self.handle().buffer(false) }
    }

    /// `recv` into the read buffer's room past its length.
    pub(crate) fn recv_input(self) -> isize {
        let buffer = self.input();
        let room = buffer.size() - buffer.len();
        let at = buffer.as_mut_ptr().wrapping_add(buffer.len());
        // SAFETY: the buffer's `room` writable bytes at `at`.
        self.handle()
            .recv(unsafe { std::slice::from_raw_parts_mut(at, room) })
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
        self.handle().set_open_length_check(true);
        self.handle().edit_protocol(|protocol| {
            protocol.package_length_size = WEBSOCKET_HEADER_LEN;
            protocol.package_length_offset = 0;
            protocol.package_body_offset = 0;
            protocol.get_package_length = Some(get_package_length);
        });
    }

    /// `ssl_get_peer_cert()`: empty, with the socket's error set, without one.
    pub(crate) fn peer_cert_pem(self) -> Vec<u8> {
        self.handle().peer_cert()
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
    write(fd, data).cast_unsigned() == data.len()
}

/// `AsyncFile::close()`. It can yield, so a caller lets go of the descriptor
/// (and anything that refers to it) before calling it.
pub(crate) fn swoole_http_client_coro_file_close(fd: c_int) {
    // A plain close of a descriptor nothing else holds.
    swoole_coroutine_close(fd);
}

/// `callable($client, $data)` inside `zend_try`: 0, 1 if the call failed,
/// 2 if it bailed out.
pub(crate) unsafe fn swoole_http_client_coro_call(
    callable: &Callable,
    client: *mut zend_object,
    data: *const c_char,
    length: size_t,
) -> c_int {
    let mut args = [UNDEF; 2];
    // SAFETY: the client object, borrowed for the call as upstream's `ZVAL_OBJ`.
    unsafe { set_object(&raw mut args[0], client) };
    // SAFETY: the caller's bytes.
    let data = unsafe { std::slice::from_raw_parts(data.cast::<u8>(), length) };
    args[1] = Owned::string(ZString::new(data)).into_zval();
    let result = match crate::native::call(callable, &mut args, std::ptr::null_mut(), false) {
        Ok(true) => 0,
        Ok(false) => 1,
        Err(bailout::Bailout) => 2,
    };
    // SAFETY: the string argument's reference.
    unsafe { zval_ptr_dtor(&raw mut args[1]) };
    result
}

/// Upstream's `decompress_response` warnings.
pub(crate) fn swoole_http_client_coro_inflate_warning(message: &str) {
    log::warning(c"Client::decompress_response", format_args!("{message}"));
}
