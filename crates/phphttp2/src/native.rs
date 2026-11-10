//! What `Coroutine\Http2\Client` (and the server's HPACK) needs below PHP: the
//! coroutine socket, through crates/phpruntime's [`co`] and its record, and
//! nghttp2's HPACK codec (`thirdparty/nghttp2`) over Zend's allocator.
use libc::{c_char, c_int, c_void, size_t, ssize_t};
use std::ptr;
use swoole_rs_phpruntime::co;
use swoole_rs_zend::sys::{
    swr_zend_ecalloc, swr_zend_efree, swr_zend_emalloc, swr_zend_erealloc, zend_object,
    zend_string, zval,
};

/// A `SocketImpl *`.
pub(crate) type Socket = co::Socket;

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

/// `nghttp2_mem`.
#[repr(C)]
struct Mem {
    user_data: *mut c_void,
    malloc: unsafe extern "C" fn(size_t, *mut c_void) -> *mut c_void,
    free: unsafe extern "C" fn(*mut c_void, *mut c_void),
    calloc: unsafe extern "C" fn(size_t, size_t, *mut c_void) -> *mut c_void,
    realloc: unsafe extern "C" fn(*mut c_void, size_t, *mut c_void) -> *mut c_void,
}

// SAFETY: a constant table of functions; nghttp2 only reads it.
unsafe impl Sync for Mem {}

unsafe extern "C" fn mem_malloc(size: size_t, _: *mut c_void) -> *mut c_void {
    // SAFETY: Zend's allocator, in a request.
    unsafe { swr_zend_emalloc(size) }
}

unsafe extern "C" fn mem_free(block: *mut c_void, _: *mut c_void) {
    // SAFETY: nghttp2 frees what these functions allocated.
    unsafe { swr_zend_efree(block) }
}

unsafe extern "C" fn mem_calloc(count: size_t, size: size_t, _: *mut c_void) -> *mut c_void {
    // SAFETY: as `mem_malloc`.
    unsafe { swr_zend_ecalloc(count, size) }
}

unsafe extern "C" fn mem_realloc(block: *mut c_void, size: size_t, _: *mut c_void) -> *mut c_void {
    // SAFETY: as `mem_free`.
    unsafe { swr_zend_erealloc(block, size) }
}

/// `php_nghttp2_mem()`.
static MEM: Mem = Mem {
    user_data: ptr::null_mut(),
    malloc: mem_malloc,
    free: mem_free,
    calloc: mem_calloc,
    realloc: mem_realloc,
};

unsafe extern "C" {
    fn nghttp2_hd_inflate_new2(inflater: *mut *mut c_void, mem: *const Mem) -> c_int;
    fn nghttp2_hd_deflate_new2(
        deflater: *mut *mut c_void,
        table_size: size_t,
        mem: *const Mem,
    ) -> c_int;
    fn nghttp2_hd_inflate_del(inflater: *mut c_void);
    fn nghttp2_hd_deflate_del(deflater: *mut c_void);
    fn nghttp2_hd_deflate_change_table_size(deflater: *mut c_void, size: size_t) -> c_int;
    fn nghttp2_hd_deflate_bound(
        deflater: *mut c_void,
        headers: *const Header,
        count: size_t,
    ) -> size_t;
    fn nghttp2_hd_deflate_hd(
        deflater: *mut c_void,
        out: *mut u8,
        capacity: size_t,
        headers: *const Header,
        count: size_t,
    ) -> ssize_t;
    fn nghttp2_hd_inflate_hd(
        inflater: *mut c_void,
        header: *mut Header,
        flags: *mut c_int,
        input: *const u8,
        length: size_t,
        in_final: c_int,
    ) -> ssize_t;
    fn nghttp2_hd_inflate_end_headers(inflater: *mut c_void) -> c_int;
    fn nghttp2_strerror(code: c_int) -> *const c_char;
}

