//! The HTTP/1 response header and trailer, and body compression, as
//! `swoole::http::Context::build_header`, `build_trailer` and `compress` did
//! them. The bytes go to a thread-local buffer C++ appends to its own.
use crate::host;
use crate::parse::CAPABILITIES;
use crate::state::State;
use std::cell::RefCell;
use std::ffi::{CStr, c_char, c_int, c_void};
use std::ptr;
use std::sync::atomic::{AtomicU32, Ordering::Relaxed};
use swoole_rs_deflate::{DEFLATE, Failure, GZIP, bound, compress as deflate};
use swoole_rs_http::swr_v2_http_status_message;
use swoole_rs_httpbindings::{
    Text, swr_v1_httpbindings_codec, swr_v1_httpbindings_codec_level, swr_v1_httpbindings_encoding,
    swr_v1_httpbindings_response_header,
};
use swoole_rs_zend::sys::php_format_date;
use swoole_rs_zend::sys::{
    HashPosition, IS_ARRAY, IS_NULL, IS_STRING, swr_zend_efree, swr_zend_emalloc,
    swr_zend_http_get_string, swr_zend_http_property_offset, swr_zend_string_release,
    zend_class_entry, zend_hash_get_current_data_ex, zend_hash_get_current_key_ex,
    zend_hash_internal_pointer_reset_ex, zend_hash_move_forward_ex, zend_long_to_str, zend_object,
    zend_string, zval,
};
use swoole_rs_zend::{bailout, string, type_of, value};

/// `swHttpHeaderFlag`.
const SERVER: i32 = 2;
const CONNECTION: i32 = 4;
const CONTENT_LENGTH: i32 = 8;
const DATE: i32 = 16;
const CONTENT_TYPE: i32 = 32;
const TRANSFER_ENCODING: i32 = 64;
const CONTENT_ENCODING: i32 = 256;
/// llhttp's `HTTP_HEAD`.
const HEAD: u8 = 2;
/// `SW_LOG_WARNING`, `SW_ERROR_HTTP_CONFLICT_HEADER`.
const LOG_WARNING: c_int = 4;
const CONFLICT_HEADER: c_int = 7105;
/// MINIT's codec bits (`parse.rs`): zlib, and any codec built.
const ZLIB: u32 = 1;
const CODECS: u32 = 7;
const COMPRESSION: u32 = 8;
/// `HTTP_COMPRESS_GZIP` and `HTTP_COMPRESS_DEFLATE`.
const METHOD_GZIP: i32 = 1;
const METHOD_DEFLATE: i32 = 2;
/// `sw_tg_buffer()`'s size, which bounds one trailer line.
const TRAILER_LINE: usize = 65536;
/// `HASH_KEY_IS_STRING`, `HASH_KEY_IS_LONG`.
const KEY_STRING: i64 = 1;
const KEY_LONG: i64 = 2;

/// `Swoole\Http\Response`'s `header`, `cookie` and `trailer` slots.
static OFFSETS: [AtomicU32; 3] = [const { AtomicU32::new(0) }; 3];

thread_local! {
    static OUT: RefCell<Vec<u8>> = const { RefCell::new(Vec::new()) };
}

unsafe extern "C" {
    fn swr_http_host_compress_other(
        context: *mut c_void,
        method: c_int,
        data: *const c_char,
        length: usize,
    ) -> c_int;
}

/// MINIT (from the response class's): find the array properties' slots.
pub(crate) fn minit(ce: *mut zend_class_entry) {
    for (slot, name) in OFFSETS.iter().zip(["header", "cookie", "trailer"]) {
        // SAFETY: the registered class, which declares the property.
        let offset = unsafe { swr_zend_http_property_offset(ce, name.as_ptr().cast(), name.len()) };
        slot.store(offset, Relaxed);
    }
}

