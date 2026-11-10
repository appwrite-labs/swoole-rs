use swoole_rs_websocket::{frame_length, header};

/// The deleted `swr_v1_ws_frame_length`'s view: the header length lands in
/// `header` once known.
fn parse(bytes: &[u8], header: &mut u32) -> i64 {
    let (total, known) = frame_length(bytes);
    if let Some(known) = known {
        *header = known;
    }
    total
}

/// `header` written to the start of `out`; its size.
fn write_header(length: u64, opcode: u8, flags: u8, out: &mut [u8]) -> usize {
    let (bytes, size) = header(length, opcode, flags);
    for (out, byte) in out.iter_mut().zip(bytes).take(size) {
        *out = byte;
    }
    size
}

#[test]
fn short_input_reports_nothing() {
    let mut header = 99;
    assert_eq!(parse(&[], &mut header), 0);
    assert_eq!(parse(&[0x82], &mut header), 0);
    assert_eq!(header, 99);
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
                let size = write_header(length, opcode, flags, &mut wire[1..]);
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
            write_header(payload, 2, if masked { 32 } else { 0 }, &mut wire);
            let mut header = 0;
            assert_eq!(parse(&wire, &mut header), expected);
            assert_eq!(header as usize, size);
        }
    }
}
