// SPDX-License-Identifier: Apache-2.0
//! The C ABI over crates/error: its `swr_v1_*` exports, upstream's C++ symbols, libc's
//! messages for system errors, and the process-global ignored-error set.
#![cfg_attr(
    not(test),
    deny(
        clippy::undocumented_unsafe_blocks,
        clippy::multiple_unsafe_ops_per_block
    )
)]

use std::collections::TryReserveError;
use std::ffi::c_char;
use std::sync::{PoisonError, RwLock};
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

/// Resolve system, known and unknown errors. An unknown Swoole code's message is
/// this thread's buffer, overwritten by the thread's next unknown Swoole code.
#[unsafe(export_name = "swr_v1_error_strerror")]
#[must_use]
pub extern "C" fn export_error_strerror(
    code: i32,
    system: Option<extern "C" fn(i32) -> *const c_char>,
) -> *const c_char {
    swoole_rs_error::strerror_with(code, system)
}

unsafe extern "C" {
    // The calling thread's errno cell, used synchronously and never sent away.
    #[cfg_attr(target_os = "linux", link_name = "__errno_location")]
    #[cfg_attr(target_os = "android", link_name = "__errno")]
    #[cfg_attr(
        not(any(target_os = "linux", target_os = "android")),
        link_name = "__error"
    )]
    safe fn errno_location() -> &'static std::cell::Cell<i32>;
}

/// A buffer and its inseparable capacity for the reentrant libc formatter.
mod message_buffer {
    use std::marker::PhantomData;
    type Brand<'id> = PhantomData<fn(&'id ()) -> &'id ()>;
    #[repr(transparent)]
    pub(super) struct Out<'id>(*mut std::ffi::c_char, Brand<'id>);
    #[repr(transparent)]
    pub(super) struct Count<'id>(usize, Brand<'id>);

    unsafe extern "C" {
        // libc writes only the lent buffer, retains nothing and accepts every code.
        #[cfg_attr(
            all(target_os = "linux", target_env = "gnu"),
            link_name = "__xpg_strerror_r"
        )]
        pub(super) safe fn strerror_r<'id>(code: i32, out: Out<'id>, size: Count<'id>) -> i32;
    }

    pub(super) fn with<T>(
        bytes: &mut [u8],
        call: impl for<'id> FnOnce(Out<'id>, Count<'id>) -> T,
    ) -> T {
        call(
            Out(bytes.as_mut_ptr().cast(), PhantomData),
            Count(bytes.len(), PhantomData),
        )
    }
}

/// libc's exact message bytes, copied by its reentrant formatter, and whether the
/// code is known. Unknown codes keep libc's shorter, next-call lifetime.
fn system_message(code: i32) -> (std::ffi::CString, bool) {
    let saved = errno_location().get();
    let mut bytes = vec![0u8; 128];
    let result = loop {
        let result = message_buffer::with(&mut bytes, |out, size| {
            message_buffer::strerror_r(code, out, size)
        });
        if result != libc::ERANGE {
            break result;
        }
        bytes.resize(bytes.len() * 2, 0);
    };
    // Apple's strerror sets EINVAL for unknown codes; GNU and musl preserve errno.
    errno_location().set(if cfg!(target_vendor = "apple") && result != 0 {
        libc::EINVAL
    } else {
        saved
    });
    (
        std::ffi::CStr::from_bytes_until_nul(&bytes)
            .unwrap_or(c"")
            .to_owned(),
        result == 0,
    )
}

/// libc's message for a system error. Known messages are retained as immutable
/// bytes for C callers that keep the pointer. Unknown messages live until this
/// thread's next unknown code or exit. Formatting on every call preserves locale.
#[must_use]
pub extern "C" fn system_error(code: i32) -> *const c_char {
    static MESSAGES: std::sync::Mutex<Vec<std::ffi::CString>> = std::sync::Mutex::new(Vec::new());
    let (message, known) = system_message(code);
    if !known {
        return unknown_system_message(message);
    }
    let mut messages = MESSAGES.lock().unwrap_or_else(PoisonError::into_inner);
    if let Some(stored) = messages.iter().find(|stored| **stored == message) {
        return stored.as_ptr();
    }
    messages.push_mut(message).as_ptr()
}

