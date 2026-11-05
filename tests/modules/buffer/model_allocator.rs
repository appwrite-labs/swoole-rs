//! Test fixture only: compile the unchanged production module with a fallible allocator.
// The native facade's real Global also owns production corebase records. Link
// that crate under the same allocator rather than substituting a fake Global.
extern crate swoole_rs_corebase as _;
use std::alloc::{GlobalAlloc, Layout, System};
use std::sync::atomic::{AtomicI64, AtomicU64, Ordering::Relaxed};
#[path = "../../../crates/buffer/src/lib.rs"]
mod implementation;
static REMAINING: AtomicI64 = AtomicI64::new(-1);
static ALLOCATIONS: AtomicU64 = AtomicU64::new(0);
static FAILURES: AtomicU64 = AtomicU64::new(0);
struct Controlled;
fn fail() -> bool {
    ALLOCATIONS.fetch_add(1, Relaxed);
    let remaining = REMAINING.load(Relaxed);
    if remaining < 0 {
        return false;
    }
    if remaining == 0 {
        REMAINING.store(-1, Relaxed); // Fail once; later operations can proceed.
        FAILURES.fetch_add(1, Relaxed);
        true
    } else {
        REMAINING.store(remaining - 1, Relaxed);
        false
    }
}
unsafe impl GlobalAlloc for Controlled {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        if fail() {
            std::ptr::null_mut()
        } else {
            unsafe { System.alloc(layout) }
        }
    }
    unsafe fn realloc(&self, data: *mut u8, layout: Layout, size: usize) -> *mut u8 {
        if fail() {
            std::ptr::null_mut()
        } else {
            unsafe { System.realloc(data, layout, size) }
        }
    }
    unsafe fn dealloc(&self, data: *mut u8, layout: Layout) {
        unsafe { System.dealloc(data, layout) }
    }
}
#[global_allocator]
static ALLOCATOR: Controlled = Controlled;
#[unsafe(no_mangle)]
pub extern "C" fn swr_model_fail_after(n: i64) {
    REMAINING.store(n, Relaxed);
    ALLOCATIONS.store(0, Relaxed);
    FAILURES.store(0, Relaxed);
}
#[unsafe(no_mangle)]
pub extern "C" fn swr_model_allocations() -> u64 {
    ALLOCATIONS.load(Relaxed)
}
#[unsafe(no_mangle)]
pub extern "C" fn swr_model_failures() -> u64 {
    FAILURES.load(Relaxed)
}
