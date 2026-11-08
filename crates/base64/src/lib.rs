//! Allocation-free Swoole base64, including its permissive decoder write semantics.
// Ported from Swoole src/core/base64.cc, Copyright (c) 2012-2017 The Swoole Group.
// Original author: Tianfeng Han <rango@swoole.com>. Apache License, Version 2.0.
#![cfg_attr(
    not(test),
    deny(
        clippy::undocumented_unsafe_blocks,
        clippy::multiple_unsafe_ops_per_block
    )
)]

use std::mem::MaybeUninit;

const ALPHABET: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";

fn encoded_size(length: usize) -> Option<usize> {
    length
        .checked_add(2)?
        .checked_div(3)?
        .checked_mul(4)?
        .checked_add(1)
}

/// The character for a sextet; only its low six bits are used.
fn symbol(sextet: u8) -> u8 {
    #[expect(
        clippy::indexing_slicing,
        reason = "masked to 0..64, the length of ALPHABET"
    )]
    ALPHABET[usize::from(sextet & 63)]
}

fn encoded_triplet(a: u8, b: u8, c: u8) -> [MaybeUninit<u8>; 4] {
    [
        symbol(a >> 2),
        symbol(((a & 3) << 4) | (b >> 4)),
        symbol(((b & 15) << 2) | (c >> 6)),
        symbol(c),
    ]
    .map(MaybeUninit::new)
}

fn encode(input: &[u8], output: &mut [MaybeUninit<u8>]) -> usize {
    #[cfg(all(
        target_arch = "aarch64",
        target_endian = "little",
        target_feature = "neon"
    ))]
    let (consumed, written) = {
        let consumed = input.len() / 48 * 48;
        let written = consumed / 3 * 4;
        if consumed != 0 {
            // SAFETY: compile target guarantees NEON; it encodes the complete
            // 48-byte input blocks into disjoint, writable 64-slot output blocks.
            unsafe { encode_neon(input, output) };
        }
        (consumed, written)
    };
    #[cfg(not(all(
        target_arch = "aarch64",
        target_endian = "little",
        target_feature = "neon"
    )))]
    let (consumed, written) = (0, 0);
    let rest = output.get_mut(written..).unwrap_or_default();
    written + encode_scalar(input.get(consumed..).unwrap_or_default(), rest)
}

#[cfg(all(
    target_arch = "aarch64",
    target_endian = "little",
    target_feature = "neon"
))]
#[target_feature(enable = "neon")]
unsafe fn encode_neon(input: &[u8], output: &mut [MaybeUninit<u8>]) {
    use core::arch::aarch64::{
        uint8x16x3_t, uint8x16x4_t, vandq_u8, vdupq_n_u8, vld1q_u8, vld3q_u8, vorrq_u8, vqtbl4q_u8,
        vshlq_n_u8, vshrq_n_u8, vst4q_u8,
    };
    // One immutable alphabet, not four persistent copies.
    let quarter = |offset: usize| {
        // SAFETY: `offset` is 0, 16, 32 or 48, so these 16 bytes lie inside the
        // initialized 64-byte `ALPHABET`.
        unsafe { vld1q_u8(ALPHABET.as_ptr().wrapping_add(offset)) }
    };
    let table = uint8x16x4_t(quarter(0), quarter(16), quarter(32), quarter(48));
    for (bytes, slots) in input
        .as_chunks::<48>()
        .0
        .iter()
        .zip(output.as_chunks_mut::<64>().0)
    {
        // Deinterleave sixteen complete triples, calculate their sextets, and
        // interleave four encoded characters per triple into exactly 64 slots.
        // SAFETY: `bytes` is a complete 48-byte chunk.
        let uint8x16x3_t(a, b, c) = unsafe { vld3q_u8(bytes.as_ptr()) };
        let encoded = uint8x16x4_t(
            vqtbl4q_u8(table, vshrq_n_u8::<2>(a)),
            vqtbl4q_u8(
                table,
                vorrq_u8(
                    vshlq_n_u8::<4>(vandq_u8(a, vdupq_n_u8(3))),
                    vshrq_n_u8::<4>(b),
                ),
            ),
            vqtbl4q_u8(
                table,
                vorrq_u8(
                    vshlq_n_u8::<2>(vandq_u8(b, vdupq_n_u8(15))),
                    vshrq_n_u8::<6>(c),
                ),
            ),
            vqtbl4q_u8(table, vandq_u8(c, vdupq_n_u8(63))),
        );
        // SAFETY: `slots` is a complete, exclusive 64-slot chunk; MaybeUninit permits
        // this full write without reading old output.
        unsafe { vst4q_u8(slots.as_mut_ptr().cast(), encoded) };
    }
}

