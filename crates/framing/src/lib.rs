//! The length/EOF receive loops, packet-length helpers and the MIME table;
//! native code keeps the socket IO, callbacks and buffer allocation.
#![forbid(unsafe_code)]
#![expect(
    clippy::disallowed_types,
    reason = "existing uses; tools/budget.py counts them down (docs/migration.md#data-layout)"
)]
pub mod mime;

pub mod recv;
pub use recv::Protocol;

/// The bytes `swoole_unpack` reads for `format`; 0 for an unknown one.
#[must_use]
pub fn width(format: u8) -> usize {
    match format {
        b'c' | b'C' => 1,
        b's' | b'S' | b'n' | b'v' => 2,
        b'i' | b'I' | b'l' | b'L' | b'N' | b'V' => 4,
        b'q' | b'Q' | b'J' | b'P' => 8,
        _ => 0,
    }
}

/// Why [`unpack`] read no value.
#[derive(Debug, PartialEq, Eq)]
pub enum UnpackError {
    /// Not one of the sixteen formats.
    Format,
    /// `data` is shorter than the format's [`width`].
    Short,
}

/// `swoole_unpack`: the value of `format` at the start of `data`.
/// # Errors
/// An unknown format, or fewer bytes than its width.
pub fn unpack(format: u8, data: &[u8]) -> Result<i64, UnpackError> {
    let width = width(format);
    if width == 0 {
        return Err(UnpackError::Format);
    }
    let field = data.get(..width).ok_or(UnpackError::Short)?;
    let mut raw = [0u8; 8];
    for (slot, &byte) in raw.iter_mut().zip(field) {
        *slot = byte;
    }
    Ok(match format {
        b'c' => i64::from(raw[0].cast_signed()),
        b'C' => i64::from(raw[0]),
        b's' => i64::from(i16::from_ne_bytes([raw[0], raw[1]])),
        b'S' => i64::from(u16::from_ne_bytes([raw[0], raw[1]])),
        b'n' => i64::from(u16::from_be_bytes([raw[0], raw[1]])),
        b'v' => i64::from(u16::from_le_bytes([raw[0], raw[1]])),
        b'i' | b'l' => i64::from(i32::from_ne_bytes([raw[0], raw[1], raw[2], raw[3]])),
        b'I' | b'L' => i64::from(u32::from_ne_bytes([raw[0], raw[1], raw[2], raw[3]])),
        b'N' => i64::from(u32::from_be_bytes([raw[0], raw[1], raw[2], raw[3]])),
        b'V' => i64::from(u32::from_le_bytes([raw[0], raw[1], raw[2], raw[3]])),
        b'q' | b'Q' => i64::from_ne_bytes(raw),
        b'J' => u64::from_be_bytes(raw).cast_signed(),
        b'P' => u64::from_le_bytes(raw).cast_signed(),
        _ => return Err(UnpackError::Format),
    })
}

/// What [`default_length`] found.
#[derive(Debug, PartialEq, Eq)]
pub enum DefaultLength {
    /// The length field isn't all here: the bytes needed to read it.
    Need(i64),
    /// The package's total length: the field's value plus the body offset.
    Total(i64),
    /// A zero size, a field past the bytes, or a negative or overflowing length.
    Invalid,
    /// An unknown length type.
    Format,
}

/// `Protocol::default_length_func` over the received `data`: the length field of
/// type `format` and `size` bytes at `offset`, plus `body_offset`.
#[must_use]
pub fn default_length(
    format: u8,
    data: &[u8],
    offset: u32,
    size: u32,
    body_offset: u32,
) -> DefaultLength {
    if size == 0 {
        return DefaultLength::Invalid;
    }
    let need = offset as usize + size as usize;
    if data.len() < need {
        return DefaultLength::Need(i64::try_from(need).unwrap_or(i64::MAX));
    }
    if width(format) == 0 {
        return DefaultLength::Format;
    }
    let Some(field) = data.get(offset as usize..) else {
        return DefaultLength::Invalid;
    };
    match unpack(format, field) {
        Ok(value) if value >= 0 => value
            .checked_add(i64::from(body_offset))
            .map_or(DefaultLength::Invalid, DefaultLength::Total),
        _ => DefaultLength::Invalid,
    }
}

#[cfg(test)]
mod tests {
    //! The value checks of the C ABI gate this crate's exports had
    //! (`tests/modules/framing/gate.cc`), with the same inputs and results.
    use super::*;

    #[test]
    fn unpack_rejects_unknown_formats_and_short_fields() {
        assert_eq!(width(b'?'), 0);
        assert_eq!(unpack(b'?', &[]), Err(UnpackError::Format));
        assert_eq!(unpack(b'N', &[0; 3]), Err(UnpackError::Short));
        assert_eq!(unpack(b'C', &[]), Err(UnpackError::Short));
    }

    #[test]
    fn default_length_outcomes() {
        let data = [0u8; 16];
        assert_eq!(
            default_length(b'C', &data[..1], 0, 0, 0),
            DefaultLength::Invalid
        );
        assert_eq!(
            default_length(b'N', &data[..3], 2, 4, 0),
            DefaultLength::Need(6)
        );
        assert_eq!(
            default_length(b'?', &data[..1], 0, 1, 0),
            DefaultLength::Format
        );
        assert_eq!(
            default_length(b'N', &data[..1], 0, 1, 0),
            DefaultLength::Invalid
        );
        let huge = i64::MAX.to_ne_bytes();
        assert_eq!(default_length(b'q', &huge, 0, 8, 1), DefaultLength::Invalid);
        assert_eq!(
            default_length(b'n', &[0, 0, 1, 2], 2, 2, 4),
            DefaultLength::Total(262)
        );
    }
}
