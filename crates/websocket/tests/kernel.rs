use std::ptr::{null, null_mut};
use swoole_rs_websocket::*;

fn parse(bytes: &[u8], header: &mut u32) -> i64 {
    unsafe { swr_v1_ws_frame_length(bytes.as_ptr(), bytes.len(), header) }
}

#[test]
fn incremental_headers_preserve_upstream_progress() {
    for (wire, progress) in [
        (vec![0x82, 0x83, 1, 2, 3, 4], vec![99, 99, 6, 6, 6, 6, 6]),
        (
            vec![0x82, 0xfe, 0, 126, 1, 2, 3, 4],
            vec![99, 99, 4, 4, 8, 8, 8, 8, 8],
        ),
        (
            vec![0x82, 0xff, 0, 0, 0, 0, 0, 1, 0, 0, 1, 2, 3, 4],
            vec![99, 99, 10, 10, 10, 10, 10, 10, 10, 10, 14, 14, 14, 14, 14],
        ),
    ] {
        for (length, expected) in progress.into_iter().enumerate() {
            let mut header = 99;
            let total = parse(&wire[..length], &mut header);
            assert_eq!(header, expected, "input length {length}");
            assert_eq!(total == 0, length < wire.len());
        }
    }
}

#[test]
fn header_boundaries_and_flags_roundtrip_without_payload() {
    // Every flag and opcode byte at each length boundary, written one byte into
    // the buffer, then every prefix of the written header (and mask key) parsed.
    for length in [0, 1, 125, 126, 127, 65535, 65536, 1 << 32] {
        for opcode in 0..=255 {
            for flags in 0..=255 {
                let mut wire = [0xa5; 16];
                let size = unsafe {
                    swr_v1_ws_write_header(length, opcode, flags, wire[1..].as_mut_ptr(), 10)
                };
                let expected_size = match length {
                    0..=125 => 2,
                    126..=65535 => 4,
                    _ => 10,
                };
                assert_eq!(size, expected_size);
                assert_eq!(wire[0], 0xa5);
                assert_eq!(wire[size + 1], 0xa5);
                assert_eq!(
                    wire[1],
                    (opcode & 15)
                        | if flags & 1 != 0 { 128 } else { 0 }
                        | if flags & 4 != 0 { 64 } else { 0 }
                );
                let code = match size {
                    2 => u8::try_from(length).unwrap(),
                    4 => 126,
                    _ => 127,
                };
                assert_eq!(wire[2], if flags & 32 != 0 { 128 } else { 0 } | code);
                if size > 2 {
                    let decoded = wire[3..=size]
                        .iter()
                        .fold(0, |acc, &b| acc << 8 | u64::from(b));
                    assert_eq!(decoded, length);
                }
                let full_header = size + if flags & 32 != 0 { 4 } else { 0 };
                for available in 0..=full_header {
                    let mut header = 99;
                    let total = parse(&wire[1..=available], &mut header);
                    let expected_total = if available < full_header {
                        0
                    } else {
                        i64::try_from(length + full_header as u64).unwrap()
                    };
                    assert_eq!(
                        total, expected_total,
                        "length {length} flags {flags} prefix {available}"
                    );
                    let expected_header = if available < 2 {
                        99
                    } else if available < size {
                        size
                    } else {
                        full_header
                    };
                    assert_eq!(header as usize, expected_header);
                }
            }
        }
    }
}

#[test]
fn signed_total_overflow_is_rejected() {
    let mut wire = [0; 14];
    for masked in [false, true] {
        let size = if masked { 14 } else { 10 };
        for (payload, expected) in [
            (isize::MAX as u64 - size as u64, isize::MAX as i64),
            (isize::MAX as u64 - size as u64 + 1, -1),
            (u64::MAX, -1),
        ] {
            unsafe {
                swr_v1_ws_write_header(
                    payload,
                    2,
                    if masked { 32 } else { 0 },
                    wire.as_mut_ptr(),
                    wire.len(),
                )
            };
            let mut header = 0;
            assert_eq!(parse(&wire, &mut header), expected);
            assert_eq!(header as usize, size);
        }
    }
}