fn encode_scalar(input: &[u8], output: &mut [MaybeUninit<u8>]) -> usize {
    let mut written = 0;
    let (chunks, tail) = input.as_chunks::<3>();
    for (chunk, slots) in chunks.iter().zip(output.as_chunks_mut::<4>().0) {
        slots.copy_from_slice(&encoded_triplet(chunk[0], chunk[1], chunk[2]));
        written += 4;
    }
    if let Some(&a) = tail.first() {
        let mut group = encoded_triplet(a, tail.get(1).copied().unwrap_or(0), 0);
        group[3].write(b'=');
        if tail.len() == 1 {
            group[2].write(b'=');
        }
        if let Some(slots) = output.get_mut(written..written + 4) {
            slots.copy_from_slice(&group);
        }
        written += 4;
    }
    if let Some(nul) = output.get_mut(written) {
        nul.write(0);
    }
    written
}

// Preserve the pinned encoder's individual read/store sequence for positive
// overlapping buffers. Loading a whole triplet (or snapshotting input) changes
// later reads when an earlier output store aliases current/preceding input.
// No references are formed from either raw pointer in this branch.
//
// SAFETY: positive length, initialized readable input, writable full encoded
// output extent, valid pointer arithmetic; the extents may overlap.
unsafe fn encode_overlapping(input: *const u8, length: usize, output: *mut u8) -> usize {
    // One raw read or store per call, in the C++ order; `wrapping_add` keeps the
    // original provenance without forming references.
    let get = |i: usize| {
        // SAFETY: every caller passes `i < length`, inside the readable input.
        unsafe { input.wrapping_add(i).read() }
    };
    let put = |at: usize, byte: u8| {
        // SAFETY: `at` stays below the encoded size, inside the writable output.
        unsafe { output.wrapping_add(at).write(byte) }
    };
    let mut written = 0;
    for i in 0..length {
        let sextet = match i % 3 {
            0 => get(i) >> 2,
            1 => ((get(i - 1) & 3) << 4) + (get(i) >> 4),
            _ => ((get(i - 1) & 15) << 2) + (get(i) >> 6),
        };
        put(written, symbol(sextet));
        written += 1;
        if i % 3 == 2 {
            // This reread must occur after the preceding store, just as in C++.
            put(written, symbol(get(i)));
            written += 1;
        }
    }
    let last = length - 1;
    match last % 3 {
        0 => {
            put(written, symbol((get(last) & 3) << 4));
            put(written + 1, b'=');
            put(written + 2, b'=');
            written += 3;
        }
        1 => {
            put(written, symbol((get(last) & 15) << 2));
            put(written + 1, b'=');
            written += 2;
        }
        _ => {}
    }
    put(written, 0);
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

fn decode(input: &[u8], output: &mut [MaybeUninit<u8>]) -> usize {
    let mut put = |at: usize, byte: u8| {
        if let Some(slot) = output.get_mut(at) {
            slot.write(byte);
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

fn valid_range(pointer: *const u8, length: usize) -> bool {
    isize::try_from(length).is_ok()
        && (!pointer.is_null() || length == 0)
        && (pointer as usize).checked_add(length).is_some()
}

/// Encode into caller-owned storage and return the count excluding the NUL.
///
/// # Safety
/// Non-null input must initially be readable and initialized for `length` bytes.
/// Output must be writable for `4 * ceil(length / 3) + 1` bytes. Valid positive
/// extents may overlap: reads and writes follow the pinned per-statement order,
/// including observing earlier output stores through the input pointer. Caller
/// storage permits these writes and excludes concurrent access during the call.
/// Neither pointer is retained. Null input is permitted for zero length; null
/// output is rejected.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn swr_v1_base64_encode(
    input: *const u8,
    length: usize,
    output: *mut u8,
) -> usize {
    let Some(size) = encoded_size(length) else {
        return 0;
    };
    if output.is_null() || !valid_range(input, length) || !valid_range(output, size) {
        return 0;
    }
    // Checked integer addresses classify extents only. Dereferences retain
    // original pointer provenance; no shared/mutable slices exist yet.
    if length != 0
        && (input as usize) < output as usize + size
        && (output as usize) < input as usize + length
    {
        // SAFETY: caller supplies readable/writable extents; checked sizes and
        // positive length satisfy the raw source-ordered encoder's contract.
        return unsafe { encode_overlapping(input, length, output) };
    }
    // Validated extents do not overlap (or input is empty).
    let input = if length == 0 {
        &[]
    } else {
        // SAFETY: the caller lends `length` readable bytes, disjoint from the output.
        unsafe { std::slice::from_raw_parts(input, length) }
    };
    // SAFETY: the caller lends `size` writable bytes, disjoint from the input.
    let output = unsafe { std::slice::from_raw_parts_mut(output.cast::<MaybeUninit<u8>>(), size) };
    encode(input, output)
}

/// Decode with pinned permissive/partial-write semantics; invalid data returns 0.
///
/// # Safety
/// Non-null input must be readable for `length` bytes. Output must be writable
/// for `length + 1` bytes and disjoint from input. Neither pointer is retained.
/// Null input is permitted for zero length; null output is rejected.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn swr_v1_base64_decode(
    input: *const u8,
    length: usize,
    output: *mut u8,
) -> usize {
    let Some(size) = length.checked_add(1) else {
        return 0;
    };
    if output.is_null() || !valid_range(input, length) || !valid_range(output, size) {
        return 0;
    }
    let input = if length == 0 {
        &[]
    } else {
        // SAFETY: the caller lends `length` readable bytes, disjoint from the output.
        unsafe { std::slice::from_raw_parts(input, length) }
    };
    // SAFETY: the caller lends `length + 1` writable bytes, disjoint from the input.
    let output = unsafe { std::slice::from_raw_parts_mut(output.cast::<MaybeUninit<u8>>(), size) };
    decode(input, output)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn slots(output: &mut [u8]) -> &mut [MaybeUninit<u8>] {
        // SAFETY: MaybeUninit has the same layout and weaker validity requirements.
        unsafe { std::slice::from_raw_parts_mut(output.as_mut_ptr().cast(), output.len()) }
    }

    // Safe indexing model of pinned base64.cc statements on copied initialized
    // storage. It deliberately rereads backing after each preceding write;
    // snapshot-input/reference-encoder comparisons cannot test this domain.
    fn source_order_model(storage: &mut [u8], input: usize, length: usize, output: usize) -> usize {
        let mut j = output;
        for i in 0..length {
            match i % 3 {
                0 => storage[j] = ALPHABET[(storage[input + i] >> 2) as usize],
                1 => {
                    storage[j] = ALPHABET[(((storage[input + i - 1] & 3) << 4)
                        + (storage[input + i] >> 4))
                        as usize];
                }
                _ => {
                    storage[j] = ALPHABET[(((storage[input + i - 1] & 15) << 2)
                        + (storage[input + i] >> 6))
                        as usize];
                    j += 1;
                    storage[j] = ALPHABET[(storage[input + i] & 63) as usize];
                }
            }
            j += 1;
        }
        let last = length - 1;
        if last.is_multiple_of(3) {
            storage[j] = ALPHABET[((storage[input + last] & 3) << 4) as usize];
            storage[j + 1] = b'=';
            storage[j + 2] = b'=';
            j += 3;
        } else if last % 3 == 1 {
            storage[j] = ALPHABET[((storage[input + last] & 15) << 2) as usize];
            storage[j + 1] = b'=';
            j += 2;
        }
        storage[j] = 0;
        j - output
    }

    #[test]
    fn encoder_overlaps_preserve_source_order_in_initialized_storage() {
        let offsets = [0, 1, 2, 7, 15, 16, 31, 47, 48, 63, 96, 223];
        let mut overlaps = 0;
        let mut disjoint = 0;
        // Miri checks the aliasing at the block and tail edges with one seed.
        let lengths =
            (1..=160).filter(|length| !cfg!(miri) || [1, 2, 3, 48, 49, 160].contains(length));
        let seeds: &[u8] = if cfg!(miri) { &[0] } else { &[0, 1, 127, 255] };
        for length in lengths {
            for input in offsets {
                for output in offsets {
                    for &seed in seeds {
                        let mut actual = [0u8; 512];
                        for (i, byte) in actual.iter_mut().enumerate() {
                            *byte = i.to_le_bytes()[0].wrapping_mul(37).wrapping_add(seed);
                        }
                        let mut expected = actual;
                        let n = source_order_model(&mut expected, input, length, output);
                        // Single original backing pointer, no live input/output
                        // references across this call. Every extent is in-bounds.
                        let base = actual.as_mut_ptr();
                        let result = unsafe {
                            swr_v1_base64_encode(base.add(input), length, base.add(output))
                        };
                        assert_eq!(result, n, "length={length} in={input} out={output}");
                        assert_eq!(actual, expected, "length={length} in={input} out={output}");
                        if input < output + encoded_size(length).unwrap() && output < input + length
                        {
                            overlaps += 1;
                        } else {
                            disjoint += 1;
                        }
                    }
                }
            }
        }
        assert!(overlaps > 0 && disjoint > 0);
        // These fixed statement-order results differ from snapshot encoding:
        // they require the second case-2 read and final padding read after write.
        for (length, result) in [(1, &b"AQ==\0"[..]), (3, &b"AQEF\0"[..])] {
            let mut storage = [0u8; 8];
            let base = storage.as_mut_ptr();
            assert_eq!(unsafe { swr_v1_base64_encode(base, length, base) }, 4);
            assert_eq!(&storage[..5], result);
        }
    }

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
            assert_eq!(encode(plain.as_bytes(), slots(&mut output)), coded.len());
            assert_eq!(&output[..=coded.len()], [coded.as_bytes(), &[0]].concat());
            assert_eq!(decode(coded.as_bytes(), slots(&mut output)), plain.len());
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
                // SAFETY: input/output live, disjoint and have the exact ABI extent.
                let actual = unsafe {
                    swr_v1_base64_encode(
                        input.as_ptr(),
                        input.len(),
                        output.as_mut_ptr().add(offset),
                    )
                };
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

    #[cfg(all(
        target_arch = "aarch64",
        target_endian = "little",
        target_feature = "neon"
    ))]
    #[test]
    #[cfg_attr(miri, ignore = "196,608 encodes are too slow to interpret")]
    fn encode_lane_order_and_each_input_byte_match_portable() {
        for offset in 0..16 {
            for position in 0..48 {
                for byte in 0..=u8::MAX {
                    let mut backing = [0xa5; 63];
                    let input = &mut backing[offset..offset + 48];
                    input[position] = byte;
                    let mut selected = [0xcc; 81];
                    let mut portable = [0xcc; 81];
                    let count = encode(input, slots(&mut selected[7..]));
                    assert_eq!(count, 64);
                    assert_eq!(encode_scalar(input, slots(&mut portable[7..])), count);
                    assert_eq!(
                        selected, portable,
                        "offset={offset}, position={position}, byte={byte}"
                    );
                    assert!(selected[..7].iter().all(|&value| value == 0xcc));
                    assert_eq!(selected[7 + count], 0);
                    assert!(selected[8 + count..].iter().all(|&value| value == 0xcc));
                }
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
            let mut coded = vec![0; encoded_size(length).unwrap()];
            let size = encode(&input, slots(&mut coded));
            for offset in 0..16 {
                let mut output = vec![0xa5; offset + size + 1 + 17];
                // SAFETY: separate live allocations satisfy the frozen decoder extents.
                let count = unsafe {
                    swr_v1_base64_decode(coded.as_ptr(), size, output.as_mut_ptr().add(offset))
                };
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
        assert_eq!(decode(b"Zh!", slots(&mut output)), 0);
        assert_eq!(output, [b'f', 16, 0xa5, 0xa5, 0xa5, 0xa5, 0xa5, 0xa5]);
        assert_eq!(decode(b"Zh=anything", slots(&mut output)), 1);
        assert_eq!(&output[..3], &[b'f', 0, 0xa5]);
        assert_eq!(decode(b"Z", slots(&mut output)), 0);
        assert_eq!(output[0], 0);
    }

    #[test]
    fn ffi_rejects_invalid_ranges_without_writes() {
        let input = *b"f";
        let mut output = [0xa5; 8];
        for function in [swr_v1_base64_encode, swr_v1_base64_decode] {
            // SAFETY: invalid inputs here must be rejected before dereferencing.
            unsafe {
                assert_eq!(function(std::ptr::null(), 1, output.as_mut_ptr()), 0);
                assert_eq!(function(input.as_ptr(), 1, std::ptr::null_mut()), 0);
                assert_eq!(function(input.as_ptr(), usize::MAX, output.as_mut_ptr()), 0);
                assert_eq!(
                    function(input.as_ptr(), isize::MAX as usize, output.as_mut_ptr()),
                    0
                );
                assert_eq!(
                    function(
                        std::ptr::without_provenance(usize::MAX),
                        1,
                        output.as_mut_ptr()
                    ),
                    0
                );
                assert_eq!(
                    function(
                        input.as_ptr(),
                        1,
                        std::ptr::without_provenance_mut(usize::MAX)
                    ),
                    0
                );
            }
            assert_eq!(output, [0xa5; 8]);
            unsafe {
                assert_eq!(function(std::ptr::null(), 0, output.as_mut_ptr()), 0);
            }
            assert_eq!(output[0], 0);
            output[0] = 0xa5;
        }
    }
}
