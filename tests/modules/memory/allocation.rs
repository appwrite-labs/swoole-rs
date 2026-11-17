use crate::test_allocator::FAIL_ONCE_AFTER as FAIL_AFTER;

#[test]
fn global_page_oom_fails_cleanly_and_bump_allocation_never_allocates() {
    use crate::memory::global::Global;
    let mut pages = Global::new(64, false);
    FAIL_AFTER.set(Some(0));
    let added = pages.add_page();
    FAIL_AFTER.set(None);
    assert!(!added);
    assert_eq!(pages.memory_size(), 0);
    assert!(pages.add_page());
    FAIL_AFTER.set(Some(0));
    let block = pages.alloc(1);
    let no_allocation = FAIL_AFTER.replace(None);
    assert!(!block.is_null());
    assert_eq!(no_allocation, Some(0));
}
