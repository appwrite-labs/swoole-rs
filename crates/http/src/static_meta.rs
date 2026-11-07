//! Inline static handler policy; native owns strings, RNG and MIME references.
use crate::proxy::HttpProxyText;
use crate::{INVALID, OVERFLOW, bytes, c_text};
use std::{mem::MaybeUninit, slice};
pub(crate) const BOUNDARY: &[u8] = b"SwooleBoundary\0";
const CONTENT_TYPE: &[u8] = b"multipart/byteranges; boundary=\0";

/// Return the lazy boundary initializer, or NULL for an existing boundary.
/// # Safety
/// `random_length` is aligned/writable; the returned string has static lifetime.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn swr_v1_http_static_boundary(
    length: usize,
    random_length: *mut usize,
) -> *const u8 {
    if length != 0 || random_length.is_null() {
        return std::ptr::null();
    }
    // SAFETY: `random_length` is non-null (checked above), aligned and writable per the caller contract.
    unsafe { random_length.write(39 - BOUNDARY.len()) };
    BOUNDARY.as_ptr()
}
/// Multipart prefix for multiple tasks, otherwise NULL (native MIME reference).
#[unsafe(no_mangle)]
pub extern "C" fn swr_v1_http_static_content_prefix(tasks: usize) -> *const u8 {
    if tasks > 1 {
        CONTENT_TYPE.as_ptr()
    } else {
        std::ptr::null()
    }
}
/// Query/write the binary-safe multipart content type, excluding any NUL.
/// # Safety
/// Input spans are initialized. Output and written are writable and disjoint
/// from inputs. Native string allocation completes before this call.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn swr_v1_http_static_content_type(
    boundary: *const HttpProxyText,
    output: *mut u8,
    capacity: usize,
    written: *mut usize,
) -> i32 {
    if boundary.is_null() || written.is_null() || (capacity != 0 && output.is_null()) {
        return INVALID;
    }
    // SAFETY: `boundary` is non-null (checked above), aligned and initialized per the caller contract.
    let boundary = unsafe { &*boundary };
    // SAFETY: the boundary span names `length` initialized bytes (null only when empty).
    let boundary = match unsafe { bytes(boundary.data, boundary.length) } {
        Ok(v) => v,
        Err(e) => return e,
    };
    let prefix = c_text(CONTENT_TYPE);
    let Some(length) = prefix
        .len()
        .checked_add(boundary.len())
        .filter(|&n| isize::try_from(n).is_ok())
    else {
        return OVERFLOW;
    };
    if capacity != 0 {
        if capacity < length || capacity > isize::MAX as usize {
            return OVERFLOW;
        }
        // SAFETY: non-null (checked above); `length <= capacity <= isize::MAX` writable bytes.
        let output = unsafe { slice::from_raw_parts_mut(output.cast::<MaybeUninit<u8>>(), length) };
        for (slot, &byte) in output.iter_mut().zip(prefix.iter().chain(boundary)) {
            slot.write(byte);
        }
    }
    // SAFETY: `written` is non-null (checked above), aligned and writable per the caller contract.
    unsafe { written.write(length) };
    0
}
/// Publish404 only for the terminal location match; no native callback occurs.
/// # Safety
/// status is aligned, initialized and exclusively writable.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn swr_v1_http_static_catch(last: u32, status: *mut i32) -> i32 {
    if status.is_null() {
        return INVALID;
    }
    if last == 0 {
        return 0;
    }
    // SAFETY: `status` is non-null (checked above), aligned and writable per the caller contract.
    unsafe { status.write(404) };
    1
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn binary_content_and_rejected_small_buffer_do_not_publish_partial_results() {
        let boundary = HttpProxyText {
            data: b"a\0b".as_ptr(),
            length: 3,
        };
        let mut output = [0xa5; 64];
        let mut written = 99;
        assert_eq!(
            unsafe {
                swr_v1_http_static_content_type(
                    &raw const boundary,
                    output.as_mut_ptr(),
                    1,
                    &raw mut written,
                )
            },
            OVERFLOW
        );
        assert_eq!(output, [0xa5; 64]);
        assert_eq!(written, 99);
        assert_eq!(
            unsafe {
                swr_v1_http_static_content_type(
                    &raw const boundary,
                    output.as_mut_ptr(),
                    output.len(),
                    &raw mut written,
                )
            },
            0
        );
        assert_eq!(&output[..written], b"multipart/byteranges; boundary=a\0b");
        assert_eq!(output[written], 0xa5);
    }
}
