//! Public status pointers: immutable known messages and one Rust-owned mutable
//! fallback per thread. Native receives a raw pointer after all Rust borrows end.
use std::{cell::UnsafeCell, ffi::c_char};
// Exact existing public C++ int[128] ABI on the gated Itanium/int32 targets.
// Native external users may mutate it; Rust never constructs an aliased reference.
// #[used] retains the data definition; actual linker/export qualification remains
// an explicit separate gate rather than an assumption from this source attribute.
#[used]
#[unsafe(export_name = "_ZN6swoole11http_server19list_of_status_codeE")]
pub static mut NATIVE_STATUS_CODES: [i32; 128] = [
    100, 101, 102, 103, 200, 201, 202, 203, 204, 205, 206, 207, 208, 226, 300, 301, 302, 303, 304,
    305, 306, 307, 308, 400, 401, 402, 403, 404, 405, 406, 407, 408, 409, 410, 411, 412, 413, 414,
    415, 416, 417, 418, 421, 422, 423, 424, 425, 426, 428, 429, 431, 451, 500, 501, 502, 503, 504,
    505, 506, 507, 508, 510, 511, -1, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
    0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
    0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
];
thread_local! {
    static UNKNOWN: UnsafeCell<[u8;128]> = const { UnsafeCell::new([0;128]) };
}
/// Known messages have immutable static backing. Unknown messages share this
/// thread's128-byte fallback until the next unknown call, matching the original
/// Native function. No allocator/callback or Native effect is invoked from Rust.
/// The raw pointer must not be used after its thread ends or through a Rust
/// reference that survives a subsequent mutation. No cross-thread lifetime or
/// concurrent mutation qualification is implied by returning a C pointer.
#[unsafe(no_mangle)]
pub extern "C" fn swr_v2_http_status_message(code: i32) -> *const c_char {
    let known = crate::swr_v1_http_status(code);
    if !known.is_null() {
        return known;
    }
    UNKNOWN.with(|cell| {
        // SAFETY: this thread's array; no reference from an earlier call survives,
        // and this one ends before the pointer returns.
        crate::dispatch::unknown_status(code, unsafe { &mut *cell.get() });
        cell.get().cast::<c_char>()
    })
}
#[cfg(test)]
mod tests {
    use super::*;
    use std::ffi::CStr;
    #[test]
    fn known_identity_and_repeated_unknown_backing_are_preserved() {
        let first = swr_v2_http_status_message(-701);
        assert_eq!(
            unsafe { CStr::from_ptr(first) }.to_bytes(),
            b"-701 Unknown Status"
        );
        assert_eq!(
            swr_v2_http_status_message(200),
            crate::swr_v1_http_status(200)
        );
        assert_eq!(
            unsafe { CStr::from_ptr(first) }.to_bytes(),
            b"-701 Unknown Status"
        );
        let next = swr_v2_http_status_message(-702);
        assert_eq!(first, next);
        assert_eq!(
            unsafe { CStr::from_ptr(first) }.to_bytes(),
            b"-702 Unknown Status"
        );
    }
}
