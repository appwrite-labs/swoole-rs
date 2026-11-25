use super::tests::*;
use std::slice;

#[test]
fn warm_cache_preserves_live_fronts_and_reuses_blocks_without_allocating() {
    let owner = Owned::new(1024);
    let payload = vec![0xa5; 65536];
    for _ in 0..2 {
        assert_eq!(owner.append(&payload), Ok(()));
        for _ in 0..64 {
            owner.pop();
        }
    }
    assert_eq!(unsafe { &*owner.0 }.recycled_bytes, 65536);
    FAIL_AFTER.with(|counter| counter.set(Some(0)));
    let status = owner.append(&payload);
    FAIL_AFTER.with(|counter| counter.set(None));
    assert_eq!(status, Ok(()));
    let front = owner.view();
    assert_eq!(
        unsafe { slice::from_raw_parts(front.data, 1024) },
        &payload[..1024]
    );
    for _ in 0..64 {
        owner.pop();
    }
    assert_eq!(owner.stats(), (0, 0));
    assert!(unsafe { &*owner.0 }.recycled_bytes <= 65536);
}

#[test]
fn bounded_append_accepts_exact_fit_and_preserves_bytes_on_refusal() {
    let mut bytes = super::BoundedBytes::new(3, 1);
    assert_eq!(bytes.append(b"a"), Ok(()));
    assert_eq!(bytes.append(b"bcd"), Err(3));
    assert_eq!(bytes.append(b"bc"), Ok(()));
    assert_eq!(bytes.append(b""), Ok(()));
    assert_eq!(bytes.append(b"d"), Err(3));
    assert_eq!(bytes.as_ref(), b"abc");
    let mut empty = super::BoundedBytes::new(0, 8192);
    assert_eq!(empty.append(b""), Ok(()));
    assert_eq!(empty.append(b"x"), Err(0));
}
