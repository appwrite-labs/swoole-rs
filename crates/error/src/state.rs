// SPDX-License-Identifier: Apache-2.0
//! Process-local ignored errors; native code owns the handle and exception boundary.
use std::alloc::{Layout, alloc, dealloc};
use std::collections::HashSet;
use std::sync::atomic::{AtomicPtr, Ordering};

pub struct IgnoredErrors(HashSet<i32>);

/// Insert idempotently. Invalid output is -1; allocation failure is -2.
/// A failed first insertion leaves the native handle null.
/// # Safety
/// `state` must be writable and contain null or this API's live handle. Access
/// must be serialized; the original native set also forbids concurrent mutation.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn swr_v1_error_ignore(state: *mut *mut IgnoredErrors, code: i32) -> i32 {
    // SAFETY: the caller passes null or a writable slot, and serializes access.
    let Some(slot) = (unsafe { state.as_mut() }) else {
        return -1;
    };
    if slot.is_null() {
        let mut values = HashSet::new();
        if values.try_reserve(1).is_err() {
            return -2;
        }
        values.insert(code);
        #[expect(
            clippy::cast_ptr_alignment,
            reason = "alloc honours Layout::new::<IgnoredErrors>()"
        )]
        // SAFETY: `IgnoredErrors` is not zero-sized; null is handled below.
        let handle = unsafe { alloc(Layout::new::<IgnoredErrors>()) }.cast::<IgnoredErrors>();
        if handle.is_null() {
            return -2;
        }
        // SAFETY: `handle` is a fresh, non-null allocation with this type's layout.
        unsafe { handle.write(IgnoredErrors(values)) };
        *slot = handle;
    } else {
        // SAFETY: a non-null slot holds this API's live handle, and access is serialized.
        let values = &mut unsafe { &mut **slot }.0;
        if !values.contains(&code) {
            if values.try_reserve(1).is_err() {
                return -2;
            }
            values.insert(code);
        }
    }
    0
}

/// Whether a null or live handle holds `code`.
#[unsafe(no_mangle)]
pub extern "C" fn swr_v1_error_is_ignored(state: Option<&IgnoredErrors>, code: i32) -> i32 {
    i32::from(state.is_some_and(|values| values.0.contains(&code)))
}

/// # Safety
/// A nonnull handle must be uniquely owned and released exactly once.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn swr_v1_error_ignored_release(state: *mut IgnoredErrors) {
    if !state.is_null() {
        // SAFETY: the caller owns this live handle uniquely and releases it once.
        unsafe { state.drop_in_place() };
        // SAFETY: `state` came from `alloc` with this layout and its value was just dropped.
        unsafe { dealloc(state.cast(), Layout::new::<IgnoredErrors>()) };
    }
}

/// The process-global set behind `swoole_ignore_error` and `swoole_is_ignored_error`.
static IGNORED: AtomicPtr<IgnoredErrors> = AtomicPtr::new(std::ptr::null_mut());

/// `swr_v1_error_ignore` on the process-global set; `swoole_ignore_error` aborts on -2.
/// # Safety
/// Access must be serialized, as upstream's unsynchronized set requires.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn swr_v1_error_ignore_global(code: i32) -> i32 {
    // SAFETY: the slot holds null or this API's live handle, and access is serialized.
    unsafe { swr_v1_error_ignore(IGNORED.as_ptr(), code) }
}

/// `void swoole_ignore_error(int)`. Upstream threw `std::bad_alloc` when the set could not
/// grow, which nothing in the extension catches, so it terminated; this aborts.
#[cfg_attr(
    feature = "upstream-symbols",
    unsafe(export_name = "_Z19swoole_ignore_errori")
)]
pub extern "C" fn swoole_ignore_error(code: i32) {
    // SAFETY: the extension serializes ignored-error writes, as upstream required.
    if unsafe { swr_v1_error_ignore_global(code) } != 0 {
        std::io::Write::write_all(
            &mut std::io::stderr(),
            b"swoole_ignore_error: out of memory\n",
        )
        .ok();
        std::process::abort();
    }
}

/// `bool swoole_is_ignored_error(int)`.
#[cfg_attr(
    feature = "upstream-symbols",
    unsafe(export_name = "_Z23swoole_is_ignored_errori")
)]
pub extern "C" fn swoole_is_ignored_error(code: i32) -> bool {
    // SAFETY: the slot holds null or this API's live handle, which is never released.
    let state = unsafe { IGNORED.load(Ordering::Relaxed).as_ref() };
    swr_v1_error_is_ignored(state, code) != 0
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::alloc::{GlobalAlloc, System};
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
        unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
            unsafe { System.dealloc(ptr, layout) }
        }
    }
    #[global_allocator]
    static ALLOCATOR: FailureAllocator = FailureAllocator;

    #[test]
    fn unknown_format_does_not_allocate() {
        // This test never overlaps another unknown-message test; other tests use
        // only ignored-state functions. The lookup's mutable buffer is serialized.
        FAIL.with(|flag| flag.set(1));
        let message = unsafe { crate::swr_v1_error_strerror(i32::MAX, None) };
        let untouched = FAIL.with(|flag| flag.replace(0));
        assert_eq!(untouched, 1);
        let bytes = unsafe { std::ffi::CStr::from_ptr(message) }.to_bytes();
        assert_eq!(
            bytes,
            if cfg!(target_vendor = "apple") {
                b"Unknown error: 2147483647".as_slice()
            } else {
                b"Unknown error 2147483647".as_slice()
            }
        );
    }

    #[test]
    fn allocation_failure_preserves_membership_and_publication() {
        let mut state: *mut IgnoredErrors = std::ptr::null_mut();
        unsafe {
            assert_eq!(swr_v1_error_ignore(std::ptr::null_mut(), 1), -1);
            assert_eq!(swr_v1_error_is_ignored(state.as_ref(), 1), 0);
            FAIL.with(|flag| flag.set(1));
            assert_eq!(swr_v1_error_ignore(&raw mut state, 1), -2);
            assert!(state.is_null());
            FAIL.with(|flag| flag.set(2)); // Fail handle allocation after bucket allocation.
            assert_eq!(swr_v1_error_ignore(&raw mut state, 1), -2);
            assert!(state.is_null());
            assert_eq!(swr_v1_error_ignore(&raw mut state, 1), 0);
            // Fill the actual allocation so the next distinct value must reserve.
            let capacity = (*state).0.capacity();
            for code in 2..=i32::try_from(capacity).expect("capacity fits an error code") {
                assert_eq!(swr_v1_error_ignore(&raw mut state, code), 0);
            }
            FAIL.with(|flag| flag.set(1));
            assert_eq!(swr_v1_error_ignore(&raw mut state, i32::MIN), -2);
            assert_eq!(swr_v1_error_is_ignored(state.as_ref(), i32::MIN), 0);
            assert_eq!(swr_v1_error_is_ignored(state.as_ref(), 1), 1);
            assert_eq!(swr_v1_error_ignore(&raw mut state, 1), 0);
            swr_v1_error_ignored_release(state);
            swr_v1_error_ignored_release(std::ptr::null_mut());
        }
    }
}