/// `swoole_http_init_and_read_property`: the response's `header`, `cookie`
/// or `trailer` array (`host::HEADER`, `COOKIE`, `TRAILER`), made on first use.
pub(crate) fn response_property(state: &mut State, which: c_int) -> *mut zval {
    let response = &mut state.response;
    let slot = match which {
        host::HEADER => &mut response.zheader,
        host::COOKIE => &mut response.zcookie,
        _ => &mut response.ztrailer,
    };
    if slot.ptr.is_null() {
        // SAFETY: the response's live object zval.
        let object: *mut zend_object = unsafe { value::obj_of(response.zobject.ptr) };
        let offset = OFFSETS[usize::try_from(which).unwrap_or(2).min(2)].load(Relaxed);
        // SAFETY: a declared property slot of that object.
        let array = unsafe { swoole_rs_zend::sys::swr_zend_http_property_array(object, offset, 8) };
        // SAFETY: the array zval just initialised.
        slot.value = unsafe { *array };
        slot.ptr = &raw mut slot.value;
    }
    slot.ptr
}

/// `sw_zend_read_property_ex(swoole_http_response_ce, response.zobject, name)`.
fn property(state: &State, which: usize) -> *mut zval {
    // SAFETY: the response's live object zval.
    let object: *mut zend_object = unsafe { value::obj_of(state.response.zobject.ptr) };
    let offset = OFFSETS[which].load(Relaxed) as usize;
    // OBJ_PROP: a declared slot of that object.
    object.cast::<u8>().wrapping_add(offset).cast()
}

/// `swr_zend_http_string`: `zval_get_string`'s argument and result.
#[repr(C)]
struct Arg {
    value: *mut zval,
    result: *mut zend_string,
}

/// `zend::String(value)`: a string zval's own string (the caller does not
/// release it) or `zval_get_string`, run in `zend_try` because it may call
/// `__toString` (released by the caller).
fn to_string(value_: *mut zval) -> Result<(*mut zend_string, bool), bailout::Bailout> {
    // SAFETY: a live zval.
    if unsafe { type_of(value_) } == IS_STRING {
        // SAFETY: a string zval.
        return Ok((unsafe { value::str_of(value_) }, false));
    }
    let mut arg = Arg {
        value: value_,
        result: ptr::null_mut(),
    };
    // SAFETY: a C body over a live zval and a writable result.
    unsafe { bailout::trap(swr_zend_http_get_string, (&raw mut arg).cast())? };
    Ok((arg.result, true))
}

fn release(string_: *mut zend_string, owned: bool) {
    if owned {
        // SAFETY: a reference `to_string` took.
        unsafe { swr_zend_string_release(string_) };
    }
}

/// Each element of a live array (`ZEND_HASH_FOREACH_KEY_VAL`): `Some(key)`
/// for a string key, `None` with the index for an integer key. Stops at
/// the first `Err`.
fn each<E>(
    array: *mut zval,
    mut body: impl FnMut(Result<*mut zend_string, i64>, *mut zval) -> Result<(), E>,
) -> Result<(), E> {
    // SAFETY: a live array zval.
    let table = unsafe { value::table(array) };
    let mut position: HashPosition = 0;
    // SAFETY: a live table and a writable position.
    unsafe { zend_hash_internal_pointer_reset_ex(table, &raw mut position) };
    loop {
        let mut name: *mut zend_string = ptr::null_mut();
        let mut index = 0;
        // SAFETY: as above.
        let kind = unsafe {
            zend_hash_get_current_key_ex(table, &raw mut name, &raw mut index, &raw mut position)
        };
        let key = match i64::from(kind) {
            KEY_STRING => Ok(name),
            KEY_LONG => Err(i64::from_ne_bytes(index.to_ne_bytes())),
            _ => return Ok(()),
        };
        // SAFETY: as above.
        let data = unsafe { zend_hash_get_current_data_ex(table, &raw mut position) };
        body(key, data)?;
        // SAFETY: as above.
        unsafe { zend_hash_move_forward_ex(table, &raw mut position) };
    }
}

thread_local! {
    /// `php_swoole_http_get_date`'s per-second cache: the time and its text.
    static DATE_CACHE: std::cell::Cell<(libc::time_t, *mut zend_string)> =
        const { std::cell::Cell::new((0, ptr::null_mut())) };
}

/// The `Date` header for now, formatted at most once a second.
pub(crate) fn date() -> *mut zend_string {
    // SAFETY: reads the clock.
    let now = unsafe { libc::time(ptr::null_mut()) };
    let (time, date) = DATE_CACHE.get();
    if now == time && !date.is_null() {
        return date;
    }
    if !date.is_null() {
        // SAFETY: the cache's reference.
        unsafe { swr_zend_string_release(date) };
    }
    let format = c"D, d M Y H:i:s T";
    // SAFETY: a NUL-terminated format and its length.
    let date = unsafe { php_format_date(format.as_ptr(), format.count_bytes(), now, false) };
    DATE_CACHE.set((now, date));
    date
}

