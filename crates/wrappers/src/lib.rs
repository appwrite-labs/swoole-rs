//! Wrapper-local request state and `http_server::listen`'s parser callbacks
//! ([`http`]). The third-party parsers, the `Context`'s containers and IO stay native.
#![cfg_attr(
    not(test),
    deny(
        clippy::undocumented_unsafe_blocks,
        clippy::multiple_unsafe_ops_per_block
    )
)]
#![expect(
    clippy::missing_safety_doc,
    reason = "every export is covered by the module-local C contract"
)]
use std::alloc::{Layout, alloc, dealloc};
use std::{ptr, slice};
mod http;
pub use http::{
    Llhttp, LlhttpSettings, Multipart, MultipartSettings, Native, swr_v1_wrappers_http_settings,
    swr_v1_wrappers_request_bind,
};

#[repr(C)]
pub struct ListenerPlan {
    pub separator: usize,
    pub valid: u32,
    pub default_host: u32,
}

/// Borrowed address bytes for this call only. Native owns strings and port parsing.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn swr_v1_wrappers_listener_plan(
    data: *const u8,
    len: usize,
) -> ListenerPlan {
    // SAFETY: `data` lends `len` initialized bytes (or is unused when `len` is 0) for this call.
    let separator = unsafe { bytes(data, len) }
        .and_then(|address| address.iter().position(|&c| c == b':'))
        .unwrap_or(len);
    ListenerPlan {
        separator,
        valid: u32::from(separator < len),
        default_host: u32::from(separator == 0 && separator < len),
    }
}

/// Next native response effect, using only facts copied after the preceding effect.
/// No native Context, Server, buffer or callback pointer enters Rust.
#[unsafe(no_mangle)]
pub extern "C" fn swr_v1_wrappers_response_next(
    previous: u32,
    length: usize,
    succeeded: u32,
    keepalive: u32,
) -> u32 {
    match previous {
        0 => 1, // start -> send header
        1 if succeeded == 0 => 6,
        1 if length != 0 => 2, // successful header -> send nonempty body
        1 | 2 if succeeded != 0 => 3, // observe keepalive after successful sends
        3 if keepalive == 0 => 4, // close, then return without rereading Context
        3 | 4 => 5,
        _ => 6,
    }
}

#[repr(C)]
#[derive(Default)]
pub struct EventInitPlan {
    pub accepted: u32,
    pub wait_exit: u32,
    pub initializers: [u32; 3],
}

/// Copied readiness/flag facts only. Native allocation, initializer calls and
/// final `SwooleTG` publication occur outside Rust using the original local owner.
#[unsafe(no_mangle)]
pub extern "C" fn swr_v1_wrappers_event_init(
    flags: u32,
    wait_exit_mask: u32,
    ready: u32,
) -> EventInitPlan {
    if ready == 0 {
        return EventInitPlan::default();
    }
    EventInitPlan {
        accepted: 1,
        wait_exit: u32::from(flags & wait_exit_mask != 0),
        initializers: [1, 2, 3],
    }
}

/// Allocation-free decimal wire value. The caller supplies live writable output bytes;
/// return `usize::MAX` without publication if capacity is insufficient. No terminator.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn swr_v1_wrappers_content_length(
    length: usize,
    out: *mut u8,
    capacity: usize,
) -> usize {
    let mut digits = [0_u8; usize::BITS as usize];
    let mut start = digits.len();
    let mut remaining = length;
    loop {
        start -= 1;
        digits[start] = b"0123456789"[remaining % 10];
        remaining /= 10;
        if remaining == 0 {
            break;
        }
    }
    let count = digits.len() - start;
    if out.is_null() || capacity < count {
        return usize::MAX;
    }
    // SAFETY: `out` is non-null and the caller lends `capacity >= count` writable bytes,
    // disjoint from the local `digits`, whose last `count` bytes start at `start`.
    unsafe { ptr::copy_nonoverlapping(digits[start..].as_ptr(), out, count) };
    count
}

