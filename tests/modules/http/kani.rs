#[kani::proof]
#[kani::unwind(12)]
fn url_lengths() {
    let mut bytes: [u8; 3] = kani::any();
    let len: usize = kani::any();
    let space: usize = kani::any();
    kani::assume(len <= bytes.len() && space <= 10);
    let input = &mut bytes[..len];
    let capacity = crate::url_capacity(input).unwrap();
    assert!(capacity >= len + 1 && capacity <= 3 * len + 1);
    let mut output = [std::mem::MaybeUninit::uninit(); 10];
    let encoded = crate::url_encode(input, &mut output[..space]);
    assert_eq!(encoded.is_ok(), capacity <= space);
    if let Ok(n) = encoded {
        assert_eq!(n + 1, capacity);
    }
    assert!(crate::url_decode(input) <= len);
}
