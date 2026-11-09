//! IEEE CRC32, ported from Swoole v6.2.2 src/core/crc32.cc (Apache-2.0).
//! Original author: Tianfeng Han <rango@swoole.com>.
//! The lookup table is immutable and generated during compilation.
#![forbid(unsafe_code)]

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

static TABLES: [[u32; 256]; 8] = tables();

/// Return IEEE CRC32 with the upstream initial and final XOR.
#[must_use]
pub fn crc32(input: &[u8]) -> u32 {
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

#[cfg(test)]
mod tests {
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
    #[cfg_attr(miri, ignore = "65,536 inputs are too slow to interpret")]
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
        }
    }

    #[test]
    #[cfg_attr(miri, ignore = "megabytes of bitwise CRC are too slow to interpret")]
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
            }
        }
    }
}
