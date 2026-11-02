//! What only C++ can do for `Coroutine\Http2\Client`, defined in the patched
//! `ext-src/swoole_http2_client_coro.cc` (declared in
//! `native/http2/swoole_rs_phphttp2.h`): the coroutine socket and nghttp2's
//! HPACK codec, one call per operation over opaque pointers.
use libc::{c_char, c_int, c_void, size_t, ssize_t};
use swoole_rs_zend::sys::{zend_object, zend_string, zval};

/// A `SocketImpl *`.
pub(crate) type Socket = *mut c_void;

/// `nghttp2_nv`.
#[repr(C)]
#[derive(Clone, Copy)]
pub(crate) struct Header {
    pub(crate) name: *const u8,
    pub(crate) value: *const u8,
    pub(crate) name_length: size_t,
    pub(crate) value_length: size_t,
    pub(crate) flags: u8,
}

unsafe extern "C" {
    /// `php_swoole_create_socket(convert_to_type(host))`: the socket object, its
    /// C++ socket and the host as `convert_to_type` rewrote it; null with
    /// `errno` in `error`.
    pub(crate) fn swoole_http2_client_coro_create_socket(
        host: *const c_char,
        length: size_t,
        converted: *mut *mut zend_string,
        socket: *mut Socket,
        error: *mut c_int,
    ) -> *mut zend_object;
    /// The close hook (`swr_v1_phphttp2_client_socket_dtor(conn)`) and zero copy.
    pub(crate) fn swoole_http2_client_coro_setup(socket: Socket, conn: *mut c_void);
    pub(crate) fn swoole_http2_client_coro_enable_ssl(socket: Socket) -> bool;
    /// HTTP/2 framing on the socket, then `apply_setting(settings)`.
    pub(crate) fn swoole_http2_client_coro_protocol(socket: Socket, settings: *const zval);
    /// `php_swoole_socket_set` when `settings` is an array.
    pub(crate) fn swoole_http2_client_coro_apply(socket: Socket, settings: *const zval);
    pub(crate) fn swoole_http2_client_coro_connect(
        socket: Socket,
        host: *const c_char,
        length: size_t,
        port: c_int,
    ) -> bool;
    pub(crate) fn swoole_http2_client_coro_close(socket: Socket) -> bool;
    pub(crate) fn swoole_http2_client_coro_is_connected(socket: Socket) -> bool;
    /// `has_bound(SW_EVENT_WRITE)`.
    pub(crate) fn swoole_http2_client_coro_write_bound(socket: Socket) -> bool;
    pub(crate) fn swoole_http2_client_coro_send_all(
        socket: Socket,
        data: *const c_char,
        length: size_t,
    ) -> ssize_t;
    /// `errCode`, with `errMsg` in `message`.
    pub(crate) fn swoole_http2_client_coro_error(
        socket: Socket,
        message: *mut *const c_char,
    ) -> c_int;
    pub(crate) fn swoole_http2_client_coro_set_error(
        socket: Socket,
        code: c_int,
        message: *const c_char,
    );
    pub(crate) fn swoole_http2_client_coro_recv_packet(socket: Socket, timeout: f64) -> ssize_t;
    pub(crate) fn swoole_http2_client_coro_read_buffer(socket: Socket) -> *const c_char;
    pub(crate) fn swoole_http2_client_coro_inflater(error: *mut c_int) -> *mut c_void;
    pub(crate) fn swoole_http2_client_coro_deflater(
        table_size: u32,
        error: *mut c_int,
    ) -> *mut c_void;
    pub(crate) fn swoole_http2_client_coro_hpack_free(inflater: *mut c_void, deflater: *mut c_void);
    pub(crate) fn swoole_http2_client_coro_deflate_table(deflater: *mut c_void, size: u32)
    -> c_int;
    /// `nghttp2_hd_deflate_bound`.
    pub(crate) fn swoole_http2_client_coro_deflate_bound(
        deflater: *mut c_void,
        headers: *const Header,
        count: size_t,
    ) -> size_t;
    /// `nghttp2_hd_deflate_hd` into `out`: the length, or a negative error.
    pub(crate) fn swoole_http2_client_coro_deflate(
        deflater: *mut c_void,
        out: *mut u8,
        capacity: size_t,
        headers: *const Header,
        count: size_t,
    ) -> ssize_t;
    /// One `nghttp2_hd_inflate_hd` step (`in_final` set).
    pub(crate) fn swoole_http2_client_coro_inflate(
        inflater: *mut c_void,
        input: *const u8,
        length: size_t,
        flags: *mut c_int,
        header: *mut Header,
    ) -> ssize_t;
    pub(crate) fn swoole_http2_client_coro_inflate_end(inflater: *mut c_void);
    pub(crate) fn swoole_http2_client_coro_strerror(code: c_int) -> *const c_char;
    /// `php_swoole_http_parse_set_cookies`.
    pub(crate) fn swoole_http2_client_coro_set_cookies(
        at: *const c_char,
        length: size_t,
        cookies: *mut zval,
        set_cookie_headers: *mut zval,
    );
    /// `swoole_fatal_error(SW_ERROR_WRONG_OPERATION, "must call constructor first")`.
    pub(crate) fn swoole_http2_client_coro_unconstructed();
    /// `swoole_set_last_error(code)`.
    pub(crate) fn swoole_http2_client_coro_last_error(code: c_int);
    /// `swoole_strerror(code)`.
    pub(crate) fn swoole_http2_client_coro_swoole_strerror(code: c_int) -> *const c_char;
    /// `php_swoole_error(E_WARNING, "%s", message)`.
    pub(crate) fn swoole_http2_client_coro_warning(message: *const c_char);
    /// Upstream's logs: 0 unknown stream, 1 decompress failed, 2 inflateInit2 failed.
    pub(crate) fn swoole_http2_client_coro_log(what: c_int, stream_id: u32);
}
