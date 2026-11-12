// SPDX-License-Identifier: Apache-2.0
//! The C ABI over crates/error: its `swr_v1_*` exports, upstream's C++ symbols, libc's
//! messages for system errors, and the ignored-error handles native code owns.
#![cfg_attr(
    not(test),
    deny(
        clippy::undocumented_unsafe_blocks,
        clippy::multiple_unsafe_ops_per_block
    )
)]

use std::alloc::{Layout, alloc, dealloc};
use std::ffi::c_char;
use std::sync::atomic::{AtomicPtr, Ordering};
use swoole_rs_error::IgnoredErrors;

// The archive's ABI queries live here, not in crates/ffi, whose object holds
// swoole_init: a C consumer that asks for them links no lifecycle.
/// Return the supported C ABI version.
#[unsafe(export_name = "swr_v1_abi_version")]
#[must_use]
pub extern "C" fn export_abi_version() -> u32 {
    swoole_rs_error::swr_v1_abi_version()
}

/// Return the migrated kernels: every module, as the bits `MODULES` in `tools/check_migration.py` names.
#[unsafe(export_name = "swr_v1_feature_bits")]
#[must_use]
pub extern "C" fn export_feature_bits() -> u64 {
    swoole_rs_error::swr_v1_feature_bits()
}

/// Return an immutable process-lifetime message, never to be freed; unknown is null.
#[unsafe(export_name = "swr_v1_error_message")]
#[must_use]
pub extern "C" fn export_error_message(code: i32) -> *const c_char {
    swoole_rs_error::swr_v1_error_message(code)
}

/// Resolve known codes, invoking an optional nonthrowing native formatter only on misses.
/// The formatter must not enter Zend, retain Rust state, or unwind through this frame.
#[unsafe(export_name = "swr_v1_error_resolve")]
#[must_use]
pub extern "C" fn export_error_resolve(
    code: i32,
    fallback: Option<extern "C" fn(i32) -> *const c_char>,
) -> *const c_char {
    swoole_rs_error::swr_v1_error_resolve(code, fallback)
}

/// `sw_error`: this thread's last-error message buffer.
#[unsafe(export_name = "swr_v1_error_buffer")]
#[must_use]
pub extern "C" fn export_error_buffer() -> *mut c_char {
    swoole_rs_error::swr_v1_error_buffer()
}

/// Resolve system, known and unknown errors without heap allocation. An unknown code's
/// message is this thread's buffer, overwritten by the thread's next unknown code.
#[unsafe(export_name = "swr_v1_error_strerror")]
#[must_use]
pub extern "C" fn export_error_strerror(
    code: i32,
    system: Option<extern "C" fn(i32) -> *const c_char>,
) -> *const c_char {
    swoole_rs_error::strerror_with(code, system)
}

unsafe extern "C" {
    safe fn strerror(code: i32) -> *const c_char;
}

/// libc's message for a system error.
#[must_use]
pub extern "C" fn system_error(code: i32) -> *const c_char {
    strerror(code)
}

/// `const char *swoole_strerror(int)`: system messages below 500, as upstream.
#[cfg_attr(
    feature = "upstream-symbols",
    unsafe(export_name = "_Z15swoole_strerrori")
)]
#[must_use]
pub extern "C" fn swoole_strerror(code: i32) -> *const c_char {
    swoole_rs_error::strerror_with(code, Some(system_error))
}

/// `const char *swoole_get_last_error_msg()`.
#[cfg_attr(
    feature = "upstream-symbols",
    unsafe(export_name = "_Z25swoole_get_last_error_msgv")
)]
#[must_use]
pub extern "C" fn export_get_last_error_msg() -> *const c_char {
    swoole_rs_error::swoole_get_last_error_msg()
}

/// `void swoole_clear_last_error_msg()`.
#[cfg_attr(
    feature = "upstream-symbols",
    unsafe(export_name = "_Z27swoole_clear_last_error_msgv")
)]
pub extern "C" fn export_clear_last_error_msg() {
    swoole_rs_error::swoole_clear_last_error_msg();
}

