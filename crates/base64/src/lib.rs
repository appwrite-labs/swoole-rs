//! Allocation-free Swoole base64, including its permissive decoder write semantics.
// Ported from Swoole src/core/base64.cc, Copyright (c) 2012-2017 The Swoole Group.
// Original author: Tianfeng Han <rango@swoole.com>. Apache License, Version 2.0.
#![forbid(unsafe_code)]

const ALPHABET: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";

/// The character for a sextet; only its low six bits are used.
fn symbol(sextet: u8) -> u8 {
    #[expect(
        clippy::indexing_slicing,
        reason = "masked to 0..64, the length of ALPHABET"
    )]
    ALPHABET[usize::from(sextet & 63)]
}

fn encoded_triplet(a: u8, b: u8, c: u8) -> [u8; 4] {
    [
        symbol(a >> 2),
        symbol(((a & 3) << 4) | (b >> 4)),
        symbol(((b & 15) << 2) | (c >> 6)),
        symbol(c),
    ]
}

/// Encode `input` with the upstream alphabet and padding, then a NUL, and return
/// the encoded length excluding the NUL. A full encoding needs
/// `4 * ceil(input.len() / 3) + 1` output bytes; groups that don't fit are skipped.
pub fn encode(input: &[u8], output: &mut [u8]) -> usize {
    let mut written = 0;
    let (chunks, tail) = input.as_chunks::<3>();
    for (chunk, slots) in chunks.iter().zip(output.as_chunks_mut::<4>().0) {
        *slots = encoded_triplet(chunk[0], chunk[1], chunk[2]);
        written += 4;
    }
    if let Some(&a) = tail.first() {
        let mut group = encoded_triplet(a, tail.get(1).copied().unwrap_or(0), 0);
        group[3] = b'=';
        if tail.len() == 1 {
            group[2] = b'=';
        }
        if let Some(slots) = output.get_mut(written..written + 4) {
            slots.copy_from_slice(&group);
        }
        written += 4;
    }
    if let Some(nul) = output.get_mut(written) {
        *nul = 0;
    }
    written
}

const fn decoding_table() -> [u8; 256] {
    let mut table = [u8::MAX; 256];
    let mut index = 0;
    while index < ALPHABET.len() {
        #[expect(clippy::cast_possible_truncation, reason = "index < 64")]
        let value = index as u8;
        #[expect(
            clippy::indexing_slicing,
            reason = "index < 64 and a u8 indexes 256 entries; get() isn't const"
        )]
        {
            table[ALPHABET[index] as usize] = value;
        }
        index += 1;
    }
    table
}
static DECODING: [u8; 256] = decoding_table();

fn sextet(byte: u8) -> Option<u8> {
    #[expect(clippy::indexing_slicing, reason = "a u8 indexes 256 entries")]
    let value = DECODING[usize::from(byte)];
    (value != u8::MAX).then_some(value)
}

