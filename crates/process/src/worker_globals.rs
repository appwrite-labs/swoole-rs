//! The current worker's per-thread/process record and lifecycle, owned by Rust.
use crate::worker_record::WorkerRecord;
use core::cell::UnsafeCell;
use core::ffi::c_int;

/// The compatibility view of the current worker; pointer fields are native loans,
/// except `worker_copy`, whose local record allocation is owned here.
#[repr(C)]
pub struct WorkerGlobalRecord {
    pub id: u32,
    pub kind: u8,
    pub pid: c_int,
    pub shutdown: bool,
    pub running: bool,
    pub max_request: u32,
    pub worker: *mut WorkerRecord,
    pub worker_copy: *mut WorkerRecord,
    pub exit_time: libc::time_t,
}

const INIT: WorkerGlobalRecord = WorkerGlobalRecord {
    id: 0,
    kind: 0,
    pid: 0,
    shutdown: false,
    running: false,
    max_request: 0,
    worker: core::ptr::null_mut(),
    worker_copy: core::ptr::null_mut(),
    exit_time: 0,
};

// `SwooleWG` is `SW_THREAD_LOCAL`: thread-local on a thread-safe PHP, one process
// record otherwise, which saves a TLS lookup per access on the request path.
// Tests (which unwind) keep it per thread, as they run in parallel.
#[cfg(any(zts, panic = "unwind"))]
std::thread_local! {
    static GLOBAL: UnsafeCell<WorkerGlobalRecord> = const { UnsafeCell::new(INIT) };
}

#[cfg(not(any(zts, panic = "unwind")))]
struct Process(UnsafeCell<WorkerGlobalRecord>);

// SAFETY: as upstream's non-thread-safe `SwooleWG`, one record the process's
// threads share; callers access single fields, never a whole-record borrow.
#[cfg(not(any(zts, panic = "unwind")))]
unsafe impl Sync for Process {}

#[cfg(not(any(zts, panic = "unwind")))]
static GLOBAL: Process = Process(UnsafeCell::new(INIT));

/// Borrows the original native field view for this thread. It has no TLS destructor
/// and remains available during native thread teardown, as the original POD did.
/// The process-pool case fixture (tests/modules/hooks/system) reads it as `SwooleWG`.
#[cfg_attr(
    feature = "upstream-symbols",
    unsafe(export_name = "swr_v1_worker_globals")
)]
pub extern "C" fn thread_globals() -> *mut WorkerGlobalRecord {
    #[cfg(any(zts, panic = "unwind"))]
    return GLOBAL.with(UnsafeCell::get);
    #[cfg(not(any(zts, panic = "unwind")))]
    GLOBAL.0.get()
}

/// `SwooleWG.worker`: this thread's current worker, or null.
#[must_use]
pub fn current_worker() -> *mut WorkerRecord {
    // SAFETY: one field of this thread's live record, without a whole-record borrow.
    unsafe { (*thread_globals()).worker }
}

/// `SwooleWG.worker = worker`.
pub fn set_current_worker(worker: *mut WorkerRecord) {
    // SAFETY: as [`current_worker`], a write.
    unsafe { (*thread_globals()).worker = worker };
}

/// Fields: 0 id, 1 pid, 2 type, 3 running, 4 shutdown, 5 max requests.
#[must_use]
pub extern "C" fn swr_v1_worker_global_get(field: u32) -> i64 {
    let global = thread_globals();
    macro_rules! read {
        ($field:ident) => {{
            // SAFETY: the live TLS record is loaned to this thread; no whole-record borrow crosses native signals.
            unsafe { (*global).$field }
        }};
    }
    match field {
        0 => i64::from(read!(id)),
        1 => i64::from(read!(pid)),
        2 => i64::from(read!(kind)),
        3 => i64::from(read!(running)),
        4 => i64::from(read!(shutdown)),
        5 => i64::from(read!(max_request)),
        _ => 0,
    }
}

/// Updates the current worker's identity, preserving native integer narrowing.
pub extern "C" fn swr_v1_worker_global_set(field: u32, value: i64) {
    let global = thread_globals();
    macro_rules! write {
        ($field:ident,$value:expr) => {{
            // SAFETY: this leaf updates one field of this thread's live TLS view, without a whole-record borrow.
            unsafe { (*global).$field = $value };
        }};
    }
    let low = u32::try_from(value.cast_unsigned() & u64::from(u32::MAX)).unwrap_or_default();
    match field {
        0 => write!(id, low),
        1 => write!(pid, c_int::from_ne_bytes(low.to_ne_bytes())),
        2 => write!(
            kind,
            u8::try_from(low & u32::from(u8::MAX)).unwrap_or_default()
        ),
        3 => write!(running, value != 0),
        4 => write!(shutdown, value != 0),
        5 => write!(max_request, low),
        _ => {}
    }
}

/// Grace randomization calls the existing native wrapper so its assertion and
/// warning/errno behavior stay identical; all request-limit decisions and storage are Rust's.
/// `random` is `swoole_system_random(int,int)` or a stand-in.
pub fn worker_global_max_requests(
    max_request: u32,
    grace: u32,
    random: extern "C" fn(c_int, c_int) -> c_int,
) {
    let mut limit = max_request;
    if max_request > 0 && grace > 0 {
        let extra = random(1, c_int::from_ne_bytes(grace.to_ne_bytes()));
        limit = limit.wrapping_add(extra.cast_unsigned());
    }
    swr_v1_worker_global_set(5, i64::from(limit));
}

