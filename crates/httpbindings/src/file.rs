//! File response range validation.
/// Preserve all defined signed native range comparisons. Rejected subtraction
/// overflow has no defined native reference result; never perform it in Rust.
#[must_use]
pub fn swr_v1_httpbindings_file_range(
    size: i64,
    offset: i64,
    length: i64,
    resolved: Option<&mut i64>,
) -> i32 {
    let Some(resolved) = resolved else {
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
            swr_v1_httpbindings_file_range(i64::MAX, i64::MIN, 0, Some(&mut value)),
            3
        );
        assert_eq!(value, 91);
        assert_eq!(swr_v1_httpbindings_file_range(0, 0, 0, None), -1);
    }
}