/// Decode with pinned permissive/partial-write semantics, then a NUL; invalid data
/// returns 0 and leaves the partial writes upstream makes. A full decoding needs
/// `input.len() + 1` output bytes; writes past `output` are skipped.
pub fn decode(input: &[u8], output: &mut [u8]) -> usize {
    let mut put = |at: usize, byte: u8| {
        if let Some(slot) = output.get_mut(at) {
            *slot = byte;
        }
    };
    let mut written = 0;
    let mut partial = 0u8;
    for (i, &byte) in input.iter().enumerate() {
        if byte == b'=' {
            break;
        }
        let Some(c) = sextet(byte) else {
            return 0;
        };
        match i % 4 {
            0 => {
                partial = c << 2;
                put(written, partial);
            }
            1 => {
                partial = partial.wrapping_add((c >> 4) & 3);
                put(written, partial);
                written += 1;
                // Keep size_t subtraction and short-circuiting: short incomplete
                // groups do write a provisional next byte in the pinned decoder.
                if i < input.len().wrapping_sub(3)
                    || input.get(input.len().wrapping_sub(2)) != Some(&b'=')
                {
                    partial = (c & 15) << 4;
                    put(written, partial);
                }
            }
            2 => {
                partial = partial.wrapping_add((c >> 2) & 15);
                put(written, partial);
                written += 1;
                if i < input.len().wrapping_sub(2) || input.last() != Some(&b'=') {
                    partial = (c & 3) << 6;
                    put(written, partial);
                }
            }
            _ => {
                partial = partial.wrapping_add(c);
                put(written, partial);
                written += 1;
            }
        }
    }
    put(written, 0);
    written
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rfc_vectors() {
        for (plain, coded) in [
            ("", ""),
            ("f", "Zg=="),
            ("fo", "Zm8="),
            ("foo", "Zm9v"),
            ("foob", "Zm9vYg=="),
            ("fooba", "Zm9vYmE="),
            ("foobar", "Zm9vYmFy"),
        ] {
            let mut output = [0xa5; 32];
            assert_eq!(encode(plain.as_bytes(), &mut output), coded.len());
            assert_eq!(&output[..=coded.len()], [coded.as_bytes(), &[0]].concat());
            assert_eq!(decode(coded.as_bytes(), &mut output), plain.len());
            assert_eq!(&output[..=plain.len()], [plain.as_bytes(), &[0]].concat());
        }
    }

    #[test]
    #[cfg_attr(miri, ignore = "megabytes of encoding are too slow to interpret")]
    fn encode_blocks_tails_and_canaries_match_bitstream_oracle() {
        // Independent bitstream extraction, without the triplet implementation.
        for length in (0..130).chain([255, 256, 257, 1023, 1024, 1025, 65536]) {
            let input: Vec<_> = (0..length)
                .map(|i: usize| (i * 37 + 19).to_le_bytes()[0])
                .collect();
            let mut expected = Vec::new();
            let (mut accumulator, mut bits) = (0u32, 0u32);
            for &byte in &input {
                accumulator = (accumulator << 8) | u32::from(byte);
                bits += 8;
                while bits >= 6 {
                    bits -= 6;
                    expected.push(ALPHABET[((accumulator >> bits) & 63) as usize]);
                }
            }
            if bits != 0 {
                expected.push(ALPHABET[((accumulator << (6 - bits)) & 63) as usize]);
            }
            while expected.len() % 4 != 0 {
                expected.push(b'=');
            }
            let count = expected.len();
            expected.push(0);
            for offset in 0..16 {
                let mut output = vec![0xa5; offset + expected.len() + 17];
                let actual = encode(&input, &mut output[offset..offset + expected.len()]);
                assert_eq!(actual, count);
                assert_eq!(&output[offset..offset + expected.len()], expected);
                assert!(output[..offset].iter().all(|&byte| byte == 0xa5));
                assert!(
                    output[offset + expected.len()..]
                        .iter()
                        .all(|&byte| byte == 0xa5)
                );
            }
        }
    }

    #[test]
    fn classification_matches_alphabet_for_every_byte() {
        for byte in 0..=u8::MAX {
            assert_eq!(
                sextet(byte),
                ALPHABET
                    .iter()
                    .position(|&b| b == byte)
                    .map(|i| u8::try_from(i).unwrap())
            );
        }
    }

    #[test]
    #[cfg_attr(miri, ignore = "megabytes of decoding are too slow to interpret")]
    fn canonical_decode_preserves_full_output_and_canaries() {
        for length in (1..130).chain([255, 256, 257, 1023, 1024, 1025, 65536]) {
            let input: Vec<_> = (0..length)
                .map(|i: usize| (i * 37 + 19).to_le_bytes()[0])
                .collect();
            let mut coded = vec![0; length.div_ceil(3) * 4 + 1];
            let size = encode(&input, &mut coded);
            for offset in 0..16 {
                let mut output = vec![0xa5; offset + size + 1 + 17];
                let count = decode(&coded[..size], &mut output[offset..=offset + size]);
                assert_eq!(count, length);
                assert_eq!(&output[offset..offset + length], input);
                assert_eq!(output[offset + length], 0);
                assert!(output[..offset].iter().all(|&byte| byte == 0xa5));
                assert!(
                    output[offset + length + 1..]
                        .iter()
                        .all(|&byte| byte == 0xa5)
                );
            }
        }
    }

    #[test]
    fn partial_writes_are_observable() {
        let mut output = [0xa5; 8];
        assert_eq!(decode(b"Zh!", &mut output), 0);
        assert_eq!(output, [b'f', 16, 0xa5, 0xa5, 0xa5, 0xa5, 0xa5, 0xa5]);
        assert_eq!(decode(b"Zh=anything", &mut output), 1);
        assert_eq!(&output[..3], &[b'f', 0, 0xa5]);
        assert_eq!(decode(b"Z", &mut output), 0);
        assert_eq!(output[0], 0);
    }
}
