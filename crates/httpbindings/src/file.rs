//! File response range validation.
/// Why a file response's range is refused.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RangeError {
    /// The offset is past the file's end.
    OffsetPastEnd,
    /// The offset is so far below zero that the bytes after it overflow.
    OffsetOverflow,
    /// More bytes are asked for than follow the offset.
    LengthPastEnd,
}

/// The bytes a file response of `length` at `offset` sends from a file of `size`: all
/// that follow the offset when `length` is zero. Preserves every defined signed native
/// range comparison; the subtraction never overflows.
///
/// # Errors
/// The range does not fit the file.
pub fn file_range(size: i64, offset: i64, length: i64) -> Result<i64, RangeError> {
    if size < offset {
        return Err(RangeError::OffsetPastEnd);
    }
    let remaining = size.checked_sub(offset).ok_or(RangeError::OffsetOverflow)?;
    if length > remaining {
        return Err(RangeError::LengthPastEnd);
    }
    Ok(if length == 0 { remaining } else { length })
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn invalid_ranges_are_refused() {
        assert_eq!(
            file_range(i64::MAX, i64::MIN, 0),
            Err(RangeError::OffsetOverflow)
        );
        assert_eq!(file_range(1, 2, 0), Err(RangeError::OffsetPastEnd));
        assert_eq!(file_range(4, 1, 4), Err(RangeError::LengthPastEnd));
        assert_eq!(file_range(4, 1, 0), Ok(3));
    }
}
