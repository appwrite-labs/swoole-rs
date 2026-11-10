use std::alloc::{GlobalAlloc, Layout, System};
use std::cell::Cell;
use std::ptr;
thread_local! { pub(crate) static FAIL_AFTER: Cell<Option<usize>> = const { Cell::new(None) }; }
struct FailureAllocator;
fn fail() -> bool {
    FAIL_AFTER
        .try_with(|counter| match counter.get() {
            None => false,
            Some(0) => {
                counter.set(None);
                true
            }
            Some(n) => {
                counter.set(Some(n - 1));
                false
            }
        })
        .unwrap_or(false)
}
unsafe impl GlobalAlloc for FailureAllocator {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        if fail() {
            ptr::null_mut()
        } else {
            unsafe { System.alloc(layout) }
        }
    }
    unsafe fn realloc(&self, data: *mut u8, layout: Layout, size: usize) -> *mut u8 {
        if fail() {
            ptr::null_mut()
        } else {
            unsafe { System.realloc(data, layout, size) }
        }
    }
    unsafe fn dealloc(&self, data: *mut u8, layout: Layout) {
        unsafe {
            System.dealloc(data, layout);
        }
    }
}
#[global_allocator]
static ALLOCATOR: FailureAllocator = FailureAllocator;

#[test]
fn global_page_oom_fails_cleanly_and_bump_allocation_never_allocates() {
    use crate::global::Global;
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