/// `php_swoole_create_socket(convert_to_type(host))`: the socket object, its
/// C++ socket and the host as `convert_to_type` rewrote it; null with `errno`
/// in `error`.
pub(crate) unsafe fn swoole_http2_client_coro_create_socket(
    host: *const c_char,
    length: size_t,
    converted: *mut *mut zend_string,
    socket: *mut Socket,
    error: *mut c_int,
) -> *mut zend_object {
    // SAFETY: per the caller, `length` readable bytes.
    let kind = unsafe { swoole_rs_reactor::swr_v1_socket_host_type(host.cast(), length) };
    // SAFETY: as above; `erase` never exceeds the length.
    let name = unsafe { std::slice::from_raw_parts(host.cast::<u8>(), length) };
    let name = swoole_rs_zend::string::copy(name.get(kind.erase..).unwrap_or_default());
    // SAFETY: a writable out-parameter.
    unsafe { *converted = name };
    let object = swoole_rs_phpclients::create_socket(kind.socket_type);
    if object.is_null() {
        // SAFETY: as above.
        unsafe { *error = std::io::Error::last_os_error().raw_os_error().unwrap_or(0) };
        return ptr::null_mut();
    }
    // SAFETY: the new object is a socket object.
    let pointer = unsafe { swoole_rs_phpclients::socket_of(object) };
    // SAFETY: a writable out-parameter.
    unsafe { *socket = pointer };
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
pub(crate) unsafe fn swoole_http2_client_coro_protocol(socket: Socket, settings: *const zval) {
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
pub(crate) unsafe fn swoole_http2_client_coro_apply(socket: Socket, settings: *const zval) {
    // SAFETY: a live zval.
    if unsafe { swoole_rs_zend::type_of(settings) } == swoole_rs_zend::sys::IS_ARRAY {
        // SAFETY: a live socket and array; 0 applies every setting.
        unsafe { swoole_rs_phpclients::socket_settings(socket, settings, 0) };
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
    message: *mut *const c_char,
) -> c_int {
    // SAFETY: per the caller.
    let record = unsafe { co::record(socket) };
    // SAFETY: a writable out-parameter.
    unsafe { *message = record.err_msg };
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
    unsafe { (*buffer).str }.cast()
}

pub(crate) unsafe fn swoole_http2_client_coro_inflater(error: *mut c_int) -> *mut c_void {
    let mut inflater = ptr::null_mut();
    // SAFETY: a writable local.
    let code = unsafe { nghttp2_hd_inflate_new2(&raw mut inflater, &raw const MEM) };
    // SAFETY: a writable out-parameter.
    unsafe { *error = code };
    inflater
}

pub(crate) unsafe fn swoole_http2_client_coro_deflater(
    table_size: u32,
    error: *mut c_int,
) -> *mut c_void {
    let mut deflater = ptr::null_mut();
    // SAFETY: a writable local.
    let code =
        unsafe { nghttp2_hd_deflate_new2(&raw mut deflater, table_size as size_t, &raw const MEM) };
    // SAFETY: a writable out-parameter.
    unsafe { *error = code };
    deflater
}

pub(crate) unsafe fn swoole_http2_client_coro_hpack_free(
    inflater: *mut c_void,
    deflater: *mut c_void,
) {
    if !inflater.is_null() {
        // SAFETY: per the caller, a live inflater.
        unsafe { nghttp2_hd_inflate_del(inflater) };
    }
    if !deflater.is_null() {
        // SAFETY: per the caller, a live deflater.
        unsafe { nghttp2_hd_deflate_del(deflater) };
    }
}

pub(crate) unsafe fn swoole_http2_client_coro_deflate_table(
    deflater: *mut c_void,
    size: u32,
) -> c_int {
    // SAFETY: per the caller.
    unsafe { nghttp2_hd_deflate_change_table_size(deflater, size as size_t) }
}

/// `nghttp2_hd_deflate_bound`.
pub(crate) unsafe fn swoole_http2_client_coro_deflate_bound(
    deflater: *mut c_void,
    headers: *const Header,
    count: size_t,
) -> size_t {
    // SAFETY: per the caller.
    unsafe { nghttp2_hd_deflate_bound(deflater, headers, count) }
}

/// `nghttp2_hd_deflate_hd` into `out`: the length, or a negative error.
pub(crate) unsafe fn swoole_http2_client_coro_deflate(
    deflater: *mut c_void,
    out: *mut u8,
    capacity: size_t,
    headers: *const Header,
    count: size_t,
) -> ssize_t {
    // SAFETY: per the caller.
    unsafe { nghttp2_hd_deflate_hd(deflater, out, capacity, headers, count) }
}

/// One `nghttp2_hd_inflate_hd` step; `last` when the input ends the header block.
pub(crate) unsafe fn swoole_http2_client_coro_inflate(
    inflater: *mut c_void,
    input: *const u8,
    length: size_t,
    flags: *mut c_int,
    header: *mut Header,
    last: bool,
) -> ssize_t {
    // SAFETY: per the caller.
    unsafe { nghttp2_hd_inflate_hd(inflater, header, flags, input, length, c_int::from(last)) }
}

pub(crate) unsafe fn swoole_http2_client_coro_inflate_end(inflater: *mut c_void) {
    // SAFETY: per the caller.
    unsafe { nghttp2_hd_inflate_end_headers(inflater) };
}

pub(crate) unsafe fn swoole_http2_client_coro_strerror(code: c_int) -> *const c_char {
    // SAFETY: nghttp2's static text.
    unsafe { nghttp2_strerror(code) }
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
