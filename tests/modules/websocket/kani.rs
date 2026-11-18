#[kani::proof]
#[kani::unwind(17)]
fn websocket_packet() {
    let mut bytes: [u8; 16] = kani::any();
    let len: usize = kani::any();
    kani::assume(len <= bytes.len());
    let [_, second, a, b, c, d, e, f, g, h, ..] = bytes;
    let (extended, body) = match second & 127 {
        126 => (2, u64::from(u16::from_be_bytes([a, b]))),
        127 => (8, u64::from_be_bytes([a, b, c, d, e, f, g, h])),
        n => (0, u64::from(n)),
    };
    let header = 2 + extended + if second & 128 == 0 { 0 } else { 4 };
    let result = crate::decode_in_place(&mut bytes[..len]);
    if len < header || body > len.saturating_sub(header) as u64 {
        assert!(result.is_err());
    }
    if let Ok(frame) = result {
        assert_eq!(frame.header_length, header);
        assert_eq!(frame.payload_length as u64, body);
    }
}