/// `void swoole_throw_error(int)`: reports the code and aborts.
#[cfg_attr(
    feature = "upstream-symbols",
    unsafe(export_name = "_Z18swoole_throw_errori")
)]
pub extern "C" fn export_throw_error(code: i32) -> ! {
    swoole_rs_error::swoole_throw_error(code)
}

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
    // SAFETY: a non-null slot holds this API's live handle, and access is serialized.
    if let Some(values) = unsafe { slot.as_mut() } {
        return if values.try_insert(code).is_ok() {
            0
        } else {
            -2
        };
    }
    let Ok(values) = IgnoredErrors::try_with(code) else {
        return -2;
    };
    // Not a Box: Box::new aborts when it cannot allocate, and this reports -2.
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
    unsafe { handle.write(values) };
    *slot = handle;
    0
}

/// Whether a null or live handle holds `code`.
#[unsafe(no_mangle)]
#[must_use]
pub extern "C" fn swr_v1_error_is_ignored(state: Option<&IgnoredErrors>, code: i32) -> i32 {
    i32::from(state.is_some_and(|values| values.contains(code)))
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
#[must_use]
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
        FAIL.with(|flag| flag.set(1));
        let message = export_error_strerror(i32::MAX, None);
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
    fn unknown_messages_are_per_thread() {
        let colon = if cfg!(target_vendor = "apple") {
            ":"
        } else {
            ""
        };
        let barrier = std::sync::Barrier::new(4);
        std::thread::scope(|scope| {
            for thread in 0..4 {
                let barrier = &barrier;
                scope.spawn(move || {
                    let code = i32::MAX - thread;
                    let message = export_error_strerror(code, None);
                    // Every thread formats its own code before any reads its message back.
                    barrier.wait();
                    for _ in 0..1000 {
                        assert_eq!(
                            export_error_strerror(code, None),
                            message,
                            "one buffer per thread"
                        );
                    }
                    barrier.wait();
                    let text = unsafe { std::ffi::CStr::from_ptr(message) }
                        .to_str()
                        .unwrap();
                    assert_eq!(text, format!("Unknown error{colon} {code}"));
                });
            }
        });
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
            let capacity = (*state).capacity();
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

    #[test]
    fn last_error_buffer_is_per_thread_and_clears() {
        let buffer = export_error_buffer();
        unsafe { buffer.copy_from_nonoverlapping(c"sentinel".as_ptr(), 9) };
        assert_eq!(export_get_last_error_msg(), buffer.cast_const());
        assert_eq!(unsafe { std::ffi::CStr::from_ptr(buffer) }, c"sentinel");
        assert_eq!(
            swoole_rs_error::last_error_message().as_c_str(),
            c"sentinel"
        );
        let other = std::thread::spawn(|| {
            let other = export_error_buffer();
            (other as usize, unsafe { *other })
        })
        .join()
        .unwrap();
        assert_ne!(other.0, buffer as usize, "each thread has its own buffer");
        assert_eq!(other.1, 0);
        export_clear_last_error_msg();
        assert_eq!(unsafe { *buffer }, 0);
        assert_eq!(swoole_rs_error::last_error_message().as_c_str(), c"");
    }

    #[test]
    #[cfg_attr(miri, ignore = "Miri has no strerror")]
    fn strerror_text_agrees_with_swoole_strerror() {
        let system = |code| {
            let text = unsafe { std::ffi::CStr::from_ptr(system_error(code)) };
            text.to_string_lossy().into_owned()
        };
        // ENOENT, success, the last system code, then known and unknown Swoole codes.
        for code in [2, 0, 499, 500, 501, 1004, 9999, i32::MAX] {
            let native = unsafe { std::ffi::CStr::from_ptr(swoole_strerror(code)) }.to_owned();
            let text = swoole_rs_error::strerror_text(code, system);
            assert_eq!(*text, *native, "code {code}");
        }
    }
}