/// Retains the exiting worker's local record without relying on C++ copying the
/// reserved private field bytes. Shared/native resource pointers remain loans.
///
/// # Safety
/// `worker` is a live initialized worker; no previous detached copy is outstanding.
pub unsafe fn worker_global_detach(worker: *mut WorkerRecord) {
    // SAFETY: the native transition lends the initialized worker for this value copy.
    let copy = unsafe { worker.read() };
    let copy = Box::into_raw(Box::new(copy));
    let global = thread_globals();
    // SAFETY: this thread exclusively owns its current worker/copy fields.
    unsafe { (*global).worker_copy = copy };
    set_current_worker(worker);
}

/// Stops the current worker and releases its local detached copy.
pub fn worker_global_stopped() {
    let global = thread_globals();
    // SAFETY: the current thread owns the leaf running-state update.
    unsafe { (*global).running = false };
    // SAFETY: the TLS record's copy slot is live and this thread owns its updates.
    let copy = unsafe { (*global).worker_copy };
    if !copy.is_null() {
        // SAFETY: clear the owning slot before releasing its local allocation.
        unsafe { (*global).worker_copy = core::ptr::null_mut() };
        // SAFETY: the shared worker is a loan, no longer current once this copy is released.
        unsafe { (*global).worker = core::ptr::null_mut() };
        // SAFETY: detach transferred this unique local record box into the TLS slot.
        drop(unsafe { Box::from_raw(copy) });
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::worker_record::{Worker, swr_v1_worker_init, swr_v1_worker_shutdown};
    use std::sync::atomic::{AtomicUsize, Ordering::Relaxed};

    static RANDOM_CALLS: AtomicUsize = AtomicUsize::new(0);
    extern "C" fn maximum(_: c_int, max: c_int) -> c_int {
        RANDOM_CALLS.fetch_add(1, Relaxed);
        max
    }

    #[test]
    fn native_views_and_worker_identity_are_thread_local() {
        swr_v1_worker_global_set(0, 7);
        swr_v1_worker_global_set(2, -1);
        assert_eq!(swr_v1_worker_global_get(2), 255);
        let native_view = thread_globals();
        unsafe { (*native_view).pid = 123 };
        assert_eq!(swr_v1_worker_global_get(1), 123);
        std::thread::spawn(|| {
            assert_eq!(swr_v1_worker_global_get(0), 0);
            swr_v1_worker_global_set(0, 22);
            assert_eq!(swr_v1_worker_global_get(0), 22);
        })
        .join()
        .unwrap();
        assert_eq!(swr_v1_worker_global_get(0), 7);
        assert_eq!(core::mem::size_of::<WorkerGlobalRecord>(), 48);
        assert_eq!(core::mem::offset_of!(WorkerGlobalRecord, max_request), 16);
        assert_eq!(core::mem::offset_of!(WorkerGlobalRecord, worker), 24);
        assert_eq!(core::mem::offset_of!(WorkerGlobalRecord, exit_time), 40);
    }

    #[test]
    #[cfg_attr(miri, ignore = "Miri has no libc time")]
    fn lifecycle_limit_and_detached_copy_preserve_private_record_fields() {
        let mut worker: WorkerRecord = unsafe { core::mem::zeroed() };
        unsafe {
            Worker::new(&raw mut worker).set_id(11);
            Worker::new(&raw mut worker).set_pid(1234);
            swr_v1_worker_init(Worker::new(&raw mut worker));
            assert_eq!(Worker::new(&raw const worker).status(), 2);
            assert_eq!(swr_v1_worker_global_get(3), 1);
            assert_eq!(swr_v1_worker_global_get(4), 0);
            worker_global_max_requests(2, 0, maximum);
            Worker::new(&raw mut worker).add_request_count();
            assert!(!Worker::new(&raw const worker).exceeded_max_request());
            Worker::new(&raw mut worker).add_request_count();
            assert!(Worker::new(&raw const worker).exceeded_max_request());
            worker_global_detach(&raw mut worker);
            let copy = (*thread_globals()).worker_copy;
            assert_ne!(copy, &raw mut worker);
            assert_eq!(Worker::new(copy).id(), 11);
            assert_eq!(Worker::new(copy).pid(), 1234);
            swr_v1_worker_shutdown(Worker::new(&raw mut worker));
            assert_eq!(Worker::new(&raw const worker).status(), 3);
            assert_eq!(swr_v1_worker_global_get(4), 1);
            worker_global_stopped();
            assert_eq!(swr_v1_worker_global_get(3), 0);
            assert!((*thread_globals()).worker_copy.is_null());
            assert!((*thread_globals()).worker.is_null());
        }
    }

    #[test]
    fn request_grace_keeps_native_unsigned_wrap_and_zero_short_circuit() {
        RANDOM_CALLS.store(0, Relaxed);
        worker_global_max_requests(0, 10, maximum);
        assert_eq!(swr_v1_worker_global_get(5), 0);
        worker_global_max_requests(10, 0, maximum);
        assert_eq!(swr_v1_worker_global_get(5), 10);
        assert_eq!(RANDOM_CALLS.load(Relaxed), 0);
        worker_global_max_requests(u32::MAX - 2, 10, maximum);
        assert_eq!(swr_v1_worker_global_get(5), 7);
        assert_eq!(RANDOM_CALLS.load(Relaxed), 1);
    }
}
