//! What `Coroutine\Http\Client` needs from the coroutine socket (methods on
//! the typed [`Co`] handle: its proxy, buffers and timeouts), files and
//! callables. WebSocket framing is crates/phphttp's, which registers it here
//! ([`WebSocket`]); the name resolver's context is still C++.
use crate::{native::Co, socket::Sock};
use libc::c_int;
use std::{ffi::CStr, sync::OnceLock};
use swoole_rs_corebase::buffer::string::CppString;
use swoole_rs_corebase::{ProxyState, log, sys::Text};
use swoole_rs_hooks::hooked::{swoole_coroutine_close, write};
use swoole_rs_hooks::socket_extra::{CloseHook, set_close_hook};
// crates/hooks' coroutine file calls, by symbol: each reads its path up to the NUL
// or works on a descriptor, as `open`, `ftruncate` and `lseek` do, so they are
// declared `safe`.
unsafe extern "C" {
    safe fn swoole_coroutine_open(path: Text<'_>, flags: c_int, mode: libc::mode_t) -> c_int;
    safe fn swoole_coroutine_ftruncate(fd: c_int, length: libc::off_t) -> c_int;
    safe fn swoole_coroutine_lseek(fd: c_int, offset: libc::off_t, whence: c_int) -> libc::off_t;
}
use swoole_rs_phpruntime::co;
use swoole_rs_zend::{Array, Owned, Value};

/// `Coroutine\Http\Client`'s WebSocket state: its `WebSocketSettings` and
/// the message it is assembling. Framing is crates/phphttp's, which depends
/// on this crate, so it registers the state's constructor at MINIT
/// ([`register_websocket`]). Methods take `&self`: a `recv` yields, and other
/// coroutines use the client meanwhile.
pub trait WebSocket {
    /// `WebSocket::apply_setting(settings, vht, false)`.
    fn apply(&self, settings: &Array);
    /// Whether the client offers `permessage-deflate`.
    fn compression(&self) -> bool;
    /// Pack a frame of `data` and send it on `socket`.
    /// # Errors
    /// Which step failed; a failed pack may leave a bailout pending, for the
    /// handler to raise.
    fn push(
        &self,
        socket: Co,
        data: &Value,
        opcode: i64,
        flags: c_int,
        code: i64,
        compress: bool,
    ) -> Result<(), PushError>;
    /// `websocket::recv_frame` on `socket`: the frame for the call's return
    /// slot; a bailout it defers is the handler's to raise.
    fn recv(&self, socket: Co, timeout: f64) -> Owned;
}

/// Where [`WebSocket::push`] failed.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PushError {
    Pack,
    Send,
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

use swoole_rs_corebase::CCast;
use swoole_rs_reactor::{Events, TimeoutKind};
/// `SW_WEBSOCKET_HEADER_LEN`.
const WEBSOCKET_HEADER_LEN: u8 = 2;

/// `php_swoole_check_reactor()`, then a `Coroutine\Socket` of `kind`, or
/// `errno`.
pub(crate) fn swoole_http_client_coro_create_socket(kind: i64) -> Result<Owned, c_int> {
    swoole_rs_phpruntime::check_reactor();
    match crate::socket::owner::create_kind(kind.c_cast::<i32>()) {
        Ok(Some(object)) => Ok(object),
        Ok(None) => Err(std::io::Error::last_os_error().raw_os_error().unwrap_or(0)),
        Err(bailout) => {
            bailout.defer();
            Err(0)
        }
    }
}

/// An owned HTTP buffer: a response's input or the write buffer a closed socket left.
#[derive(Default)]
pub(crate) struct HttpBuffer(Option<Box<CppString>>);

impl HttpBuffer {
    /// The response's own buffer, separate from TLS's socket read buffer.
    pub(crate) fn input(&mut self) -> &mut CppString {
        self.0.get_or_insert_with(|| {
            swoole_rs_corebase::buffer::string::boxed_sized_string(
                swoole_rs_corebase::constants::SW_BUFFER_SIZE_BIG as usize,
                Some(swoole_rs_phpswoole::zend_string_allocator()),
            )
        })
    }

    /// The bytes the socket had written, if it left a buffer.
    pub(crate) fn buffer(&self) -> Option<&CppString> {
        self.0.as_deref()
    }
}

impl Drop for HttpBuffer {
    /// `delete string`.
    fn drop(&mut self) {
        if let Some(mut string) = self.0.take() {
            string.dispose();
        }
    }
}

/// The core socket of the `Coroutine\Socket` object `socket` holds frames
/// WebSocket packets from now on.
pub fn websocket_protocol(socket: &Value) {
    if let Some(co) = Sock::of(socket).and_then(Sock::co) {
        co.websocket_protocol();
    }
}

/// What the HTTP client asks of its socket.
impl Co {
    /// `f` over the HTTP proxy's state, unless TLS tunnels through it.
    fn with_plain_proxy<T>(self, f: impl FnOnce(&ProxyState) -> T) -> Option<T> {
        if !self.ssl_context().is_null() {
            return None;
        }
        self.handle().with_http_proxy(f)
    }

    /// After the settings: no proxy handshake for plain HTTP proxying.
    pub(crate) fn skip_proxy_handshake(self) {
        self.with_plain_proxy(|proxy| proxy.update(|fields| fields.dont_handshake = true));
    }

    /// Whether requests go through a plain HTTP proxy.
    pub(crate) fn proxied(self) -> bool {
        self.with_plain_proxy(|_| ()).is_some()
    }

    /// `"Basic " + http_proxy->get_auth_str()`, when a plain HTTP proxy has a
    /// password.
    pub(crate) fn proxy_authorization(self) -> Option<Vec<u8>> {
        self.with_plain_proxy(|proxy| {
            if proxy.string(2).is_empty() {
                return None;
            }
            let mut value = b"Basic ".to_vec();
            value.extend(swoole_rs_http_ffi::authorization(proxy));
            Some(value)
        })
        .flatten()
    }

    /// Connect timeout (0: the default), the close hook and the zend string
    /// allocator.
    pub(crate) fn configure_http(self, timeout: f64, hook: CloseHook) {
        let timeout = if timeout == 0.0 {
            swoole_rs_reactor::SOCKET_DEFAULTS.connect.get()
        } else {
            timeout
        };
        self.set_timeout(timeout, TimeoutKind::CONNECT);
        self.use_zend_strings();
        set_close_hook(self.handle(), hook);
    }

    /// `connect(host, port)` under the timeout `configure_http` set.
    pub(crate) fn connect_http(self, host: &[u8], port: c_int) -> bool {
        self.handle().connect_host(host, port)
    }

    /// `check_bound_co(event)`: a fatal error if another coroutine holds it.
    pub(crate) fn check_bound(self, event: Events) {
        self.handle().check_bound(event);
    }

    /// The write buffer becomes `data`, then `send_all` of it: whether all went.
    pub(crate) fn send_buffered(self, data: &[u8]) -> bool {
        self.with_write_buffer(|buffer| {
            buffer.clear();
            buffer.append(data);
        });
        self.send_all(data)
    }

    /// `send_all(data)`: whether all went.
    pub(crate) fn send_all(self, data: &[u8]) -> bool {
        self.handle().send_all(data) == Some(data.len())
    }

    /// The socket gives up its write buffer, as before its first write.
    pub(crate) fn take_write_buffer(self) -> HttpBuffer {
        HttpBuffer(self.handle().take_write_buffer())
    }

    /// `f` over `get_write_buffer()`, made on first use.
    pub(crate) fn with_write_buffer<R>(self, f: impl FnOnce(&mut CppString) -> R) -> R {
        self.handle().with_buffer(true, f)
    }

    /// `f` over `get_read_buffer()`, made on first use.
    pub(crate) fn with_input<R>(self, f: impl FnOnce(&mut CppString) -> R) -> R {
        self.handle().with_buffer(false, f)
    }

    /// Receive into the response's own buffer, leaving the socket's buffer to TLS.
    pub(crate) fn recv_input(self, input: &mut CppString) -> Option<usize> {
        input.fill(|out| {
            self.handle()
                .recv_spare(swoole_rs_hooks::socket_members::Receive::Recv, out)
        })
    }

    /// A read `TimeoutController` (0: the default read timeout).
    pub(crate) fn read_timeout(self, timeout: f64) -> co::Timeout {
        let timeout = if timeout == 0.0 {
            swoole_rs_reactor::SOCKET_DEFAULTS.read.get()
        } else {
            timeout
        };
        self.timeout(timeout, TimeoutKind::READ)
    }

    /// The socket frames WebSocket packets from now on.
    pub(crate) fn websocket_protocol(self) {
        self.handle().set_open_length_check(true);
        self.handle().edit_protocol(|protocol| {
            protocol.package_length_size = WEBSOCKET_HEADER_LEN;
            protocol.package_length_offset = 0;
            protocol.package_body_offset = 0;
            protocol.get_package_length = Some(swoole_rs_ports::http_routes::websocket_length);
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
    let fd = swoole_coroutine_open(name.into(), libc::O_CREAT | libc::O_WRONLY, 0o664);
    let origin = c"http_parser_on_body";
    let failed = if fd == -1 {
        log::sys_warning(
            origin,
            format_args!("open({path}, O_CREAT | O_WRONLY) failed"),
        );
        return None;
    } else if offset == 0 && swoole_coroutine_ftruncate(fd, 0) != 0 {
        log::sys_warning(origin, format_args!("ftruncate({path}) failed"));
        true
    // Upstream fails only on a zero result.
    } else if offset != 0 && swoole_coroutine_lseek(fd, offset, libc::SEEK_SET) == 0 {
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
    write(fd, data).is_ok_and(|written| written == data.len())
}

/// `AsyncFile::close()`. It can yield, so a caller lets go of the descriptor
/// (and anything that refers to it) before calling it.
pub(crate) fn swoole_http_client_coro_file_close(fd: c_int) {
    // A plain close of a descriptor nothing else holds.
    swoole_coroutine_close(fd);
}

/// Upstream's `decompress_response` warnings.
pub(crate) fn swoole_http_client_coro_inflate_warning(message: &str) {
    log::warning(c"Client::decompress_response", format_args!("{message}"));
}