/// `php_swoole_http_response_rshutdown`: drop the cached date.
#[unsafe(no_mangle)]
pub extern "C" fn swr_v1_phphttp_response_rshutdown() {
    let (_, date) = DATE_CACHE.replace((0, ptr::null_mut()));
    if !date.is_null() {
        // SAFETY: the cache's reference.
        unsafe { swr_zend_string_release(date) };
    }
}

/// `swoole_http_has_crlf`, warning as upstream does.
pub(crate) fn has_crlf(bytes: &[u8]) -> bool {
    for &byte in bytes {
        if byte == b'\n' || byte == b'\r' {
            host::notice("Header may not contain more than a single header, new line detected");
            return true;
        }
        if byte == 0 {
            host::notice("Header may not contain NUL bytes");
            return true;
        }
    }
    false
}

/// `http_response_add_custom_header`. Like upstream, the right trim writes
/// into the value's own string.
fn add_custom_header(
    out: &mut Vec<u8>,
    key: &[u8],
    value_: *mut zval,
) -> Result<(), bailout::Bailout> {
    // SAFETY: a live zval.
    if unsafe { type_of(value_) } == IS_NULL {
        return Ok(());
    }
    let (text, owned) = to_string(value_)?;
    // SAFETY: a live string.
    let bytes = unsafe { string::bytes(text) };
    let trimmed = bytes
        .iter()
        .rposition(|byte| !byte.is_ascii_whitespace_c())
        .map_or(0, |last| last + 1);
    if trimmed != bytes.len() {
        let val = string::val(text);
        // SAFETY: the string holds `bytes.len()` bytes; `swoole::rtrim` zeroes the trimmed ones.
        unsafe { ptr::write_bytes(val.wrapping_add(trimmed), 0, bytes.len() - trimmed) };
        // SAFETY: as above: `ZSTR_LEN(str) = swoole::rtrim(...)`.
        unsafe { (*text).len = trimmed };
    }
    // SAFETY: as above.
    let bytes = unsafe { string::bytes(text) };
    if !has_crlf(bytes) {
        out.extend_from_slice(key);
        out.extend_from_slice(b": ");
        out.extend_from_slice(bytes);
        out.extend_from_slice(b"\r\n");
    }
    release(text, owned);
    Ok(())
}

/// C's `isspace` in the "C" locale.
trait CSpace {
    fn is_ascii_whitespace_c(&self) -> bool;
}
impl CSpace for u8 {
    fn is_ascii_whitespace_c(&self) -> bool {
        matches!(self, b' ' | b'\t' | b'\n' | 0x0b | 0x0c | b'\r')
    }
}

fn log_conflict(message: &str) {
    host::log(
        LOG_WARNING,
        CONFLICT_HEADER,
        c"Context::build_header",
        message,
    );
}

/// The header's key and the header flag it sets (0 for a custom header).
fn header_key(key: Result<*mut zend_string, i64>) -> (*mut zend_string, bool, i32) {
    let (key, owned) = match key {
        // SAFETY: a key of a live array, borrowed.
        Ok(name) => (name, false),
        // SAFETY: Zend formats the integer as a new string.
        Err(index) => (unsafe { zend_long_to_str(index) }, true),
    };
    // SAFETY: a live string.
    let bytes = unsafe { string::bytes(key) };
    // SAFETY: the key's bytes.
    let flag = unsafe {
        swr_v1_httpbindings_response_header(Text {
            data: bytes.as_ptr(),
            length: bytes.len(),
        })
    };
    (key, owned, flag)
}

