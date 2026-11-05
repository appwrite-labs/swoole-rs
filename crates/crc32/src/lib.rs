//! IEEE CRC32, ported from Swoole v6.2.2 src/core/crc32.cc (Apache-2.0).
//! Original author: Tianfeng Han <rango@swoole.com>.
//! The lookup table is immutable and generated during compilation.
#![cfg_attr(
    not(test),
    deny(
        clippy::undocumented_unsafe_blocks,
        clippy::multiple_unsafe_ops_per_block
    )
)]

// Use instructions only when the compiler's target contract already guarantees
// IEEE CRC support. No runtime CPU query, lazy state or OS effect is introduced.
// Other targets retain the exact slicing-by-eight implementation below.
#[cfg(any(test, not(all(target_arch = "aarch64", target_feature = "crc"))))]
mod portable {
    // Row n is the byte contribution followed by n zero bytes. Divide the base
    // row once, then append each zero byte using that same immutable recurrence.
    #[expect(
        clippy::indexing_slicing,
        reason = "const-evaluated into TABLES: an out-of-range index fails the build, \
                  and const fn has no get()"
    )]
    const fn tables() -> [[u32; 256]; 8] {
        let mut entries = [[0; 256]; 8];
        let mut index = 0;
        while index < 256 {
            #[expect(clippy::cast_possible_truncation, reason = "index < 256")]
            let mut value = index as u32;
            let mut bit = 0;
            while bit < 8 {
                value = (value >> 1) ^ (0xedb8_8320 * (value & 1));
                bit += 1;
            }
            entries[0][index] = value;
            index += 1;
        }
        let mut row = 1;
        while row < entries.len() {
            index = 0;
            while index < 256 {
                let value = entries[row - 1][index];
                entries[row][index] = (value >> 8) ^ entries[0][(value & 0xff) as usize];
                index += 1;
            }
            row += 1;
        }
        entries
    }

    pub(super) static TABLES: [[u32; 256]; 8] = tables();

    /// Return IEEE CRC32 with the upstream initial and final XOR.
    pub(super) fn crc32(input: &[u8]) -> u32 {
        let mut crc = u32::MAX;
        let (blocks, tail) = input.as_chunks::<8>();
        for block in blocks {
            // Exact chunks establish the conversion length. Explicit little-endian
            // decoding retains the same byte recurrence on either host endianness.
            let bytes = (u64::from_le_bytes(*block) ^ u64::from(crc)).to_le_bytes();
            crc = TABLES
                .iter()
                .rev()
                .zip(bytes)
                .fold(0, |crc, (row, byte)| crc ^ entry(row, byte));
        }
        !tail.iter().fold(crc, |crc, &byte| {
            (crc >> 8) ^ entry(&TABLES[0], crc.to_le_bytes()[0] ^ byte)
        })
    }

    // A byte always indexes a 256-entry row, so the fallback is never taken.
    fn entry(row: &[u32; 256], byte: u8) -> u32 {
        row.get(usize::from(byte)).copied().unwrap_or_default()
    }
}

/// Return IEEE CRC32 with the upstream initial and final XOR.
#[must_use]
pub fn crc32(input: &[u8]) -> u32 {
    #[cfg(all(target_arch = "aarch64", target_feature = "crc"))]
    // SAFETY: the selected compile target already guarantees the crc feature.
    unsafe {
        crc32_instructions(input)
    }
    #[cfg(not(all(target_arch = "aarch64", target_feature = "crc")))]
    portable::crc32(input)
}

#[cfg(all(target_arch = "aarch64", target_feature = "crc"))]
#[target_feature(enable = "crc")]
unsafe fn crc32_instructions(input: &[u8]) -> u32 {
    use core::arch::aarch64::{__crc32b, __crc32d, __crc32h, __crc32w};
    let mut crc = u32::MAX;
    let (blocks, mut tail) = input.as_chunks::<8>();
    for block in blocks {
        crc = __crc32d(crc, u64::from_le_bytes(*block));
    }
    if let Some((word, remaining)) = tail.split_first_chunk::<4>() {
        crc = __crc32w(crc, u32::from_le_bytes(*word));
        tail = remaining;
    }
    if let Some((half, remaining)) = tail.split_first_chunk::<2>() {
        crc = __crc32h(crc, u16::from_le_bytes(*half));
        tail = remaining;
    }
    if let Some(&byte) = tail.first() {
        crc = __crc32b(crc, byte);
    }
    !crc
}

