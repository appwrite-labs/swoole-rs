use super::tests::*;
use super::*;

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
