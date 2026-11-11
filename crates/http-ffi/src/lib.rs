//! crates/http's C ABI (`native/http/swoole_rs_http.h`, `swoole_rs_http_context.h`)
//! for the native gates and fuzz targets, and `src/protocol/http.cc`'s helpers under
//! their C++ symbols ([`http_server`]), over crates/http's safe API.
#![cfg_attr(
    not(test),
    deny(
        clippy::undocumented_unsafe_blocks,
        clippy::multiple_unsafe_ops_per_block
    )
)]
use std::cell::UnsafeCell;
use std::ffi::c_char;
use std::mem::MaybeUninit;
use std::{ptr, slice};
use swoole_rs_http::{HttpCookie, HttpSpan, HttpStaticContext, HttpStaticPath, STATUS_CODES};

pub mod http_server;
mod owned_proxy;
pub use owned_proxy::*;
mod request;
pub use request::{
    HttpForm, HttpNative, HttpRequest, HttpString, destroy_multipart_parser, init_multipart_parser,
    parse_multipart_data, request_free, request_new,
};
#[cfg(test)]
mod pin_helpers;

const INVALID: i32 = -1;
const OVERFLOW: i32 = -2;

/// `length` initialized bytes at `data`, which may be null only when empty.
///
/// # Safety
/// A nonempty span is readable and initialized for its length; no borrow escapes.
unsafe fn bytes<'a>(data: *const u8, length: usize) -> Result<&'a [u8], i32> {
    if length > isize::MAX as usize {
        return Err(OVERFLOW);
    }
    if length == 0 {
        return Ok(&[]);
    }
    if data.is_null() {
        return Err(INVALID);
    }
    // SAFETY: non-null, `length <= isize::MAX`; callers lend that many initialized bytes.
    Ok(unsafe { slice::from_raw_parts(data, length) })
}

/// Required encoded capacity, INCLUDING the trailing NUL. No allocation occurs.
/// # Safety
/// Input is readable and output is aligned/writable/disjoint.
pub unsafe fn swr_v1_http_url_capacity(
    data: *const u8,
    length: usize,
    output: Option<&mut usize>,
) -> i32 {
    let Some(output) = output else {
        return INVALID;
    };
    *output = 0;
    // SAFETY: the caller lends `length` initialized bytes at `data` (null only when empty).
    match unsafe { bytes(data, length) }.and_then(swoole_rs_http::url_capacity) {
        Ok(n) => {
            *output = n;
            0
        }
        Err(status) => status,
    }
}
/// Encode into disjoint native-owned storage, terminating it. No mutation on error.
/// # Safety
/// Input is readable; output capacity is writable, disjoint from input/length.
/// Length output is valid/aligned. No allocation, callback or borrow escapes.
pub unsafe fn swr_v1_http_url_encode(
    data: *const u8,
    length: usize,
    output: *mut u8,
    capacity: usize,
    written: Option<&mut usize>,
) -> i32 {
    let Some(written) = written else {
        return INVALID;
    };
    *written = 0;
    // SAFETY: the caller lends `length` initialized bytes at `data` (null only when empty).
    let input = match unsafe { bytes(data, length) } {
        Ok(input) => input,
        Err(status) => return status,
    };
    let needed = match swoole_rs_http::url_capacity(input) {
        Ok(needed) => needed,
        Err(status) => return status,
    };
    if output.is_null() || capacity < needed {
        return INVALID;
    }
    // SAFETY: non-null, `needed <= capacity`; the caller lends writable, disjoint storage.
    let output = unsafe { slice::from_raw_parts_mut(output.cast::<MaybeUninit<u8>>(), needed) };
    match swoole_rs_http::url_encode(input, output) {
        Ok(length) => {
            *written = length;
            0
        }
        Err(status) => status,
    }
}
/// Decode in place, preserving malformed escapes and decoding '+' as space.
/// # Safety
/// Data is writable for length+1 initialized input bytes; written is disjoint.
pub unsafe fn swr_v1_http_url_decode(
    data: *mut u8,
    length: usize,
    written: Option<&mut usize>,
) -> i32 {
    let Some(written) = written else {
        return INVALID;
    };
    *written = 0;
    if length >= isize::MAX as usize {
        return OVERFLOW;
    }
    if data.is_null() {
        return INVALID;
    }
    // SAFETY: non-null, `length + 1 <= isize::MAX` (checked above) writable initialized bytes.
    let buffer = unsafe { slice::from_raw_parts_mut(data, length + 1) };
    let decoded = swoole_rs_http::url_decode(buffer.get_mut(..length).unwrap_or_default());
    if let Some(nul) = buffer.get_mut(decoded) {
        *nul = 0;
    }
    *written = decoded;
    0
}
/// Find legacy multipart boundary without allocating or retaining input.
/// # Safety
/// Input is readable; output is valid/aligned/disjoint. Status 1=found, 0=absent.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn swr_v1_http_boundary(
    data: *const u8,
    length: usize,
    offset: usize,
    output: Option<&mut HttpSpan>,
) -> i32 {
    let Some(output) = output else {
        return INVALID;
    };
    *output = HttpSpan::default();
    // SAFETY: the caller lends `length` initialized bytes at `data` (null only when empty).
    match unsafe { bytes(data, length) }.and_then(|input| swoole_rs_http::boundary(input, offset)) {
        Ok(Some(span)) => {
            *output = span;
            1
        }
        Ok(None) => 0,
        Err(status) => status,
    }
}