/// Borrow input for one call; no allocations, callbacks, or retained pointers.
///
/// # Safety
/// Nonempty input must point to `length` readable bytes in one allocation.
/// Empty input may be null. Invalid null/oversized input returns zero.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn swr_v1_crc32(input: *const u8, length: usize) -> u32 {
    if length == 0
        || input.is_null()
        || length > isize::MAX as usize
        || (input as usize).checked_add(length).is_none()
    {
        return 0;
    }
    // SAFETY: null, representability and address wrap checked; caller provides extent.
    crc32(unsafe { core::slice::from_raw_parts(input, length) })
}

#[cfg(test)]
mod tests {
    use super::portable::TABLES;
    use super::*;

    #[test]
    fn every_table_row_matches_independent_polynomial_division() {
        for (row, entries) in TABLES.iter().enumerate() {
            for (byte, &actual) in entries.iter().enumerate() {
                let mut expected = u32::try_from(byte).expect("a table index is a byte");
                for _ in 0..8 * (row + 1) {
                    expected = if expected & 1 == 0 {
                        expected >> 1
                    } else {
                        (expected >> 1) ^ 0xedb8_8320
                    };
                }
                assert_eq!(actual, expected, "row={row}, byte={byte}");
            }
        }
    }

    #[test]
    fn standard_vectors() {
        assert_eq!(crc32(b""), 0);
        assert_eq!(crc32(b"123456789"), 0xcbf4_3926);
        assert_eq!(
            crc32(b"The quick brown fox jumps over the lazy dog"),
            0x414f_a339
        );
        assert_eq!(crc32(&[0; 32]), 0x190a_55ad);
        assert_eq!(crc32(&[255; 32]), 0xff6c_ab0b);
    }

    #[test]
    fn exhaustive_two_byte_bitwise_oracle() {
        // Independent polynomial division, without the implementation's table.
        for value in 0..=u16::MAX {
            let bytes = value.to_le_bytes();
            let mut expected = u32::MAX;
            for byte in bytes {
                expected ^= u32::from(byte);
                for _ in 0..8 {
                    expected = if expected & 1 == 0 {
                        expected >> 1
                    } else {
                        (expected >> 1) ^ 0xedb8_8320
                    };
                }
            }
            assert_eq!(crc32(&bytes), !expected);
            assert_eq!(portable::crc32(&bytes), !expected);
        }
    }

    #[test]
    fn sliced_blocks_and_tails_match_bitwise_oracle() {
        fn bitwise(input: &[u8]) -> u32 {
            let mut crc = u32::MAX;
            for &byte in input {
                crc ^= u32::from(byte);
                for _ in 0..8 {
                    crc = (crc >> 1) ^ (0xedb8_8320u32.wrapping_mul(crc & 1));
                }
            }
            !crc
        }
        let mut data = vec![0u8; 65536 + 16];
        let mut seed = 0x1234_abcdu32;
        for byte in &mut data {
            seed ^= seed << 13;
            seed ^= seed >> 17;
            seed ^= seed << 5;
            *byte = seed.to_le_bytes()[0];
        }
        for offset in 0..16 {
            for length in
                (0..=129).chain([255, 256, 257, 1023, 1024, 1025, 4095, 4096, 4097, 65536])
            {
                let bytes = &data[offset..offset + length];
                let expected = bitwise(bytes);
                assert_eq!(crc32(bytes), expected, "offset={offset}, length={length}");
                assert_eq!(
                    portable::crc32(bytes),
                    expected,
                    "portable offset={offset}, length={length}"
                );
                // The same actual C entrypoint is linked by production callers.
                // SAFETY: `bytes` is a live slice of exactly `bytes.len()` readable bytes.
                let actual = unsafe { swr_v1_crc32(bytes.as_ptr(), bytes.len()) };
                assert_eq!(actual, expected);
            }
        }
    }

    #[test]
    fn defensive_bounds() {
        // The export rejects null, overlong and address-wrapping
        // ranges before reading, so none of these pointers is dereferenced.
        let cases: [(*const u8, usize); 5] = [
            (core::ptr::null(), 0),
            (core::ptr::null(), 1),
            (core::ptr::dangling(), usize::MAX),
            (usize::MAX as *const u8, 1),
            ((usize::MAX - 7) as *const u8, 8),
        ];
        for (data, length) in cases {
            // SAFETY: see above; the export returns 0 without reading.
            assert_eq!(
                unsafe { swr_v1_crc32(data, length) },
                0,
                "{data:?} {length}"
            );
        }
    }
}