fn build(
    state: &mut State,
    body: *const c_char,
    mut length: usize,
    out: &mut Vec<u8>,
) -> Result<(), bailout::Bailout> {
    let compression = CAPABILITIES.load(Relaxed) & COMPRESSION != 0;
    out.clear();
    // http status line
    out.extend_from_slice(b"HTTP/1.1 ");
    if state.response.reason.is_null() {
        // SAFETY: the http module's static status texts.
        let status = unsafe { CStr::from_ptr(swr_v2_http_status_message(state.response.status)) };
        out.extend_from_slice(status.to_bytes());
    } else {
        out.extend_from_slice(state.response.status.to_string().as_bytes());
        out.push(b' ');
        // SAFETY: `status()` stored a NUL-terminated reason.
        out.extend_from_slice(unsafe { CStr::from_ptr(state.response.reason) }.to_bytes());
    }
    out.extend_from_slice(b"\r\n");

    // http headers
    let mut flags = 0;
    let header = property(state, 0);
    // SAFETY: a declared property slot.
    if unsafe { type_of(header) } == IS_ARRAY {
        // Asked once, like upstream's `compression_types` test.
        let typed = compression && crate::context::compression_type(state, None);
        let mut content_type: Option<(*mut zend_string, bool)> = None;
        let walked = each(header, |key, value_| {
            let (key, owned_key, flag) = header_key(key);
            let result = (|| {
                if flag > 0 {
                    if compression {
                        if flag == CONTENT_TYPE && state.accept_compression != 0 && typed {
                            if let Some((old, owned)) = content_type.take() {
                                release(old, owned);
                            }
                            let (text, owned) = to_string(value_)?;
                            if !owned {
                                // SAFETY: a live string; `zval_get_string` would add this reference.
                                unsafe { swoole_rs_zend::sys::swr_zend_string_copy(text) };
                            }
                            content_type = Some((text, true));
                        }
                        // SAFETY: a live zval.
                        if flag == CONTENT_ENCODING && unsafe { type_of(value_) } == IS_STRING {
                            state.accept_compression = 0;
                        }
                        // https://github.com/swoole/swoole-src/issues/4857
                        // https://github.com/swoole/swoole-src/issues/5026
                        if flag == CONTENT_LENGTH && state.accept_compression != 0 {
                            log_conflict(
                                "The client has set 'Accept-Encoding', 'Content-Length' will be ignored",
                            );
                            return Ok(());
                        }
                    }
                    if flag == CONTENT_LENGTH && state.send_chunked != 0 {
                        log_conflict(
                            "You have set 'Transfer-Encoding', 'Content-Length' will be ignored",
                        );
                        return Ok(());
                    }
                    flags |= flag;
                    // SAFETY: a live zval.
                    if unsafe { swoole_rs_zend::string_of(value_) }.is_some_and(<[u8]>::is_empty) {
                        return Ok(());
                    }
                }
                // SAFETY: a live string key.
                let name = unsafe { string::bytes(key) };
                // SAFETY: a live zval.
                if unsafe { type_of(value_) } == IS_ARRAY {
                    each(value_, |_, item| add_custom_header(out, name, item))
                } else {
                    add_custom_header(out, name, value_)
                }
            })();
            release(key, owned_key);
            result
        });
        if walked.is_ok() && compression && state.accept_compression != 0 && typed {
            let kind = content_type.map_or(&b"text/html"[..], |(text, _)| {
                // SAFETY: a string this function holds.
                unsafe { string::bytes(text) }
            });
            state.accept_compression =
                u8::from(crate::context::compression_type(state, Some(kind)));
        }
        if let Some((text, owned)) = content_type {
            release(text, owned);
        }
        walked?;
    }

    // http cookies
    let cookie = property(state, 1);
    // SAFETY: a declared property slot.
    if unsafe { type_of(cookie) } == IS_ARRAY {
        each::<()>(cookie, |_, value_| {
            // SAFETY: a live zval.
            if let Some(line) = unsafe { swoole_rs_zend::string_of(value_) }
                && !has_crlf(line)
            {
                out.extend_from_slice(b"Set-Cookie: ");
                out.extend_from_slice(line);
                out.extend_from_slice(b"\r\n");
            }
            Ok(())
        })
        .ok();
    }

    if flags & SERVER == 0 {
        out.extend_from_slice(b"Server: swoole-http-server\r\n");
    }
    if flags & DATE == 0 {
        let date = date();
        // SAFETY: a live string.
        let date = unsafe { string::bytes(date) };
        out.extend_from_slice(b"Date: ");
        out.extend_from_slice(date);
        out.extend_from_slice(b"\r\n");
    }
    // websocket protocol (subsequent header info is unnecessary)
    if state.upgrade == 1 {
        out.extend_from_slice(b"\r\n");
        state.send_header = 1;
        return Ok(());
    }
    if flags & CONNECTION == 0 {
        out.extend_from_slice(if state.keepalive != 0 {
            b"Connection: keep-alive\r\n"
        } else {
            b"Connection: close\r\n"
        });
    }
    if flags & CONTENT_TYPE == 0 {
        out.extend_from_slice(b"Content-Type: text/html\r\n");
    }
    if state.send_chunked != 0 {
        if flags & TRANSFER_ENCODING == 0 {
            out.extend_from_slice(b"Transfer-Encoding: chunked\r\n");
        }
    } else if length > 0 || state.parser.method != HEAD {
        if compression && compress(state, body, length) {
            length = compressed(state).len();
            let codecs = CAPABILITIES.load(Relaxed) & CODECS;
            let encoding =
                swr_v1_httpbindings_encoding(i32::from(state.compression_method), codecs);
            out.extend_from_slice(b"Content-Encoding: ");
            // SAFETY: a static label.
            out.extend_from_slice(unsafe {
                std::slice::from_raw_parts(encoding.data, encoding.length)
            });
            out.extend_from_slice(b"\r\n");
        }
        if flags & CONTENT_LENGTH == 0 {
            out.extend_from_slice(b"Content-Length: ");
            out.extend_from_slice(length.to_string().as_bytes());
            out.extend_from_slice(b"\r\n");
        }
    }
    out.extend_from_slice(b"\r\n");
    state.send_header = 1;
    Ok(())
}

