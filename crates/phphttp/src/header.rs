//! The HTTP/1 response header and trailer, and body compression, as
//! `swoole::http::Context::build_header`, `build_trailer` and `compress` did
//! them. Each send leases reusable storage until its IO completes.
use crate::parse::CAPABILITIES;
use crate::state::{Slot, State};
use crate::{host, send};
use std::borrow::Cow;
use std::cell::RefCell;
use std::ffi::{CStr, c_char, c_int, c_void};
use std::ptr;
use std::sync::atomic::{AtomicU32, Ordering::Relaxed};
use swoole_rs_deflate::{Allocator, DEFLATE, Failure, GZIP, bound, compress as deflate};
use swoole_rs_httpbindings::{
    CONNECTION, CONTENT_ENCODING, CONTENT_LENGTH, CONTENT_TYPE, DATE, SERVER, TRANSFER_ENCODING,
    encoding, response_header, swr_v1_httpbindings_codec, swr_v1_httpbindings_codec_level,
};
use swoole_rs_inflate::brotli::{self, MAX_QUALITY, MIN_QUALITY};
use swoole_rs_inflate::zstd;
use swoole_rs_zend::sys::php_format_date;
use swoole_rs_zend::sys::{
    swr_zend_efree, swr_zend_emalloc, swr_zend_http_get_string, swr_zend_http_property_offset,
    swr_zend_string_release, zend_class_entry, zend_string, zval,
};
use swoole_rs_zend::{Array, Key, Value, ZStr, ZString, bailout, string};

/// llhttp's `HTTP_HEAD`.
const HEAD: u8 = 2;
/// `SW_LOG_WARNING`, `SW_ERROR_HTTP_CONFLICT_HEADER`.
const LOG_WARNING: c_int = 4;
const CONFLICT_HEADER: c_int = 7105;
/// MINIT's codec bits (`parse.rs`): zlib, Brotli and zstd, and any codec built.
const CODECS: u32 = 7;
const COMPRESSION: u32 = 8;
/// `HTTP_COMPRESS_GZIP`, `HTTP_COMPRESS_BR` and `HTTP_COMPRESS_ZSTD`.
const METHOD_GZIP: i32 = 1;
const METHOD_BR: i32 = 3;
const METHOD_ZSTD: i32 = 4;
/// `sw_tg_buffer()`'s size, which bounds one trailer line.
const TRAILER_LINE: usize = 65536;

/// `Swoole\Http\Response`'s `header`, `cookie` and `trailer` slots.
static OFFSETS: [AtomicU32; 3] = [const { AtomicU32::new(0) }; 3];

php_local! {
    static OUT: RefCell<Vec<u8>> = RefCell::new(Vec::new());
}

/// Each send owns its bytes across yields and reentry; only an idle buffer is cached.
pub(crate) fn buffer<R>(f: impl FnOnce(&mut Vec<u8>) -> R) -> R {
    let mut out = OUT.with_borrow_mut(std::mem::take);
    out.clear();
    let result = f(&mut out);
    OUT.with_borrow_mut(|idle| {
        if out.capacity() <= TRAILER_LINE && out.capacity() > idle.capacity() {
            *idle = out;
        }
    });
    result
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
    let index = usize::try_from(which).unwrap_or(2).min(2);
    let offset = OFFSETS.get(index).map_or(0, |offset| offset.load(Relaxed));
    slot.init_array(&response.zobject, offset, 8)
}

/// `sw_zend_read_property_ex(swoole_http_response_ce, response.zobject, name)`, when it
/// is an array.
/// The context's reference keeps the response object, and so the array, live
/// while its slot is borrowed.
fn property(response: &Slot, which: usize) -> Option<&Array> {
    let object = response.object()?;
    let offset = OFFSETS.get(which).map_or(0, |offset| offset.load(Relaxed)) as usize;
    // OBJ_PROP: a declared slot of that object.
    let slot = object.wrapping_byte_add(offset).cast::<zval>();
    // SAFETY: a declared property slot of the context's live response.
    unsafe { Value::from_ptr(slot) }.and_then(Value::array)
}

/// `swr_zend_http_string`: `zval_get_string`'s argument and result.
#[repr(C)]
struct Arg {
    value: *mut zval,
    result: *mut zend_string,
}

/// `zend::String(value)`: a string zval's own string, or `zval_get_string`'s new one.
enum Text<'a> {
    Borrowed(&'a ZStr),
    Owned(ZString),
}

impl std::ops::Deref for Text<'_> {
    type Target = ZStr;

    fn deref(&self) -> &ZStr {
        match self {
            Self::Borrowed(text) => text,
            Self::Owned(text) => text,
        }
    }
}