/// # Safety
/// Nonempty input is readable for length bytes. No borrow/allocator escapes.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn swr_v1_http_method(data: *const u8, length: usize) -> i32 {
    // SAFETY: the caller lends `length` initialized bytes at `data` (null only when empty).
    match unsafe { bytes(data, length) } {
        Ok(input) => swoole_rs_http::method(input),
        Err(status) => status,
    }
}
#[unsafe(no_mangle)]
pub extern "C" fn swr_v1_http_method_string(method: i32) -> *const c_char {
    swoole_rs_http::method_string(method).map_or(ptr::null(), core::ffi::CStr::as_ptr)
}

/// Upstream's `list_of_status_code`, which native code may write.
#[used]
#[unsafe(export_name = "_ZN6swoole11http_server19list_of_status_codeE")]
pub static mut NATIVE_STATUS_CODES: [i32; 128] = STATUS_CODES;
thread_local! {
    static UNKNOWN: UnsafeCell<[u8; 128]> = const { UnsafeCell::new([0; 128]) };
}
/// Known messages have immutable static backing. Unknown messages share this
/// thread's 128-byte fallback until the next unknown call, as the pinned function's
/// do. The pointer must not be used after its thread ends.
#[unsafe(no_mangle)]
pub extern "C" fn swr_v2_http_status_message(code: i32) -> *const c_char {
    let message = swoole_rs_http::status_message(code);
    if let std::borrow::Cow::Borrowed(known) = message {
        // The table's messages are C strings; the NUL follows the borrowed bytes.
        return known.as_ptr().cast();
    }
    UNKNOWN.with(|cell| {
        // SAFETY: this thread's array; no reference from an earlier call survives,
        // and this one ends before the pointer returns.
        let unknown = unsafe { &mut *cell.get() };
        let length = message.len().min(unknown.len() - 1);
        if let (Some(to), Some(from)) = (unknown.get_mut(..length), message.get(..length)) {
            to.copy_from_slice(from);
        }
        if let Some(nul) = unknown.get_mut(length) {
            *nul = 0;
        }
        cell.get().cast::<c_char>()
    })
}

/// Classify the mature parser's return after its native execution has ended.
#[must_use]
pub fn swr_v1_http_multipart_result(processed: i64, length: usize, excepted: u32) -> i32 {
    swoole_rs_http::multipart_result(processed, length, excepted)
}
/// Classify the complete native header name before scratch allocation can move it.
/// # Safety
/// The input span is readable for this call. No reference survives the return.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn swr_v1_http_multipart_disposition(data: *const u8, length: usize) -> i32 {
    // SAFETY: the caller lends `length` initialized bytes at `data` (null only when empty).
    match unsafe { bytes(data, length) } {
        Ok(name) => i32::from(swoole_rs_http::multipart_disposition(name)),
        Err(status) => status,
    }
}
/// The cookies of `data` from `cursor` to the NUL that ends them, or `INVALID`
/// when `capacity` ends first.
///
/// # Safety
/// `data` is null or writable through `capacity` bytes, initialized up to the first
/// NUL at or after `cursor`.
unsafe fn cookies<'a>(data: *mut u8, capacity: usize, cursor: usize) -> Result<&'a mut [u8], i32> {
    if data.is_null() || capacity > isize::MAX as usize {
        return Err(INVALID);
    }
    // Only the bytes up to that NUL are initialized: find it before borrowing them.
    let mut end = cursor;
    loop {
        if end >= capacity {
            return Err(INVALID);
        }
        // SAFETY: `end < capacity`, an initialized byte up to the NUL.
        if unsafe { data.wrapping_add(end).read() } == 0 {
            break;
        }
        end += 1;
    }
    // SAFETY: `end < capacity` writable bytes, initialized through the NUL at `end`.
    Ok(unsafe { slice::from_raw_parts_mut(data, end + 1) })
}
/// Consume the destructive cookie iterator through the first filename.
/// # Safety
/// Scratch is writable through capacity<=`isize::MAX` and initialized through a
/// reachable NUL terminator. No reference or owner is retained across native work.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn swr_v1_http_multipart_filename(data: *mut u8, capacity: usize) -> i32 {
    // SAFETY: as the caller guarantees.
    match unsafe { cookies(data, capacity, 0) } {
        Ok(data) => swoole_rs_http::multipart_filename(data),
        Err(status) => status,
    }
}
/// Tokenize one cookie; native invokes its callback after Rust returns.
/// Buffer capacity may contain uninitialized suffix bytes: reads stop at its
/// initialized NUL terminator. Delimiter/'=' mutations match legacy `strtok_r`.
/// # Safety
/// Buffer is writable through capacity<=`isize::MAX`, initialized through a NUL
/// reachable from cursor; cursor/output are valid/aligned/disjoint. Callback may
/// mutate the native buffer between calls, but never retains a Rust reference.
pub unsafe fn swr_v1_http_cookie_next(
    data: *mut u8,
    capacity: usize,
    cursor: Option<&mut usize>,
    output: Option<&mut HttpCookie>,
) -> i32 {
    let (Some(cursor), Some(output)) = (cursor, output) else {
        return INVALID;
    };
    *output = HttpCookie::default();
    // SAFETY: as the caller guarantees.
    let data = match unsafe { cookies(data, capacity, *cursor) } {
        Ok(data) => data,
        Err(status) => return status,
    };
    match swoole_rs_http::cookie_next(data, cursor) {
        Ok(Some(value)) => {
            *output = value;
            1
        }
        Ok(None) => 0,
        Err(status) => status,
    }
}