/// Apple's unknown-message buffer has a fixed `NL_TEXTMAX` address even when a
/// later lookup overwrites it. Keep that lifetime, independently for each thread.
#[cfg(target_vendor = "apple")]
fn unknown_system_message(message: std::ffi::CString) -> *const c_char {
    thread_local! {
        static UNKNOWN: [std::cell::Cell<c_char>; 2048] = const { [const { std::cell::Cell::new(0) }; 2048] };
    }
    let bytes = message.into_bytes();
    UNKNOWN.with(|buffer| {
        let bytes = bytes.iter().take(2047).chain(std::iter::once(&0));
        for (cell, &byte) in buffer.iter().zip(bytes) {
            cell.set(c_char::from_ne_bytes([byte]));
        }
        buffer.as_ptr().cast()
    })
}

/// GNU's unknown-message pointer lasts until the next unknown code or thread exit.
#[cfg(not(target_vendor = "apple"))]
fn unknown_system_message(message: std::ffi::CString) -> *const c_char {
    thread_local! {
        static UNKNOWN: std::cell::Cell<Vec<u8>> = const { std::cell::Cell::new(Vec::new()) };
    }
    let bytes = message.into_bytes_with_nul();
    let pointer = bytes.as_ptr().cast();
    UNKNOWN.set(bytes);
    pointer
}

/// `const char *swoole_strerror(int)`: system messages below 500, as upstream.
#[unsafe(export_name = "_Z15swoole_strerrori")]
#[must_use]
pub extern "C" fn swoole_strerror(code: i32) -> *const c_char {
    swoole_rs_error::strerror_with(code, Some(system_error))
}

/// `const char *swoole_get_last_error_msg()`.
#[unsafe(export_name = "_Z25swoole_get_last_error_msgv")]
#[must_use]
pub extern "C" fn export_get_last_error_msg() -> *const c_char {
    swoole_rs_error::swoole_get_last_error_msg()
}

/// `void swoole_clear_last_error_msg()`.
#[unsafe(export_name = "_Z27swoole_clear_last_error_msgv")]
pub extern "C" fn export_clear_last_error_msg() {
    swoole_rs_error::swoole_clear_last_error_msg();
}

/// `void swoole_throw_error(int)`: reports the code and aborts.
#[unsafe(export_name = "_Z18swoole_throw_errori")]
pub extern "C" fn export_throw_error(code: i32) -> ! {
    swoole_rs_error::swoole_throw_error(code)
}

/// The process-global set behind `swoole_ignore_error` and `swoole_is_ignored_error`:
/// empty until the first insert, then one set. The set sits in a one-element `Vec`
/// rather than in place: allocating that holder is a failure point of its own, as
/// upstream's set handle was. Inserts come once at startup, so a read's lock is never
/// contended.
struct Ignored(RwLock<Vec<IgnoredErrors>>);

static IGNORED: Ignored = Ignored::new();

impl Ignored {
    const fn new() -> Self {
        Self(RwLock::new(Vec::new()))
    }

    /// Insert idempotently. When allocation fails the set, or its absence, is unchanged.
    fn insert(&self, code: i32) -> Result<(), TryReserveError> {
        let mut holder = self.0.write().unwrap_or_else(PoisonError::into_inner);
        if let Some(values) = holder.first_mut() {
            return values.try_insert(code);
        }
        let values = IgnoredErrors::try_with(code)?;
        // Box::new would abort when it cannot allocate.
        holder.try_reserve_exact(1)?;
        holder.push(values);
        Ok(())
    }

    fn contains(&self, code: i32) -> bool {
        let holder = self.0.read().unwrap_or_else(PoisonError::into_inner);
        holder.first().is_some_and(|values| values.contains(code))
    }
}

/// `void swoole_ignore_error(int)`. Upstream threw `std::bad_alloc` when the set could not
/// grow, which nothing in the extension catches, so it terminated; this aborts.
#[unsafe(export_name = "_Z19swoole_ignore_errori")]
pub extern "C" fn swoole_ignore_error(code: i32) {
    if IGNORED.insert(code).is_err() {
        #[expect(
            clippy::unused_result_ok,
            reason = "the process aborts next, stderr or not"
        )]
        std::io::Write::write_all(
            &mut std::io::stderr(),
            b"swoole_ignore_error: out of memory\n",
        )
        .ok();
        #[expect(
            clippy::disallowed_methods,
            reason = "the line above says why; this crate sits below corebase"
        )]
        std::process::abort();
    }
}

/// `bool swoole_is_ignored_error(int)`.
#[unsafe(export_name = "_Z23swoole_is_ignored_errori")]
#[must_use]
pub extern "C" fn swoole_is_ignored_error(code: i32) -> bool {
    IGNORED.contains(code)
}

