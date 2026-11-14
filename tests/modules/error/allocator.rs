// SPDX-License-Identifier: Apache-2.0
// Test-only global allocator; the production error module is compiled unchanged.
/* ERROR_MODULE */
use std::alloc::{GlobalAlloc, Layout, System};
use std::cell::Cell;
thread_local! { static FAIL: Cell<usize> = const { Cell::new(0) }; }
struct FailureAllocator;
unsafe impl GlobalAlloc for FailureAllocator {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        if FAIL
            .try_with(|flag| {
                let left = flag.get();
                flag.set(left.saturating_sub(1));
                left == 1
            })
            .unwrap_or(false)
        {
            std::ptr::null_mut()
        } else {
            unsafe { System.alloc(layout) }
        }
    }
    unsafe fn dealloc(&self, pointer: *mut u8, layout: Layout) {
        unsafe { System.dealloc(pointer, layout) }
    }
}
#[global_allocator]
static ALLOCATOR: FailureAllocator = FailureAllocator;
#[unsafe(no_mangle)]
pub extern "C" fn swr_test_fail_allocation(countdown: usize) {
    FAIL.with(|flag| flag.set(countdown));
}
#[unsafe(no_mangle)]
pub extern "C" fn swr_test_allocation_countdown() -> usize {
    FAIL.with(Cell::get)
}
