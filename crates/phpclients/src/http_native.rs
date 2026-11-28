//! Coroutine HTTP client access to socket operations, files and callables.
use crate::{native::Co, socket::Sock};
use libc::c_int;
use std::ffi::CStr;
use swoole_rs_corebase::buffer::string::StringBuffer;
use swoole_rs_corebase::{ProxyState, log};
use swoole_rs_hooks::hooked::{swoole_coroutine_close, write};
use swoole_rs_hooks::stdio::{
    swoole_coroutine_ftruncate, swoole_coroutine_lseek, swoole_coroutine_open,
};
use swoole_rs_phpruntime::co;
use swoole_rs_websocket::HEADER_LEN as WEBSOCKET_HEADER_LEN;
use swoole_rs_zend::{Owned, Value};

use swoole_rs_corebase::CCast;
use swoole_rs_corebase::reactor::{Events, TimeoutKind};

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
pub(crate) struct HttpBuffer(Option<Box<StringBuffer>>);

impl HttpBuffer {
    /// The response's own buffer, separate from TLS's socket read buffer.
    pub(crate) fn input(&mut self) -> &mut StringBuffer {
        self.0.get_or_insert_with(|| {
            swoole_rs_corebase::buffer::string::boxed_sized_string(
                swoole_rs_corebase::constants::SW_BUFFER_SIZE_BIG as usize,
                Some(swoole_rs_phpswoole::zend_string_allocator()),
            )
        })
    }

    /// The bytes the socket had written, if it left a buffer.
    pub(crate) fn buffer(&self) -> Option<&StringBuffer> {
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
            value.extend(swoole_rs_http::authorization(proxy));
            Some(value)
        })
        .flatten()
    }

    /// Connect timeout (0: the default) and the zend string allocator.
    pub(crate) fn configure_http(self, timeout: f64) {
        let timeout = if timeout == 0.0 {
            swoole_rs_corebase::reactor::SOCKET_DEFAULTS.connect.get()
        } else {
            timeout
        };
        self.set_timeout(timeout, TimeoutKind::CONNECT);
        self.use_zend_strings();
    }

    /// `connect(host, port)` under the timeout `configure_http` set.
    pub(crate) fn connect_http(self, host: &[u8], port: c_int) -> bool {
        self.handle().connect_host(host, port)
    }

    /// `check_bound_co(event)`: a fatal error if another coroutine holds it.
    pub(crate) fn check_bound(self, event: Events) {
        self.handle().check_bound(event);
    }

    /// Retain the request head and send it with an optional borrowed body.
    pub(crate) fn send_buffered(self, data: &[u8], body: &[u8]) -> bool {
        self.with_write_buffer(|buffer| {
            buffer.clear();
            buffer.append(data);
        });
        if body.is_empty() {
            return self.send_all(data);
        }
        self.send_vectors([data, body])
    }

    pub(crate) fn send_vectors<const N: usize>(self, parts: [&[u8]; N]) -> bool {
        let length = parts.iter().map(|part| part.len()).sum();
        let vectors = parts.map(std::io::IoSlice::new);
        let (sent, _, _) = co::vector(self.handle(), co::Vectors::Write(&vectors), true);
        usize::try_from(sent) == Ok(length)
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
    pub(crate) fn with_write_buffer<R>(self, f: impl FnOnce(&mut StringBuffer) -> R) -> R {
        self.handle().with_buffer(true, f)
    }

    /// `f` over `get_read_buffer()`, made on first use.
    pub(crate) fn with_input<R>(self, f: impl FnOnce(&mut StringBuffer) -> R) -> R {
        self.handle().with_buffer(false, f)
    }

    /// Receive into the response's own buffer, leaving the socket's buffer to TLS.
    pub(crate) fn recv_input(self, input: &mut StringBuffer) -> Option<usize> {
        input.fill(|out| {
            self.handle()
                .recv_spare(swoole_rs_hooks::socket_members::Receive::Recv, out)
        })
    }

    /// A read `TimeoutController` (0: the default read timeout).
    pub(crate) fn read_timeout(self, timeout: f64) -> co::Timeout {
        let timeout = if timeout == 0.0 {
            swoole_rs_corebase::reactor::SOCKET_DEFAULTS.read.get()
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
            protocol.get_package_length = Some(swoole_rs_clients::ws::websocket_length);
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