#[test]
fn invalid_pointers_lengths_and_short_outputs_do_not_write() {
    let mut header = 99;
    let mut output = [0xa5; 10];
    unsafe {
        assert_eq!(swr_v1_ws_frame_length(null(), 0, &raw mut header), 0);
        assert_eq!(swr_v1_ws_frame_length(null(), 1, &raw mut header), -1);
        assert_eq!(
            swr_v1_ws_frame_length(output.as_ptr(), usize::MAX, &raw mut header),
            -1
        );
        assert_eq!(swr_v1_ws_frame_length(output.as_ptr(), 2, null_mut()), -1);
        assert_eq!(header, 99);
        for (length, required) in [(0, 2), (126, 4), (65536, 10), (u64::MAX, 10)] {
            for flags in [32, 255] {
                for capacity in 0..required {
                    assert_eq!(
                        swr_v1_ws_write_header(length, 2, flags, output.as_mut_ptr(), capacity),
                        0
                    );
                    assert_eq!(output, [0xa5; 10]);
                }
            }
        }
        assert_eq!(swr_v1_ws_write_header(0, 2, 0, null_mut(), 10), 0);
        assert_eq!(swr_v1_ws_write_header(1, 2, 1, null_mut(), 10), 0);
        assert_eq!(
            swr_v1_ws_write_header(1, 2, 1, output.as_mut_ptr(), usize::MAX),
            0
        );
        assert_eq!(
            swr_v1_ws_write_header(0, 2, 0, output.as_mut_ptr(), usize::MAX),
            0
        );
        assert_eq!(swr_v1_ws_mask(null_mut(), 0, null()), 0);
        assert_eq!(swr_v1_ws_mask(null_mut(), 1, output.as_ptr()), -1);
        assert_eq!(swr_v1_ws_mask(output.as_mut_ptr(), 1, null()), -1);
        assert_eq!(
            swr_v1_ws_mask(output.as_mut_ptr(), usize::MAX, output.as_ptr()),
            -1
        );
        assert_eq!(output, [0xa5; 10]);
    }
}

#[test]
fn binary_mask_matches_known_vector_and_is_an_involution() {
    // RFC 6455 section 5.7's masked Hello frame.
    let key = [0x37, 0xfa, 0x21, 0x3d];
    let mut hello = *b"Hello";
    unsafe { swr_v1_ws_mask(hello.as_mut_ptr(), hello.len(), key.as_ptr()) };
    assert_eq!(hello, [0x7f, 0x9f, 0x4d, 0x51, 0x58]);
    for length in 1..=1024 {
        let mut bytes: Vec<_> = (0..length + 2).map(|x: usize| x.to_le_bytes()[0]).collect();
        let original = bytes.clone();
        assert_eq!(
            unsafe { swr_v1_ws_mask(bytes[1..].as_mut_ptr(), length, key.as_ptr()) },
            0
        );
        for i in 0..length {
            assert_eq!(bytes[i + 1], original[i + 1] ^ key[i % 4]);
        }
        assert_eq!(bytes[0], original[0]);
        assert_eq!(bytes[length + 1], original[length + 1]);
        assert_eq!(
            unsafe { swr_v1_ws_mask(bytes[1..].as_mut_ptr(), length, key.as_ptr()) },
            0
        );
        assert_eq!(bytes, original);
    }
}

#[test]
fn masking_copies_overlapping_key_before_writing() {
    // Every byte alignment and word tail, including keys partly overlapping
    // either payload boundary: the key is read before any payload write.
    for alignment in 0..8 {
        for length in 0..=33 {
            let start = 8 + alignment;
            for key_offset in start - 4..=start + length {
                let mut bytes: Vec<u8> = (0..64u8)
                    .map(|i| i.wrapping_mul(37).wrapping_add(19))
                    .collect();
                let original = bytes.clone();
                let base = bytes.as_mut_ptr();
                // SAFETY: payload and key both lie inside `bytes`.
                let result =
                    unsafe { swr_v1_ws_mask(base.add(start), length, base.add(key_offset)) };
                assert_eq!(result, 0);
                for (i, value) in bytes.iter().enumerate() {
                    let expected = if (start..start + length).contains(&i) {
                        original[i] ^ original[key_offset + (i - start) % 4]
                    } else {
                        original[i]
                    };
                    assert_eq!(
                        *value, expected,
                        "alignment {alignment} length {length} key {key_offset}"
                    );
                }
            }
        }
    }
}
