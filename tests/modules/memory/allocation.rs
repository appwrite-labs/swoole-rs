use crate::heap::*;
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
fn heap_oom_is_transactional_and_removal_never_allocates() {
    unsafe {
        for nth in 0..=2 {
            let mut heap = ptr::dangling_mut();
            FAIL_AFTER.set(Some(nth));
            let status = swr_v1_memory_heap_create(8, 0, &raw mut heap);
            FAIL_AFTER.set(None);
            if nth < 2 {
                assert_eq!(status, -1);
                assert!(heap.is_null());
            } else {
                assert_eq!(status, 0);
                swr_v1_memory_heap_release(heap);
            }
        }
        for nth in 0..=2 {
            let mut heap = ptr::null_mut();
            assert_eq!(swr_v1_memory_heap_create(129, 0, &raw mut heap), 0);
            let first = swr_v1_memory_heap_push(heap, 10, ptr::without_provenance_mut(1));
            assert!(!first.is_null());
            // Exhaust the bounded slab so this sweep still covers both vector
            // growth and node allocation, preserving the cold-failure checks.
            for id in 2..=129 {
                assert!(
                    !swr_v1_memory_heap_push(heap, 10, ptr::without_provenance_mut(id)).is_null()
                );
            }
            FAIL_AFTER.set(Some(nth));
            let second = swr_v1_memory_heap_push(heap, 5, ptr::without_provenance_mut(130));
            FAIL_AFTER.set(None);
            assert_eq!(
                swr_v1_memory_heap_count(heap),
                if nth < 2 { 129 } else { 130 }
            );
            assert_eq!(second.is_null(), nth < 2);
            assert!(
                (0..swr_v1_memory_heap_count(heap))
                    .any(|i| swr_v1_memory_heap_at(heap, i) == first)
            );
            FAIL_AFTER.set(Some(0));
            let removed = swr_v1_memory_heap_remove(heap, first);
            let no_allocation = FAIL_AFTER.replace(None);
            assert_eq!(removed, 0);
            assert_eq!(no_allocation, Some(0));
            swr_v1_memory_heap_release(heap);
        }
    }
}

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

#[test]
fn heap_reuses_one_removed_node_without_allocating_and_keeps_live_handles_stable() {
    unsafe {
        let mut heap = ptr::null_mut();
        assert_eq!(swr_v1_memory_heap_create(2, 0, &raw mut heap), 0);
        let first = swr_v1_memory_heap_push(heap, 10, ptr::without_provenance_mut(1));
        let second = swr_v1_memory_heap_push(heap, 20, ptr::without_provenance_mut(2));
        assert!(!first.is_null() && !second.is_null());
        assert_eq!(swr_v1_memory_heap_pop(heap).addr(), 1);
        FAIL_AFTER.set(Some(0));
        let reused = swr_v1_memory_heap_push(heap, 5, ptr::without_provenance_mut(3));
        let no_allocation = FAIL_AFTER.replace(None);
        assert!(!reused.is_null());
        assert_eq!(no_allocation, Some(0));
        assert_eq!(swr_v1_memory_heap_count(heap), 2);
        // The surviving live handle remains valid when a different node is reused.
        assert_eq!(swr_v1_memory_heap_update(heap, 1, second), 0);
        assert_eq!(swr_v1_memory_heap_pop(heap).addr(), 2);
        assert_eq!(swr_v1_memory_heap_pop(heap).addr(), 3);
        assert_eq!(swr_v1_memory_heap_count(heap), 0);
        FAIL_AFTER.set(Some(0));
        swr_v1_memory_heap_release(heap);
        assert_eq!(FAIL_AFTER.replace(None), Some(0));
    }
}
