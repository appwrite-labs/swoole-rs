#[kani::proof]
fn task_packet() {
    use crate::{DataHead, process::task_file::Packed};
    const HEAD: usize = size_of::<DataHead>();
    let bytes: [u8; HEAD + 4] = kani::any();
    let len: usize = kani::any();
    kani::assume(len <= bytes.len());
    let at = std::mem::offset_of!(DataHead, len);
    let declared = u32::from_ne_bytes(bytes[at..at + 4].try_into().unwrap()) as usize;
    let packet = Packed::from_bytes(&bytes[..len]);
    assert_eq!(
        packet.is_some(),
        len >= HEAD && declared <= len.saturating_sub(HEAD)
    );
    if let Some(record) = packet {
        assert_eq!(record.bytes().len(), HEAD + declared);
        assert_eq!(record.head().len as usize, declared);
    }
}