impl Text<'_> {
    /// A reference of its own (`zval_get_string`'s result keeps the one it has).
    fn into_owned(self) -> ZString {
        match self {
            Self::Borrowed(text) => text.to_owned(),
            Self::Owned(text) => text,
        }
    }
}

/// `zend::String(value)`, with `zval_get_string` run in `zend_try` because it may call
/// `__toString`.
fn to_string(value_: &Value) -> Result<Text<'_>, bailout::Bailout> {
    if let Some(text) = value_.str() {
        return Ok(Text::Borrowed(text));
    }
    let mut arg = Arg {
        value: value_.as_ptr(),
        result: ptr::null_mut(),
    };
    // SAFETY: a C body over a live zval and a writable result.
    unsafe { bailout::trap(swr_zend_http_get_string, (&raw mut arg).cast())? };
    // SAFETY: `zval_get_string`'s new reference, which the `ZString` takes.
    Ok(Text::Owned(unsafe { ZString::from_raw(arg.result) }))
}

/// Each element of an array (`ZEND_HASH_FOREACH_KEY_VAL`): `Ok(key)` for a string
/// key, `Err(index)` for an integer key. Stops at the first `Err`.
fn each<'a, E>(
    array: &'a Array,
    mut body: impl FnMut(Result<&'a [u8], i64>, &'a Value) -> Result<(), E>,
) -> Result<(), E> {
    let mut entries = array.entries();
    while let Some((key, data)) = entries.next_keyed() {
        let key = match key {
            Key::Name(name) => Ok(name),
            Key::Index(index) => Err(i64::from_ne_bytes(index.to_ne_bytes())),
        };
        body(key, data)?;
    }
    Ok(())
}

php_local! {
    /// `php_swoole_http_get_date`'s per-second cache: the time and its text.
    static DATE_CACHE: std::cell::Cell<(libc::time_t, *mut zend_string)> =
        std::cell::Cell::new((0, ptr::null_mut()));
}

