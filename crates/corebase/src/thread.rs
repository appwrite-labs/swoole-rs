//! The lower Thread's OS handle and status. Native owns only its C++ callable:
//! pthread invokes that callable's native trampoline directly, with no Rust
//! child-entry frame for a C++ exception or pthread forced unwind to cross.
use std::{
    ffi::{c_int, c_void},
    ptr,
    sync::atomic::{AtomicI32, AtomicU8, Ordering},
};

#[repr(C)]
#[derive(Default)]
pub struct CoreThread {
    handle: libc::pthread_t,
    exit_status: c_int,
    living: u8,
}
unsafe fn status<'a>(record: *const CoreThread) -> &'a AtomicI32 {
    // SAFETY: the caller lends an initialized, aligned live record.
    let slot = unsafe { &raw const (*record).exit_status };
    // SAFETY: all concurrent accesses to this slot use this same atomic view.
    unsafe { AtomicI32::from_ptr(slot.cast_mut()) }
}
unsafe fn living<'a>(record: *const CoreThread) -> &'a AtomicU8 {
    // SAFETY: the caller lends an initialized, aligned live record.
    let slot = unsafe { &raw const (*record).living };
    // SAFETY: all concurrent accesses to this slot use this same atomic view.
    unsafe { AtomicU8::from_ptr(slot.cast_mut()) }
}
/// # Safety
/// `record` is a live owner used exclusively by its controlling thread.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn swr_v1_core_thread_joinable(record: *const CoreThread) -> bool {
    // SAFETY: the controlling thread alone reads/writes the OS handle.
    unsafe { (*record).handle != libc::pthread_t::default() }
}
/// # Safety
/// A live record whose running thread may publish its state concurrently.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn swr_v1_core_thread_alive(record: *const CoreThread) -> bool {
    // SAFETY: per the live-record contract; the state slot is only accessed atomically.
    unsafe { living(record) }.load(Ordering::Acquire) != 0
}
/// # Safety
/// A live record whose running thread may publish its state concurrently.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn swr_v1_core_thread_status(record: *const CoreThread) -> c_int {
    // SAFETY: per the live-record contract; the state slot is only accessed atomically.
    unsafe { status(record) }.load(Ordering::Acquire)
}
/// # Safety
/// The record outlives the running thread; only that thread publishes status.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn swr_v1_core_thread_enter(record: *const CoreThread) {
    // SAFETY: the record's atomic state remains live until the thread has ended.
    unsafe { status(record) }.store(0, Ordering::Release);
    // SAFETY: the same live state record.
    unsafe { living(record) }.store(1, Ordering::Release);
}
/// # Safety
/// As `swr_v1_core_thread_enter`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn swr_v1_core_thread_exit(record: *const CoreThread, code: c_int) {
    // SAFETY: the running thread publishes into its live atomic state.
    unsafe { status(record) }.store(code, Ordering::Release);
    // SAFETY: publish completion after the exit status.
    unsafe { living(record) }.store(0, Ordering::Release);
}
/// # Safety
/// Both records are live and owned exclusively by this controlling thread.
/// Native terminates on a false result, as `std::thread`'s move assignment does.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn swr_v1_core_thread_move(
    destination: *mut CoreThread,
    source: *mut CoreThread,
) -> bool {
    // SAFETY: copy scalar state before std::thread's original joinable check.
    let code = unsafe { swr_v1_core_thread_status(source) };
    // SAFETY: source is a live record.
    let alive = unsafe { swr_v1_core_thread_alive(source) };
    // SAFETY: destination is live; its state slots have atomic-only accesses.
    unsafe { status(destination) }.store(code, Ordering::Release);
    // SAFETY: as above.
    unsafe { living(destination) }.store(u8::from(alive), Ordering::Release);
    // SAFETY: the controlling thread alone reads/writes both handles.
    if unsafe { (*destination).handle != libc::pthread_t::default() } {
        return false;
    }
    // SAFETY: source's controlling thread transfers its unique handle.
    let handle = unsafe { (*source).handle };
    // SAFETY: destination has no owned thread and receives this unique handle.
    unsafe {
        (*destination).handle = handle;
    }
    // SAFETY: source is now moved from (also valid for a nonjoinable self-move).
    unsafe {
        (*source).handle = libc::pthread_t::default();
    }
    true
}
/// # Safety
/// A live record used exclusively by its controlling thread.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn swr_v1_core_thread_id(record: *const CoreThread) -> libc::pthread_t {
    // SAFETY: the controlling thread alone reads/writes the OS handle.
    unsafe { (*record).handle }
}
/// # Safety
/// `entry` is a native pthread entry, owning `argument` after success. It must
/// not require a Rust entry frame; a failed creation leaves ownership native.
/// The controlling thread exclusively owns `record` and handles termination if
/// a successful creation replaces a previously joinable handle.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn swr_v1_core_thread_start(
    record: *mut CoreThread,
    entry: extern "C" fn(*mut c_void) -> *mut c_void,
    argument: *mut c_void,
) -> c_int {
    let mut thread = std::mem::MaybeUninit::<libc::pthread_t>::uninit();
    // SAFETY: default pthread attributes; native entry owns argument only on success.
    let code = unsafe { libc::pthread_create(thread.as_mut_ptr(), ptr::null(), entry, argument) };
    if code == 0 {
        // SAFETY: successful pthread_create initialized the handle.
        let thread = unsafe { thread.assume_init() };
        // SAFETY: only the controlling thread publishes/reads the owned handle.
        unsafe {
            (*record).handle = thread;
        }
    }
    code
}
/// Validate the owned handle before native performs the cancellable join.
/// # Safety
/// The controlling thread exclusively owns this record.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn swr_v1_core_thread_join_check(record: *const CoreThread) -> c_int {
    // SAFETY: the controlling thread alone reads/writes its owned handle.
    if unsafe { (*record).handle == libc::pthread_t::default() } {
        libc::EINVAL
    } else {
        0
    }
}
/// Commit the native join's result; a cancelled join never reaches this leaf.
/// # Safety
/// `code` is the result of joining this record's current handle, or the failed
/// validation status. The controlling thread exclusively owns the record.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn swr_v1_core_thread_join_complete(
    record: *mut CoreThread,
    code: c_int,
) -> c_int {
    if code == 0 {
        // SAFETY: successful native join consumed exactly this owner handle.
        unsafe {
            (*record).handle = libc::pthread_t::default();
        }
    }
    code
}
/// # Safety
/// The controlling thread exclusively owns this record and detaches at most once.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn swr_v1_core_thread_detach(record: *mut CoreThread) -> c_int {
    // SAFETY: exclusive access to the owner handle.
    let handle = unsafe { (*record).handle };
    if handle == libc::pthread_t::default() {
        return libc::EINVAL;
    }
    // SAFETY: this is the owner's current joinable pthread handle.
    let code = unsafe { libc::pthread_detach(handle) };
    if code == 0 {
        // SAFETY: a successful detach consumed this handle; failures preserve it.
        unsafe {
            (*record).handle = libc::pthread_t::default();
        }
    }
    code
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::AtomicBool;
    // The test controls its own uncancelled pthread; production's cancellable
    // join is native and executes only after the Rust validation leaf returns.
    unsafe fn join(record: *mut CoreThread) -> c_int {
        let code = unsafe { swr_v1_core_thread_join_check(record) };
        let code = if code == 0 {
            unsafe { libc::pthread_join(swr_v1_core_thread_id(record), ptr::null_mut()) }
        } else {
            code
        };
        unsafe { swr_v1_core_thread_join_complete(record, code) }
    }
    struct Running {
        record: *mut CoreThread,
        ready: AtomicBool,
        release: AtomicBool,
    }
    extern "C" fn child(context: *mut c_void) -> *mut c_void {
        unsafe {
            let context = &*context.cast::<Running>();
            swr_v1_core_thread_enter(context.record);
            context.ready.store(true, Ordering::Release);
            while !context.release.load(Ordering::Acquire) {
                std::thread::yield_now();
            }
            swr_v1_core_thread_exit(context.record, 37);
        }
        ptr::null_mut()
    }
    #[test]
    fn real_pthread_handle_and_status_are_owned_until_join() {
        unsafe {
            let mut owner = Box::<CoreThread>::default();
            let mut context = Box::new(Running {
                record: &raw mut *owner,
                ready: AtomicBool::new(false),
                release: AtomicBool::new(false),
            });
            assert!(!swr_v1_core_thread_joinable(&raw const *owner));
            assert!(!swr_v1_core_thread_alive(&raw const *owner));
            assert_eq!(join(&raw mut *owner), libc::EINVAL);
            assert_eq!(
                swr_v1_core_thread_start(&raw mut *owner, child, (&raw mut *context).cast()),
                0
            );
            while !context.ready.load(Ordering::Acquire) {
                std::thread::yield_now();
            }
            assert!(swr_v1_core_thread_alive(&raw const *owner));
            assert!(swr_v1_core_thread_joinable(&raw const *owner));
            assert_eq!(swr_v1_core_thread_status(&raw const *owner), 0);
            assert_ne!(
                swr_v1_core_thread_id(&raw const *owner),
                libc::pthread_t::default()
            );
            context.release.store(true, Ordering::Release);
            assert_eq!(join(&raw mut *owner), 0);
            assert!(!swr_v1_core_thread_alive(&raw const *owner));
            assert!(!swr_v1_core_thread_joinable(&raw const *owner));
            assert_eq!(swr_v1_core_thread_status(&raw const *owner), 37);
            assert_eq!(join(&raw mut *owner), libc::EINVAL);
            assert_eq!(swr_v1_core_thread_detach(&raw mut *owner), libc::EINVAL);
        }
    }
}
