#[kani::proof]
fn length_field() {
    let bytes: [u8; 8] = kani::any();
    let len: usize = kani::any();
    kani::assume(len <= bytes.len());
    let format = kani::any();
    let width = crate::width(format);
    let result = crate::unpack(format, &bytes[..len]);
    assert_eq!(result.is_ok(), width != 0 && width <= len);
}