/// The `Date` header for now, formatted at most once a second.
pub(crate) fn date() -> *mut zend_string {
    let now = swoole_rs_corebase::time_now();
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
pub extern "C" fn response_rshutdown() {
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
    value_: &Value,
) -> Result<(), bailout::Bailout> {
    if value_.is_null() {
        return Ok(());
    }
    let text = to_string(value_)?;
    let length = text.bytes().len();
    let trimmed = text
        .bytes()
        .iter()
        .rposition(|byte| !byte.is_ascii_whitespace_c())
        .map_or(0, |last| last + 1);
    if trimmed != length {
        let val = string::val(text.as_ptr());
        // SAFETY: the string holds `length` bytes; `swoole::rtrim` zeroes the trimmed ones.
        unsafe { ptr::write_bytes(val.wrapping_add(trimmed), 0, length - trimmed) };
        // SAFETY: as above: `ZSTR_LEN(str) = swoole::rtrim(...)`.
        unsafe { (*text.as_ptr()).len = trimmed };
    }
    let bytes = text.bytes();
    if !has_crlf(bytes) {
        out.extend_from_slice(key);
        out.extend_from_slice(b": ");
        out.extend_from_slice(bytes);
        out.extend_from_slice(b"\r\n");
    }
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

/// The header's key and the header flag it sets (0 for a custom header); an integer
/// key as Zend formats it.
fn header_key(key: Result<&[u8], i64>) -> (Cow<'_, [u8]>, i32) {
    let key = key.map_or_else(
        |index| Cow::Owned(index.to_string().into_bytes()),
        Cow::Borrowed,
    );
    let flag = response_header(&key);
    (key, flag)
}

/// `value` in decimal, written into the end of `buffer` (`swoole_itoa`);
/// twenty bytes hold any i64. Not `write!`: `core::fmt` costs four times as much
/// on every response header.
#[must_use]
pub fn decimal(buffer: &mut [u8; 20], value: i64) -> &[u8] {
    let mut rest = value.unsigned_abs();
    let mut start = buffer.len();
    for slot in buffer.iter_mut().rev() {
        *slot = b'0' + (rest % 10) as u8;
        start -= 1;
        rest /= 10;
        if rest == 0 {
            break;
        }
    }
    if value < 0
        && let Some(sign) = start.checked_sub(1)
        && let Some(slot) = buffer.get_mut(sign)
    {
        *slot = b'-';
        start = sign;
    }
    buffer.get(start..).unwrap_or_default()
}

fn build(
    state: &mut State,
    body: *const c_char,
    mut length: usize,
    out: &mut Vec<u8>,
) -> Result<(), bailout::Bailout> {
    let compression = CAPABILITIES.load(Relaxed) & COMPRESSION != 0;
    // http status line
    out.extend_from_slice(b"HTTP/1.1 ");
    if state.response.reason.is_null() {
        out.extend_from_slice(&swoole_rs_http::status_message(state.response.status));
    } else {
        out.extend_from_slice(decimal(&mut [0; 20], state.response.status.into()));
        out.push(b' ');
        // SAFETY: `status()` stored a NUL-terminated reason.
        out.extend_from_slice(unsafe { CStr::from_ptr(state.response.reason) }.to_bytes());
    }
    out.extend_from_slice(b"\r\n");

    // http headers
    let mut flags = 0;
    if let Some(header) = property(&state.response.zobject, 0) {
        // Asked once, like upstream's `compression_types` test.
        let typed = compression && crate::context::compression_type(state, None);
        let mut content_type: Option<ZString> = None;
        let walked = each(header, |key, value_| {
            let (key, flag) = header_key(key);
            if flag > 0 {
                if compression {
                    if flag == CONTENT_TYPE && state.accept_compression != 0 && typed {
                        content_type = Some(to_string(value_)?.into_owned());
                    }
                    if flag == CONTENT_ENCODING && value_.str().is_some() {
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
                if value_.bytes().is_some_and(<[u8]>::is_empty) {
                    return Ok(());
                }
            }
            if let Some(items) = value_.array() {
                each(items, |_, item| add_custom_header(out, &key, item))
            } else {
                add_custom_header(out, &key, value_)
            }
        });
        if walked.is_ok() && compression && state.accept_compression != 0 && typed {
            let kind = content_type
                .as_ref()
                .map_or(&b"text/html"[..], |text| text.bytes());
            state.accept_compression =
                u8::from(crate::context::compression_type(state, Some(kind)));
        }
        walked?;
    }

    // http cookies
    if let Some(cookie) = property(&state.response.zobject, 1) {
        each::<()>(cookie, |_, value_| {
            if let Some(line) = value_.bytes()
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
            let label = encoding(i32::from(state.compression_method), codecs);
            out.extend_from_slice(b"Content-Encoding: ");
            out.extend_from_slice(label.map_or(b"".as_slice(), CStr::to_bytes));
            out.extend_from_slice(b"\r\n");
        }
        if flags & CONTENT_LENGTH == 0 {
            out.extend_from_slice(b"Content-Length: ");
            let length = i64::try_from(length).unwrap_or(i64::MAX);
            out.extend_from_slice(decimal(&mut [0; 20], length));
            out.extend_from_slice(b"\r\n");
        }
    }
    out.extend_from_slice(b"\r\n");
    state.send_header = 1;
    Ok(())
}

/// `Context::build_header` of a `length`-byte body (compressing it when the
/// response asks to), then `f(header)`. The buffer stays owned until `f`
/// returns, including when sending yields to another request.
/// # Errors
/// A header value's `__toString` bailed out.
pub(crate) fn header<R>(
    ctx: *mut host::Context,
    body: *const c_char,
    length: usize,
    f: impl FnOnce(&mut Vec<u8>) -> R,
) -> Result<R, bailout::Bailout> {
    buffer(|out| {
        build(send::state(ctx), body, length, out)?;
        Ok(f(out))
    })
}

/// `Context::build_trailer`: `name: value` lines and the closing CRLF, then
/// `f(trailer)`, as [`header`].
/// # Errors
/// A trailer value's `__toString` bailed out.
pub(crate) fn trailer<R>(
    ctx: *mut host::Context,
    f: impl FnOnce(&[u8]) -> R,
) -> Result<R, bailout::Bailout> {
    buffer(|out| {
        if let Some(trailer) = property(&send::state(ctx).response.zobject, 2) {
            each(trailer, |key, value_| {
                let Ok(name) = key else { return Ok(()) };
                if value_.is_null() {
                    return Ok(());
                }
                let text = to_string(value_)?;
                let value_ = text.bytes();
                let mut line = Vec::with_capacity(name.len() + value_.len() + 4);
                // `%.*s` stops at a NUL, and sw_snprintf at its buffer.
                line.extend_from_slice(name.split(|&b| b == 0).next().unwrap_or_default());
                line.extend_from_slice(b": ");
                line.extend_from_slice(value_.split(|&b| b == 0).next().unwrap_or_default());
                line.extend_from_slice(b"\r\n");
                line.truncate(TRAILER_LINE - 1);
                out.extend_from_slice(&line);
                Ok(())
            })?;
        }
        out.extend_from_slice(b"\r\n");
        Ok(f(out))
    })
}

extern "C" fn zalloc(_: *mut c_void, items: u32, size: u32) -> *mut c_void {
    // `safe_emalloc(items, size, 0)`; an overflow fails the allocation.
    let Some(bytes) = (items as usize).checked_mul(size as usize) else {
        return ptr::null_mut();
    };
    // SAFETY: PHP's request allocator.
    unsafe { swr_zend_emalloc(bytes) }
}

unsafe extern "C" fn zfree(_: *mut c_void, address: *mut c_void) {
    // SAFETY: memory `zalloc` returned.
    unsafe { swr_zend_efree(address) };
}

/// zlib allocating from PHP's request allocator, as upstream's `php_zlib_alloc`.
// SAFETY: `zalloc` returns null or `items * size` bytes of emalloc'd memory,
// which `zfree` efrees.
pub(crate) const ZEND_ZLIB: Allocator = unsafe { Allocator::new(zalloc, zfree) };

fn compress_warning(message: &str) {
    host::log(0, 0, c"Context::compress", message);
}

/// `Context::compress`.
fn compress(state: &mut State, data: *const c_char, length: usize) -> bool {
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
    let input: &[u8] = if length == 0 {
        &[]
    } else {
        // SAFETY: the body's span.
        unsafe { std::slice::from_raw_parts(data.cast(), length) }
    };
    let level = i32::from(state.compression_level);
    let compressed = match method {
        METHOD_BR => {
            let quality = swr_v1_httpbindings_codec_level(method, level, MIN_QUALITY, MAX_QUALITY);
            // The Brotli quality is stored back, as upstream's clamp does; it fits int8_t.
            state.compression_level = i8::try_from(quality).unwrap_or(0);
            brotli::compress(input, quality)
                .ok_or_else(|| "BrotliEncoderCompress() failed".to_owned())
        }
        METHOD_ZSTD => {
            let (minimum, maximum) = zstd::levels();
            zstd::compress(
                input,
                swr_v1_httpbindings_codec_level(method, level, minimum, maximum),
            )
        }
        _ => zlib(state, input, method, level),
    };
    match compressed {
        Ok(out) => {
            state.compressed = Some(Box::new(out));
            state.content_compressed = 1;
            true
        }
        Err(message) => {
            compress_warning(&message);
            false
        }
    }
}

/// `Context::compress`'s gzip and deflate: one `deflate(Z_FINISH)` into a
/// buffer of `bound(length)`, at the clamped level stored back.
fn zlib(state: &mut State, input: &[u8], method: i32, level: i32) -> Result<Vec<u8>, String> {
    let window_bits = if method == METHOD_GZIP { GZIP } else { DEFLATE };
    let level = swr_v1_httpbindings_codec_level(method, level, 0, 0);
    // zlib's clamp lands in -1..=9, which fits int8_t.
    state.compression_level = i8::try_from(level).unwrap_or(-1);
    match deflate(input, bound(input.len()), window_bits, level, ZEND_ZLIB) {
        Ok(out) => Ok(out),
        Err(Failure::Init(status)) => Err(format!("deflateInit2() failed, Error: [{status}]")),
        Err(Failure::Deflate(status)) => Err(format!("deflate() failed, Error: [{status}]")),
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
    let label = encoding(i32::from(state.compression_method), codecs).unwrap_or_default();
    Some((label, compressed(state)))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn decimal_matches_display() {
        for value in [0, 7, 200, 404, 1_048_576, -1, -503, i64::MAX, i64::MIN] {
            assert_eq!(decimal(&mut [0; 20], value), value.to_string().as_bytes());
        }
    }

    #[test]
    fn output_survives_reentry_and_returns_on_error() {
        let result: Result<(), ()> = buffer(|outer| {
            outer.extend_from_slice(b"pending response");
            buffer(|inner| inner.extend_from_slice(b"another coroutine"));
            assert_eq!(outer, b"pending response");
            Err(())
        });
        assert!(result.is_err());
        buffer(|out| assert!(out.is_empty() && out.capacity() >= b"pending response".len()));
        buffer(|out| out.resize(TRAILER_LINE + 1, 0));
        OUT.with_borrow(|idle| assert!(idle.capacity() <= TRAILER_LINE));
    }

    #[test]
    fn c_isspace_is_the_c_locale_set() {
        let spaces: Vec<u8> = (0..=255u8).filter(CSpace::is_ascii_whitespace_c).collect();
        assert_eq!(spaces, b"\t\n\x0b\x0c\r ", "isspace in the C locale");
    }
}
