//! Thread-local allocation failures for the original buffer, memory and timer tests.
use std::alloc::{GlobalAlloc, Layout, System};
use std::cell::Cell;
use std::ptr;

thread_local! {
    pub(crate) static FAIL_AFTER: Cell<Option<usize>> = const { Cell::new(None) };
    pub(crate) static FAIL_ONCE_AFTER: Cell<Option<usize>> = const { Cell::new(None) };
}

fn fail() -> bool {
    let step = |counter: &Cell<Option<usize>>, once: bool| match counter.get() {
        None => false,
        Some(0) => {
            if once {
                counter.set(None);
            }
            true
        }
        Some(n) => {
            counter.set(Some(n - 1));
            false
        }
    };
    FAIL_AFTER
        .try_with(|counter| step(counter, false))
        .unwrap_or(false)
        || FAIL_ONCE_AFTER
            .try_with(|counter| step(counter, true))
            .unwrap_or(false)
}

struct FailureAllocator;
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
