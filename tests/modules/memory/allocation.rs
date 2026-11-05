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
fn borrowed_column_index_disposal_never_allocates_or_disposes_native_tokens() {
    use crate::table_columns::*;
    unsafe {
        let mut columns = ptr::null_mut();
        assert_eq!(swr_v1_memory_table_columns_create(&mut columns), 0);
        let mut tokens = [0x1234u64, 0x5678];
        for (i, token) in tokens.iter_mut().enumerate() {
            let name = [b'a' + i as u8];
            assert_eq!(
                swr_v1_memory_table_columns_insert(
                    columns,
                    name.as_ptr(),
                    name.len(),
                    ptr::from_mut(token).cast()
                ),
                0
            );
        }
        // Frozen release still rejects loss of retained tokens. Only the new
        // borrowed-index contract permits disposal while the native owner lives.
        assert_eq!(swr_v1_memory_table_columns_release(columns), -1);
        FAIL_AFTER.set(Some(0));
        let result = swr_v1_memory_table_columns_discard(columns);
        let no_allocation = FAIL_AFTER.replace(None);
        assert_eq!(result, 0);
        assert_eq!(no_allocation, Some(0));
        assert_eq!(tokens, [0x1234, 0x5678]);
        assert_eq!(swr_v1_memory_table_columns_discard(ptr::null_mut()), -1);
    }
}

#[test]
fn heap_oom_is_transactional_and_removal_never_allocates() {
    unsafe {
        for nth in 0..=2 {
            let mut heap = ptr::dangling_mut();
            FAIL_AFTER.set(Some(nth));
            let status = swr_v1_memory_heap_create(8, 0, &mut heap);
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
            assert_eq!(swr_v1_memory_heap_create(129, 0, &mut heap), 0);
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
fn global_registry_oom_retains_native_page_custody_and_alloc_never_allocates() {
    use crate::global::*;
    unsafe {
        let mut state = ptr::dangling_mut();
        FAIL_AFTER.set(Some(0));
        let status = swr_v1_memory_global_create(64, &mut state);
        FAIL_AFTER.set(None);
        assert_eq!(status, -1);
        assert!(state.is_null());
        FAIL_AFTER.set(Some(0));
        let opened = swr_v1_memory_global_open(4096, 4096, 1, 41, &mut state);
        FAIL_AFTER.set(None);
        assert_eq!(opened, -1);
        assert!(state.is_null());
        assert_eq!(swr_v1_memory_global_create(64, &mut state), 0);
        let mut page = [0u64; 8];
        let pointer = page.as_mut_ptr().cast();
        FAIL_AFTER.set(Some(0));
        let failed = swr_v1_memory_global_add_page(state, pointer);
        FAIL_AFTER.set(None);
        assert_eq!(failed, -1);
        assert_eq!(swr_v1_memory_global_memory_size(state), 0);
        assert_eq!(swr_v1_memory_global_add_page(state, pointer), 0);
        let mut output = ptr::null_mut();
        FAIL_AFTER.set(Some(0));
        let allocated = swr_v1_memory_global_alloc(state, 1, &mut output);
        let no_allocation = FAIL_AFTER.replace(None);
        assert_eq!(allocated, 0);
        assert_eq!(no_allocation, Some(0));
        let mut effect = GlobalEffect::default();
        FAIL_AFTER.set(Some(0));
        let checked = swr_v1_memory_global_step(state, 1, 41, 1, &mut effect);
        let drained = swr_v1_memory_global_step(state, 2, 0, 0, &mut effect);
        let released = swr_v1_memory_global_release(state);
        let no_lifecycle_allocation = FAIL_AFTER.replace(None);
        assert_eq!((checked, drained, released), (0, 0, 0));
        assert_eq!(effect.page, pointer);
        assert_eq!(effect.action, 3);
        assert_eq!(no_lifecycle_allocation, Some(0));
    }
}

#[test]
fn column_index_oom_keeps_tokens_native_and_lookup_disposal_do_not_allocate() {
    use crate::table_columns::*;
    unsafe {
        let mut columns = ptr::dangling_mut();
        FAIL_AFTER.set(Some(0));
        let status = swr_v1_memory_table_columns_create(&mut columns);
        FAIL_AFTER.set(None);
        assert_eq!(status, -1);
        assert!(columns.is_null());
        for nth in 0..=2 {
            assert_eq!(swr_v1_memory_table_columns_create(&mut columns), 0);
            let token = ptr::without_provenance_mut(1);
            FAIL_AFTER.set(Some(nth));
            let status = swr_v1_memory_table_columns_insert(columns, b"field".as_ptr(), 5, token);
            FAIL_AFTER.set(None);
            assert_eq!(status, if nth < 2 { -1 } else { 0 });
            assert_eq!(
                swr_v1_memory_table_columns_count(columns),
                usize::from(nth == 2)
            );
            FAIL_AFTER.set(Some(0));
            let found = swr_v1_memory_table_columns_get(columns, b"field".as_ptr(), 5);
            let popped = swr_v1_memory_table_columns_pop(columns);
            let released = swr_v1_memory_table_columns_release(columns);
            let untouched = FAIL_AFTER.replace(None);
            assert_eq!(found, if nth == 2 { token } else { ptr::null_mut() });
            assert_eq!(popped, found);
            assert_eq!(released, 0);
            assert_eq!(untouched, Some(0));
        }
    }
}

#[test]
fn heap_reuses_one_removed_node_without_allocating_and_keeps_live_handles_stable() {
    unsafe {
        let mut heap = ptr::null_mut();
        assert_eq!(swr_v1_memory_heap_create(2, 0, &mut heap), 0);
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
