//! What only C++ can do for `Coroutine\Http\Client`, defined in the patched
//! `ext-src/swoole_http_client_coro.cc` (declared in
//! `native/clients/swoole_rs_phpclients.h`): the coroutine socket, its buffers
//! and timeouts, WebSocket framing, files and callables are still C++, so each
//! operation is one call over an opaque pointer.
use crate::native::Socket;
use libc::{c_char, c_int, c_void, size_t, ssize_t};
use swoole_rs_zend::sys::{zend_object, zend_string, zval};

unsafe extern "C" {
    /// `php_swoole_check_reactor()`, then a `Coroutine\Socket` of `kind`;
    /// null with `errno` in `error`.
    pub(crate) fn swoole_http_client_coro_create_socket(
        kind: i64,
        error: *mut c_int,
    ) -> *mut zend_object;
    pub(crate) fn swoole_http_client_coro_enable_ssl(socket: Socket) -> bool;
    /// After the settings: no proxy handshake for plain HTTP proxying.
    pub(crate) fn swoole_http_client_coro_apply(socket: Socket);
    /// A `NameResolver::Context`.
    /// `new NameResolver::Context()` (ext-src/swoole_name_resolver.cc).
    pub(crate) fn swoole_name_resolver_context_new() -> *mut c_void;
    pub(crate) fn swoole_http_client_coro_resolve_reset(context: *mut c_void, with_port: bool);
    pub(crate) fn swoole_name_resolver_context_free(context: *mut c_void);
    /// Connect timeout (0: the default), resolve context, the close hook
    /// (`swr_v1_phpclients_http_client_socket_dtor(conn, ..)`) and the zend
    /// string allocator.
    pub(crate) fn swoole_http_client_coro_configure(
        socket: Socket,
        timeout: f64,
        resolve: *mut c_void,
        conn: *mut c_void,
    );
    pub(crate) fn swoole_http_client_coro_connect(
        socket: Socket,
        host: *const c_char,
        length: size_t,
        port: c_int,
    ) -> bool;
    pub(crate) fn swoole_http_client_coro_close(socket: Socket) -> bool;
    /// `check_bound_co(event)`: a fatal error if another coroutine holds it.
    pub(crate) fn swoole_http_client_coro_check_bound(socket: Socket, event: c_int);
    pub(crate) fn swoole_http_client_coro_set_err(socket: Socket, code: c_int);
    /// `swoole_get_last_error()`.
    pub(crate) fn swoole_http_client_coro_last_error() -> c_int;
    /// 0 without a plain HTTP proxy, 1 with one, 2 with one that authenticates.
    pub(crate) fn swoole_http_client_coro_proxy(socket: Socket) -> c_int;
    /// `"Basic " + http_proxy->get_auth_str()`.
    pub(crate) fn swoole_http_client_coro_proxy_auth(socket: Socket) -> *mut zend_string;
    /// The write buffer becomes `data`, then `send_all` of it: whether all went.
    pub(crate) fn swoole_http_client_coro_send(
        socket: Socket,
        data: *const c_char,
        length: size_t,
    ) -> bool;
    /// `send_all(data)`: whether all went.
    pub(crate) fn swoole_http_client_coro_send_raw(
        socket: Socket,
        data: *const c_char,
        length: size_t,
    ) -> bool;
    /// The socket's write buffer, or with no socket `saved` (a popped one).
    pub(crate) fn swoole_http_client_coro_written(
        socket: Socket,
        saved: *mut c_void,
        length: *mut size_t,
    ) -> *const c_char;
    pub(crate) fn swoole_http_client_coro_buffer_free(buffer: *mut c_void);
    /// The read buffer's storage, length and size.
    pub(crate) fn swoole_http_client_coro_read_buffer(
        socket: Socket,
        length: *mut size_t,
        size: *mut size_t,
    ) -> *mut c_char;
    pub(crate) fn swoole_http_client_coro_read_buffer_set(socket: Socket, length: size_t);
    /// Keep the read buffer's bytes past `offset` (`reduce`).
    pub(crate) fn swoole_http_client_coro_read_buffer_keep(
        socket: Socket,
        length: size_t,
        offset: size_t,
    );
    pub(crate) fn swoole_http_client_coro_recv(
        socket: Socket,
        data: *mut c_char,
        length: size_t,
    ) -> ssize_t;
    /// A read `TimeoutController` (0: the default read timeout).
    pub(crate) fn swoole_http_client_coro_timeout(socket: Socket, timeout: f64) -> *mut c_void;
    pub(crate) fn swoole_http_client_coro_timed_out(controller: *mut c_void) -> bool;
    pub(crate) fn swoole_http_client_coro_timeout_end(controller: *mut c_void);
    /// WebSocket settings and continuation buffer.
    pub(crate) fn swoole_http_client_coro_ws_new() -> *mut c_void;
    pub(crate) fn swoole_http_client_coro_ws_free(ws: *mut c_void);
    pub(crate) fn swoole_http_client_coro_ws_apply(ws: *mut c_void, settings: *const zval);
    pub(crate) fn swoole_http_client_coro_ws_compression(ws: *mut c_void) -> bool;
    /// Pack a frame into the write buffer and send it: 0, 1 if packing failed,
    /// 2 if sending did.
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
    /// The socket frames WebSocket packets from now on.
    pub(crate) fn swoole_http_client_coro_ws_protocol(socket: Socket);
    /// An `AsyncFile` for a download, truncated or positioned; null after
    /// upstream's warning.
    pub(crate) fn swoole_http_client_coro_file_open(
        name: *const c_char,
        offset: i64,
    ) -> *mut c_void;
    pub(crate) fn swoole_http_client_coro_file_write(
        file: *mut c_void,
        data: *const c_char,
        length: size_t,
    ) -> bool;
    pub(crate) fn swoole_http_client_coro_file_free(file: *mut c_void);
    /// `sw_callable_create(fn)`: null after a warning.
    pub(crate) fn swoole_http_client_coro_callable(function: *mut zval) -> *mut c_void;
    pub(crate) fn swoole_http_client_coro_callable_free(callable: *mut c_void);
    /// `callable($client, $data)` inside `zend_try`: 0, 1 if the call failed,
    /// 2 if it bailed out.
    pub(crate) fn swoole_http_client_coro_call(
        callable: *mut c_void,
        client: *mut zend_object,
        data: *const c_char,
        length: size_t,
    ) -> c_int;
    pub(crate) fn swoole_http_client_coro_peer_cert(socket: Socket) -> *mut zend_string;
    /// `mime_type::get(path)`.
    pub(crate) fn swoole_http_client_coro_mime(path: *const c_char) -> *const c_char;
    /// `php_swoole_sys_error(E_WARNING, "%s", message)`.
    pub(crate) fn swoole_http_client_coro_sys_error(message: *const c_char);
    /// Upstream's `decompress_response` warnings: unknown `method`, or failed (-1).
    pub(crate) fn swoole_http_client_coro_inflate_warning(method: c_int);
}