#[cfg(test)]
mod tests {
    use super::*;
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
        let state = Ignored::new();
        let empty = || state.0.read().unwrap().is_empty();
        assert!(!state.contains(1));
        FAIL.with(|flag| flag.set(1));
        assert!(state.insert(1).is_err());
        assert!(empty());
        FAIL.with(|flag| flag.set(2)); // Fail handle allocation after bucket allocation.
        assert!(state.insert(1).is_err());
        assert!(empty());
        assert!(state.insert(1).is_ok());
        // Fill the actual allocation so the next distinct value must reserve.
        let capacity = state.0.read().unwrap()[0].capacity();
        for code in 2..=i32::try_from(capacity).expect("capacity fits an error code") {
            assert!(state.insert(code).is_ok());
        }
        FAIL.with(|flag| flag.set(1));
        assert!(state.insert(i32::MIN).is_err());
        assert!(!state.contains(i32::MIN));
        assert!(state.contains(1));
        assert!(state.insert(1).is_ok());
    }

    /// The former `tests/modules/error/allocator.cc`, on the process-global set: bucket,
    /// handle and growth allocation failures keep membership, a duplicate does not
    /// allocate, a retry succeeds and `swoole_ignore_error` aborts on a failure.
    #[test]
    #[cfg_attr(miri, ignore = "Miri has no fork")]
    fn global_set_allocation_faults() {
        fn fail_insert(code: i32, allocation: usize) {
            FAIL.with(|flag| flag.set(allocation));
            assert!(IGNORED.insert(code).is_err());
            assert_eq!(FAIL.with(Cell::get), 0);
            assert!(!swoole_is_ignored_error(code));
        }
        fail_insert(1, 1); // Bucket allocation.
        fail_insert(1, 2); // Opaque handle after a successful bucket allocation.
        swoole_ignore_error(1);
        assert!(swoole_is_ignored_error(1));
        FAIL.with(|flag| flag.set(1));
        swoole_ignore_error(1); // Duplicate must not consume an allocation failure.
        assert_eq!(FAIL.with(Cell::get), 1);
        let mut failed = 0;
        for code in 2..100 {
            if IGNORED.insert(code).is_err() {
                failed = code;
                break;
            }
        }
        assert!(failed > 1 && FAIL.with(Cell::get) == 0);
        assert!(!swoole_is_ignored_error(failed));
        for code in 1..failed {
            assert!(swoole_is_ignored_error(code));
        }
        let child = unsafe { libc::fork() };
        assert!(child >= 0);
        if child == 0 {
            FAIL.with(|flag| flag.set(1));
            swoole_ignore_error(failed);
            unsafe { libc::_exit(0) };
        }
        let mut status = 0;
        assert_eq!(unsafe { libc::waitpid(child, &raw mut status, 0) }, child);
        assert!(libc::WIFSIGNALED(status) && libc::WTERMSIG(status) == libc::SIGABRT);
        swoole_ignore_error(failed);
        assert!(swoole_is_ignored_error(failed));
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
    fn system_messages_keep_their_bytes_across_threads_and_later_errors() {
        let known = system_error(libc::ENOENT);
        let known_text = unsafe { std::ffi::CStr::from_ptr(known) }.to_owned();
        let unknown = system_error(-1);
        let unknown_text = unsafe { std::ffi::CStr::from_ptr(unknown) }.to_owned();
        std::thread::spawn(|| {
            for code in [libc::EACCES, -2, 499] {
                let pointer = system_error(code);
                assert!(!unsafe { std::ffi::CStr::from_ptr(pointer) }.is_empty());
            }
        })
        .join()
        .unwrap();
        assert_eq!(
            unsafe { std::ffi::CStr::from_ptr(unknown) },
            unknown_text.as_c_str()
        );
        for code in [libc::EACCES, -2, 499] {
            _ = system_error(code);
        }
        assert_eq!(
            unsafe { std::ffi::CStr::from_ptr(known) },
            known_text.as_c_str()
        );
        assert_eq!(system_error(libc::ENOENT), known);
    }

    #[test]
    #[cfg_attr(miri, ignore = "Miri has no strerror")]
    fn reentrant_system_messages_match_libc_bytes_and_errno() {
        for code in [-2_147_483_647, -1, 0, libc::ENOENT, libc::EACCES, 499] {
            errno_location().set(libc::EBADF);
            let expected = unsafe { std::ffi::CStr::from_ptr(libc::strerror(code)) }.to_owned();
            let expected_errno = errno_location().get();
            errno_location().set(libc::EBADF);
            let actual = system_error(code);
            assert_eq!(
                unsafe { std::ffi::CStr::from_ptr(actual) },
                expected.as_c_str(),
                "code {code}"
            );
            assert_eq!(errno_location().get(), expected_errno, "errno for {code}");
        }
    }

    #[test]
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