/// `location_match` for `tests/modules/http/locale_gate.cc`.
/// # Safety
/// Input spans are initialized; aligned state is writable and disjoint. No
/// allocation, callback or retained Rust reference occurs.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn swr_v1_http_static_location(
    url: *const u8,
    url_length: usize,
    location: *const u8,
    location_length: usize,
    state: Option<&mut HttpStaticPath>,
) -> i32 {
    let Some(state) = state else {
        return INVALID;
    };
    // SAFETY: the caller lends `url_length` initialized URL bytes.
    let url = unsafe { bytes(url, url_length) };
    // SAFETY: the caller lends `location_length` initialized location bytes.
    let location = unsafe { bytes(location, location_length) };
    let (url, location) = match (url, location) {
        (Ok(a), Ok(b)) => (a, b),
        (Err(e), _) | (_, Err(e)) => return e,
    };
    match swoole_rs_http::location_match(url, location, state) {
        Ok(found) => i32::from(found),
        Err(error) => error,
    }
}

/// A static context for `tests/modules/http/rewrite_gate.cc`, which frees it with
/// `swr_v2_http_context_drop`; null on failure.
/// # Safety
/// `url` lends `length` initialized bytes (null only when empty).
#[unsafe(no_mangle)]
pub unsafe extern "C" fn swr_v2_http_context_new(
    url: *const u8,
    length: usize,
    capacity: usize,
) -> *mut HttpStaticContext {
    // SAFETY: as the caller guarantees.
    let Ok(url) = (unsafe { bytes(url, length) }) else {
        return ptr::null_mut();
    };
    HttpStaticContext::new(url, capacity)
        .map_or(ptr::null_mut(), |context| Box::into_raw(Box::new(context)))
}
/// # Safety
/// Null or a context from `swr_v2_http_context_new`, dropped once.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn swr_v2_http_context_drop(owner: *mut HttpStaticContext) {
    if !owner.is_null() {
        // SAFETY: a unique, live context from `context_new`, dropped once.
        drop(unsafe { Box::from_raw(owner) });
    }
}
/// Text `kind`'s length into `written`, and its bytes into `output` when `capacity`
/// is nonzero; a short capacity copies nothing.
/// # Safety
/// `owner` is null or live; `written` is writable; `output` holds `capacity`
/// writable bytes disjoint from the context.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn swr_v2_http_context_text(
    owner: Option<&HttpStaticContext>,
    kind: u32,
    output: *mut u8,
    capacity: usize,
    written: Option<&mut usize>,
) -> i32 {
    let Some(text) = owner.and_then(|owner| owner.text(kind)) else {
        return INVALID;
    };
    let Some(written) = written else {
        return INVALID;
    };
    if capacity != 0 {
        if capacity < text.len() || output.is_null() {
            return OVERFLOW;
        }
        // SAFETY: `text.len() <= capacity` writable bytes, disjoint from the context.
        unsafe { ptr::copy_nonoverlapping(text.as_ptr(), output, text.len()) };
    }
    *written = text.len();
    0
}

#[cfg(test)]
mod tests;
