//! File response range validation.
/// Preserve all defined signed native range comparisons. Rejected subtraction
/// overflow has no defined native reference result; never perform it in Rust.
/// # Safety
/// `resolved` is initialized writable caller storage for this call only.
pub unsafe fn swr_v1_httpbindings_file_range(
    size: i64,
    offset: i64,
    length: i64,
    resolved: *mut i64,
) -> i32 {
    // SAFETY: `resolved` is null or aligned, writable and exclusively lent for this call.
    let Some(resolved) = (unsafe { resolved.as_mut() }) else {
        return -1;
    };
    if size < offset {
        return 1;
    }
    let Some(remaining) = size.checked_sub(offset) else {
        return 3;
    };
    if length > remaining {
        return 2;
    }
    *resolved = if length == 0 { remaining } else { length };
    0
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn invalid_ranges_and_cursors_do_not_publish_a_result() {
        let mut value = 91;
        assert_eq!(
            unsafe { swr_v1_httpbindings_file_range(i64::MAX, i64::MIN, 0, &raw mut value) },
            3
        );
        assert_eq!(value, 91);
        assert_eq!(
            unsafe { swr_v1_httpbindings_file_range(0, 0, 0, std::ptr::null_mut()) },
            -1
        );
    }
}
