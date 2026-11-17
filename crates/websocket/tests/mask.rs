use swoole_rs_websocket::mask;

#[test]
fn binary_mask_matches_known_vector_and_is_an_involution() {
    // RFC 6455 section 5.7's masked Hello frame.
    let key = [0x37, 0xfa, 0x21, 0x3d];
    let mut hello = *b"Hello";
    mask(&mut hello, key);
    assert_eq!(hello, [0x7f, 0x9f, 0x4d, 0x51, 0x58]);
    for length in 1..=1024 {
        let mut bytes: Vec<_> = (0..length + 2).map(|x: usize| x.to_le_bytes()[0]).collect();
        let original = bytes.clone();
        mask(&mut bytes[1..=length], key);
        for i in 0..length {
            assert_eq!(bytes[i + 1], original[i + 1] ^ key[i % 4]);
        }
        assert_eq!(bytes[0], original[0]);
        assert_eq!(bytes[length + 1], original[length + 1]);
        mask(&mut bytes[1..=length], key);
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
                // The key is a value, copied out before the payload is borrowed.
                let key = *bytes[key_offset..].first_chunk().unwrap();
                mask(&mut bytes[start..start + length], key);
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

#[test]
fn a_header_decode_leaves_a_masked_payload_as_it_arrived() {
    let key = [1, 2, 3, 4];
    let mut payload = *b"hello";
    swoole_rs_websocket::mask(&mut payload, key);
    let mut frame = vec![0x81, 0x80 | 5];
    frame.extend_from_slice(&key);
    frame.extend_from_slice(&payload);
    let arrived = frame.clone();
    let header = swoole_rs_websocket::decode_header(&frame).unwrap();
    assert_eq!(frame, arrived);
    assert_eq!((header.header_length, header.payload_length), (6, 5));
    assert_eq!(header.mask_key, key);
    let decoded = swoole_rs_websocket::decode_in_place(&mut frame).unwrap();
    assert_eq!(decoded.mask_key, key);
    assert_eq!(&frame[6..], b"hello");
}