#[derive(Default)]
pub struct Request {
    header: Vec<u8>,
    form: Vec<u8>,
    input: Vec<u8>,
    body: Vec<u8>,
    completed: bool,
    started: bool,
    binding: http::Binding,
}
#[repr(C)]
#[derive(Clone, Copy)]
pub struct Span {
    pub data: *const u8,
    pub len: usize,
}
impl Default for Span {
    fn default() -> Self {
        Self::from(&[])
    }
}
impl Span {
    fn from(v: &[u8]) -> Self {
        Self {
            data: v.as_ptr(),
            len: v.len(),
        }
    }
}
#[repr(C)]
#[derive(Default)]
pub struct View {
    pub header: Span,
    pub form: Span,
    pub input: Span,
    pub body: Span,
    pub flags: u32,
}
unsafe fn bytes<'a>(p: *const u8, n: usize) -> Option<&'a [u8]> {
    if n == 0 {
        Some(&[])
    } else if p.is_null() || n > isize::MAX as usize {
        None
    } else {
        // SAFETY: `p` is non-null and the caller lends `n <= isize::MAX` initialized bytes.
        Some(unsafe { slice::from_raw_parts(p, n) })
    }
}
fn assign(dst: &mut Vec<u8>, src: &[u8]) -> i32 {
    if dst
        .try_reserve(src.len().saturating_sub(dst.len()))
        .is_err()
    {
        return -1;
    }
    dst.clear();
    dst.extend_from_slice(src);
    0
}
#[unsafe(no_mangle)]
pub extern "C" fn swr_v1_wrappers_request_new() -> *mut Request {
    #[expect(
        clippy::cast_ptr_alignment,
        reason = "alloc honours Layout::new::<Request>()"
    )]
    // SAFETY: `Request` is not zero-sized; null is handled below.
    let p = unsafe { alloc(Layout::new::<Request>()).cast::<Request>() };
    if !p.is_null() {
        // SAFETY: `p` is a fresh, non-null allocation with `Request`'s layout.
        unsafe { p.write(Request::default()) };
    }
    p
}
#[unsafe(no_mangle)]
pub unsafe extern "C" fn swr_v1_wrappers_request_free(p: *mut Request) {
    if !p.is_null() {
        // SAFETY: `p` is a live handle from `swr_v1_wrappers_request_new`.
        unsafe { (*p).clear_multipart() };
        // SAFETY: `p` is a live handle from `swr_v1_wrappers_request_new`, freed exactly once.
        unsafe { ptr::drop_in_place(p) };
        // SAFETY: `p` came from `alloc` with this layout and its value was just dropped.
        unsafe { dealloc(p.cast(), Layout::new::<Request>()) };
    }
}
#[unsafe(no_mangle)]
pub unsafe extern "C" fn swr_v1_wrappers_request_view(p: *const Request, out: *mut View) -> i32 {
    // SAFETY: `p` is null or a live request handle from `swr_v1_wrappers_request_new`, with no other active use.
    let s = unsafe { p.as_ref() };
    // SAFETY: `out` is null or a writable View.
    let out = unsafe { out.as_mut() };
    let (Some(s), Some(out)) = (s, out) else {
        return -1;
    };
    *out = View {
        header: Span::from(&s.header),
        form: Span::from(&s.form),
        input: Span::from(&s.input),
        body: Span::from(&s.body),
        flags: u32::from(s.completed),
    };
    0
}
#[unsafe(no_mangle)]
pub unsafe extern "C" fn swr_v1_wrappers_header(
    p: *mut Request,
    data: *const u8,
    len: usize,
) -> i32 {
    // SAFETY: `p` is null or a live request handle from `swr_v1_wrappers_request_new`, with no other active use.
    let s = unsafe { p.as_mut() };
    // SAFETY: `data` lends `len` initialized bytes (or is unused when `len` is 0) for this call.
    let data = unsafe { bytes(data, len) };
    let (Some(s), Some(data)) = (s, data) else {
        return -1;
    };
    assign(&mut s.header, data)
}
#[unsafe(no_mangle)]
pub unsafe extern "C" fn swr_v1_wrappers_header_kind(
    p: *const Request,
    method: u32,
    data: *const u8,
    len: usize,
    multipart: u8,
) -> i32 {
    // SAFETY: `p` is null or a live request handle from `swr_v1_wrappers_request_new`, with no other active use.
    let s = unsafe { p.as_ref() };
    // SAFETY: `data` lends `len` initialized bytes (or is unused when `len` is 0) for this call.
    let data = unsafe { bytes(data, len) };
    let (Some(s), Some(data)) = (s, data) else {
        return -1;
    };
    if multipart != 0 {
        if s.header.eq_ignore_ascii_case(b"content-disposition") {
            3
        } else if s.header.eq_ignore_ascii_case(b"swoole-upload-file") {
            4
        } else {
            0
        }
    } else if matches!(method, 0 | 3 | 4 | 28) && s.header.eq_ignore_ascii_case(b"content-type") {
        if data
            .get(..33)
            .is_some_and(|v| v.eq_ignore_ascii_case(b"application/x-www-form-urlencoded"))
        {
            1
        } else if data
            .get(..19)
            .is_some_and(|v| v.eq_ignore_ascii_case(b"multipart/form-data"))
        {
            2
        } else {
            0
        }
    } else {
        0
    }
}
/// Decode the wrapper's Content-Disposition parameters directly into its sole
/// request metadata. Unlike generic cookie callbacks, no native scratch buffer,
/// map, callback or temporary owned name/filename is required. Input bytes must
/// be initialized and disjoint from this request's owned storage.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn swr_v1_wrappers_disposition(
    p: *mut Request,
    data: *const u8,
    len: usize,
) -> i32 {
    // SAFETY: `p` is null or a live request handle from `swr_v1_wrappers_request_new`, with no other active use.
    let state = unsafe { p.as_mut() };
    // SAFETY: `data` lends `len` initialized bytes (or is unused when `len` is 0) for this call.
    let data = unsafe { bytes(data, len) };
    let (Some(state), Some(data)) = (state, data) else {
        return -1;
    };
    // strtok_r/strchr/strlen stop at the first NUL. Quotes and trailing spaces
    // are literal, key matching is case sensitive, and the last duplicate wins.
    let data = &data[..data.iter().position(|&c| c == 0).unwrap_or(data.len())];
    let mut name = None;
    let mut filename = None;
    for token in data.split(|&c| c == b';') {
        let equal = token.iter().position(|&c| c == b'=');
        let mut start = 0;
        // SAFETY: isspace receives an unsigned char value, as C requires.
        while start < token.len() && unsafe { disposition_space(i32::from(token[start])) } != 0 {
            start += 1;
        }
        if start == token.len() || equal == Some(start) {
            continue;
        }
        let end = equal.unwrap_or(token.len());
        let key = &token[start..end];
        let value = equal.map_or(&b""[..], |at| &token[at + 1..]);
        match key {
            b"name" => name = Some(value),
            b"filename" => filename = Some(value),
            _ => {}
        }
    }
    if let Some(value) = filename {
        assign(&mut state.input, value)
    } else if let Some(value) = name {
        assign(&mut state.form, value)
    } else {
        // Keep the existing selected rejection for the pinned wrapper's missing
        // name end-iterator dereference. This undefined baseline is not parity.
        -1
    }
}
unsafe extern "C" {
    #[link_name = "isspace"]
    fn disposition_space(value: i32) -> i32;
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn swr_v1_wrappers_body_skip(
    p: *mut Request,
    data: *const u8,
    len: usize,
) -> usize {
    // SAFETY: `p` is null or a live request handle from `swr_v1_wrappers_request_new`, with no other active use.
    let s = unsafe { p.as_mut() };
    // SAFETY: `data` lends `len` initialized bytes (or is unused when `len` is 0) for this call.
    let data = unsafe { bytes(data, len) };
    let (Some(s), Some(data)) = (s, data) else {
        return 0;
    };
    if s.started || data.is_empty() {
        return 0;
    }
    s.started = true;
    data.iter()
        .take_while(|&&c| c == b'\r' || c == b'\n')
        .count()
}
#[unsafe(no_mangle)]
pub unsafe extern "C" fn swr_v1_wrappers_body_append(
    p: *mut Request,
    data: *const u8,
    len: usize,
) -> i32 {
    // SAFETY: `p` is null or a live request handle from `swr_v1_wrappers_request_new`, with no other active use.
    let s = unsafe { p.as_mut() };
    // SAFETY: `data` lends `len` initialized bytes (or is unused when `len` is 0) for this call.
    let data = unsafe { bytes(data, len) };
    let (Some(s), Some(data)) = (s, data) else {
        return -1;
    };
    if s.body.try_reserve(data.len()).is_err() {
        return -1;
    }
    s.body.extend_from_slice(data);
    0
}
#[unsafe(no_mangle)]
pub unsafe extern "C" fn swr_v1_wrappers_part_end(p: *mut Request) {
    // SAFETY: `p` is null or a live request handle from `swr_v1_wrappers_request_new`, with no other active use.
    if let Some(s) = unsafe { p.as_mut() } {
        if !s.form.is_empty() {
            s.body.clear();
        }
        s.header.clear();
        s.input.clear();
        s.form.clear();
    }
}
#[unsafe(no_mangle)]
pub unsafe extern "C" fn swr_v1_wrappers_complete(p: *mut Request) {
    // SAFETY: `p` is null or a live request handle from `swr_v1_wrappers_request_new`, with no other active use.
    if let Some(s) = unsafe { p.as_mut() } {
        s.completed = true;
    }
}
#[unsafe(no_mangle)]
pub unsafe extern "C" fn swr_v1_wrappers_split(
    data: *const u8,
    len: usize,
    delimiter: u8,
) -> usize {
    // SAFETY: `data` lends `len` initialized bytes (or is unused when `len` is 0) for this call.
    unsafe { bytes(data, len) }.map_or(len, |s| {
        s.iter().position(|&c| c == delimiter).unwrap_or(len)
    })
}
#[unsafe(no_mangle)]
pub unsafe extern "C" fn swr_v1_wrappers_header_wire(
    key: *const u8,
    kn: usize,
    value: *const u8,
    vn: usize,
    out: *mut u8,
    cap: usize,
) -> usize {
    // SAFETY: `key` lends `kn` initialized bytes (or is unused when `kn` is 0) for this call.
    let key = unsafe { bytes(key, kn) };
    // SAFETY: `value` lends `vn` initialized bytes (or is unused when `vn` is 0) for this call.
    let value = unsafe { bytes(value, vn) };
    let (Some(key), Some(value)) = (key, value) else {
        return 0;
    };
    let key = &key[..key.iter().position(|&c| c == 0).unwrap_or(key.len())];
    let value = &value[..value.iter().position(|&c| c == 0).unwrap_or(value.len())];
    if cap == 0 {
        return key.len().saturating_add(value.len()).saturating_add(4);
    }
    if out.is_null() || cap > isize::MAX as usize {
        return 0;
    }
    // SAFETY: `out` is non-null, `cap <= isize::MAX`, and the caller lends `cap` writable
    // bytes disjoint from `key` and `value`.
    let dest = unsafe { slice::from_raw_parts_mut(out, cap) };
    let mut n = 0;
    for &c in key
        .iter()
        .chain(b": ")
        .chain(value)
        .chain(b"\r\n")
        .take(cap - 1)
    {
        dest[n] = c;
        n += 1;
    }
    dest[n] = 0;
    n
}
// Preserve native short-circuiting: if_exit is evaluated natively only for wait_exit.
#[unsafe(no_mangle)]
pub extern "C" fn swr_v1_wrappers_event_wait(wait_exit: u8, exit: u8) -> u8 {
    u8::from(wait_exit == 0 || exit == 0)
}
#[unsafe(no_mangle)]
pub extern "C" fn swr_v1_wrappers_event_direction(event: i32) -> i32 {
    match event {
        512 => 1,
        1024 => 2,
        _ => 0,
    }
}

#[cfg(test)]
mod tests;

#[cfg(test)]
mod allocation_failures {
    use super::*;
    use std::alloc::{GlobalAlloc, System};
    use std::cell::Cell;
    thread_local! { static FAIL: Cell<bool> = const { Cell::new(false) }; }
    struct FailOnce;
    fn fail() -> bool {
        FAIL.try_with(|f| f.replace(false)).unwrap_or(false)
    }
    unsafe impl GlobalAlloc for FailOnce {
        unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
            if fail() {
                ptr::null_mut()
            } else {
                unsafe { System.alloc(layout) }
            }
        }
        unsafe fn dealloc(&self, p: *mut u8, layout: Layout) {
            unsafe { System.dealloc(p, layout) }
        }
        unsafe fn realloc(&self, p: *mut u8, layout: Layout, n: usize) -> *mut u8 {
            if fail() {
                ptr::null_mut()
            } else {
                unsafe { System.realloc(p, layout, n) }
            }
        }
    }
    #[global_allocator]
    static ALLOCATOR: FailOnce = FailOnce;
    #[test]
    fn fallible_abi_keeps_prior_state_and_can_be_destroyed() {
        FAIL.with(|f| f.set(true));
        assert!(swr_v1_wrappers_request_new().is_null());
        unsafe {
            let p = swr_v1_wrappers_request_new();
            assert!(!p.is_null());
            assert_eq!(swr_v1_wrappers_header(p, b"old".as_ptr(), 3), 0);
            let larger = [b'x'; 1024];
            FAIL.with(|f| f.set(true));
            assert_eq!(swr_v1_wrappers_header(p, larger.as_ptr(), larger.len()), -1);
            assert_eq!((*p).header, b"old");
            FAIL.with(|f| f.set(true));
            assert_eq!(
                swr_v1_wrappers_body_append(p, larger.as_ptr(), larger.len()),
                -1
            );
            assert_eq!((*p).body, [] as [u8; 0]);
            assert_eq!(
                swr_v1_wrappers_body_append(p, larger.as_ptr(), larger.len()),
                0
            );
            swr_v1_wrappers_request_free(p);
        }
    }
}