/// `Context::build_header` of a `length`-byte body (compressing it when the
/// response asks to), then `f(state, header)`. `f` must not yield or re-enter:
/// the bytes are thread-local.
/// # Errors
/// A header value's `__toString` bailed out.
pub(crate) fn header<R>(
    state: &mut State,
    body: *const c_char,
    length: usize,
    f: impl FnOnce(&mut State, &[u8]) -> R,
) -> Result<R, bailout::Bailout> {
    OUT.with_borrow_mut(|out| {
        build(state, body, length, out)?;
        Ok(f(state, out))
    })
}

/// `Context::build_trailer`: `name: value` lines and the closing CRLF, then
/// `f(trailer)`, as [`header`].
/// # Errors
/// A trailer value's `__toString` bailed out.
pub(crate) fn trailer<R>(state: &State, f: impl FnOnce(&[u8]) -> R) -> Result<R, bailout::Bailout> {
    OUT.with_borrow_mut(|out| {
        out.clear();
        let trailer = property(state, 2);
        // SAFETY: a declared property slot.
        let built: Result<(), bailout::Bailout> = if unsafe { type_of(trailer) } == IS_ARRAY {
            each(trailer, |key, value_| {
                // SAFETY: a live zval.
                let Ok(key) = key else { return Ok(()) };
                // SAFETY: as above.
                if unsafe { type_of(value_) } == IS_NULL {
                    return Ok(());
                }
                let (text, owned) = to_string(value_)?;
                // `%.*s` stops at a NUL, and sw_snprintf at its buffer.
                let until_nul =
                    |bytes: &[u8]| bytes.iter().position(|&b| b == 0).unwrap_or(bytes.len());
                // SAFETY: a live key and string.
                let name = unsafe { string::bytes(key) };
                // SAFETY: as above.
                let value_ = unsafe { string::bytes(text) };
                let mut line = Vec::with_capacity(name.len() + value_.len() + 4);
                line.extend_from_slice(&name[..until_nul(name)]);
                line.extend_from_slice(b": ");
                line.extend_from_slice(&value_[..until_nul(value_)]);
                line.extend_from_slice(b"\r\n");
                line.truncate(TRAILER_LINE - 1);
                out.extend_from_slice(&line);
                release(text, owned);
                Ok(())
            })
        } else {
            Ok(())
        };
        built?;
        out.extend_from_slice(b"\r\n");
        Ok(f(out))
    })
}

pub(crate) unsafe extern "C" fn zalloc(_: *mut c_void, items: u32, size: u32) -> *mut c_void {
    // `safe_emalloc(items, size, 0)`; an overflow fails the allocation.
    let Some(bytes) = (items as usize).checked_mul(size as usize) else {
        return ptr::null_mut();
    };
    // SAFETY: PHP's request allocator.
    unsafe { swr_zend_emalloc(bytes) }
}

