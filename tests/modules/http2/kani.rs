#[kani::proof]
fn http2_packet() {
    let bytes: [u8; 17] = kani::any();
    let len: usize = kani::any();
    kani::assume(len <= bytes.len());
    let payload_len = u32::from_be_bytes([0, bytes[0], bytes[1], bytes[2]]) as usize;
    let packet = crate::decode_packet(&bytes[..len], kani::any(), kani::any());
    assert_eq!(packet.is_ok(), len >= 9 + payload_len);
    if let Ok((facts, payload)) = packet {
        assert_eq!(facts.length as usize, payload_len);
        assert_eq!(payload.len(), payload_len);
    }
}

#[kani::proof]
fn http2_fixed_payloads() {
    let bytes: [u8; 9] = kani::any();
    let len: usize = kani::any();
    kani::assume(len <= bytes.len());
    let payload = &bytes[..len];
    assert_eq!(crate::window_increment(payload).is_ok(), len == 4);
    assert_eq!(crate::ping_payload(payload).is_ok(), len == 8);
}