pub(crate) unsafe extern "C" fn zfree(_: *mut c_void, address: *mut c_void) {
    // SAFETY: memory `zalloc` returned.
    unsafe { swr_zend_efree(address) };
}

fn compress_warning(message: &str) {
    host::log(0, 0, c"Context::compress", message);
}

/// `Context::compress`.
fn compress(state: &mut State, data: *const c_char, length: usize) -> bool {
    let context = ptr::from_mut(state).cast::<c_void>();
    let capabilities = CAPABILITIES.load(Relaxed);
    let method = swr_v1_httpbindings_codec(
        i32::from(state.compression_method),
        capabilities & CODECS,
        u32::from(state.accept_compression),
        length,
        state.compression_min_length,
    );
    if method == 0 {
        return false;
    }
    if method < 0 {
        compress_warning("Unknown compression method");
        return false;
    }
    // SAFETY: the context and the body's span; C++ handles brotli and zstd.
    match unsafe { swr_http_host_compress_other(context, method, data, length) } {
        1 => return true,
        0 => return false,
        _ => {}
    }
    if capabilities & ZLIB == 0 {
        return false;
    }
    let window_bits = match method {
        METHOD_GZIP => GZIP,
        METHOD_DEFLATE => DEFLATE,
        _ => 0,
    };
    let level = swr_v1_httpbindings_codec_level(method, i32::from(state.compression_level), 0, 0);
    #[expect(
        clippy::cast_possible_truncation,
        reason = "Context::compression_level is int8_t"
    )]
    let level = level as i8;
    state.compression_level = level;
    let out = state.compressed.insert(Box::new(vec![0; bound(length)]));
    let input: &[u8] = if length == 0 {
        &[]
    } else {
        // SAFETY: the body's span.
        unsafe { std::slice::from_raw_parts(data.cast(), length) }
    };
    match deflate(input, out, window_bits, c_int::from(level), zalloc, zfree) {
        Ok(compressed) => {
            out.truncate(compressed);
            state.content_compressed = 1;
            true
        }
        Err(Failure::Init(status)) => {
            compress_warning(&format!("deflateInit2() failed, Error: [{status}]"));
            false
        }
        Err(Failure::Deflate(status)) => {
            compress_warning(&format!("deflate() failed, Error: [{status}]"));
            false
        }
    }
}

/// The body the last `compress` made.
pub(crate) fn compressed(state: &State) -> &[u8] {
    state.compressed.as_deref().map_or(&[], Vec::as_slice)
}

/// `Context::compress` for the HTTP/2 server: the encoding's name and the
/// compressed body, or `None` when the body goes as it is.
pub(crate) fn compress_body<'a>(
    state: &'a mut State,
    data: &[u8],
) -> Option<(&'static CStr, &'a [u8])> {
    if !compress(state, data.as_ptr().cast(), data.len()) {
        return None;
    }
    let codecs = CAPABILITIES.load(Relaxed) & CODECS;
    let encoding = swr_v1_httpbindings_encoding(i32::from(state.compression_method), codecs);
    // SAFETY: a static NUL-terminated label.
    let encoding = unsafe { CStr::from_ptr(encoding.data.cast()) };
    Some((encoding, compressed(state)))
}

/// brotli or zstd's output (C++, `swr_http_host_compress_other`), copied in
/// as the compressed body.
/// # Safety
/// `context` is a live context and `data` spans `length` bytes.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn swr_v1_phphttp_set_compressed(
    context: *mut c_void,
    data: *const c_char,
    length: usize,
) {
    // SAFETY: per the caller.
    let body = unsafe { std::slice::from_raw_parts(data.cast::<u8>(), length) };
    // SAFETY: per the caller.
    let state = unsafe { &mut *context.cast::<State>() };
    state.compressed = Some(Box::new(body.to_vec()));
    state.content_compressed = 1;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn c_isspace_is_the_c_locale_set() {
        let spaces: Vec<u8> = (0..=255u8).filter(CSpace::is_ascii_whitespace_c).collect();
        assert_eq!(spaces, b"\t\n\x0b\x0c\r ", "isspace in the C locale");
    }
}
