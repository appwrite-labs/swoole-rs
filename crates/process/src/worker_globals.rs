//! The current worker's per-thread/process record and lifecycle, owned by Rust.
use crate::worker_record::WorkerRecord;
use core::cell::Cell;
use core::ffi::c_int;

/// The compatibility view of the current worker; pointer fields are native loans,
/// except `worker_copy`, whose local record allocation is owned here. Each field is
/// a `Cell`, read and written one at a time, as native code and signal handlers
/// write single fields of the same record.
#[repr(C)]
pub struct WorkerGlobalRecord {
    pub id: Cell<u32>,
    pub kind: Cell<u8>,
    pub pid: Cell<c_int>,
    pub shutdown: Cell<bool>,
    pub running: Cell<bool>,
    pub max_request: Cell<u32>,
    pub worker: Cell<*mut WorkerRecord>,
    worker_copy: Cell<*mut WorkerRecord>,
    pub exit_time: Cell<libc::time_t>,
}

const fn init() -> WorkerGlobalRecord {
    WorkerGlobalRecord {
        id: Cell::new(0),
        kind: Cell::new(0),
        pid: Cell::new(0),
        shutdown: Cell::new(false),
        running: Cell::new(false),
        max_request: Cell::new(0),
        worker: Cell::new(core::ptr::null_mut()),
        worker_copy: Cell::new(core::ptr::null_mut()),
        exit_time: Cell::new(0),
    }
}

// `SwooleWG` is `SW_THREAD_LOCAL`: thread-local on a thread-safe PHP, one process
// record otherwise, which saves a TLS lookup per access on the request path.
// Tests (which unwind) keep it per thread, as they run in parallel.
#[cfg(any(zts, panic = "unwind"))]
std::thread_local! {
    static GLOBAL: WorkerGlobalRecord = const { init() };
}

#[cfg(not(any(zts, panic = "unwind")))]
struct Process(WorkerGlobalRecord);

// SAFETY: as upstream's non-thread-safe `SwooleWG`, one record the process's
// threads share; callers access single fields, never a whole-record borrow.
#[cfg(not(any(zts, panic = "unwind")))]
unsafe impl Sync for Process {}

#[cfg(not(any(zts, panic = "unwind")))]
static GLOBAL: Process = Process(init());

/// `f` over this thread's record (`SwooleWG`). It has no TLS destructor and stays
/// available during native thread teardown, as the original POD did.
pub fn with_globals<R>(f: impl FnOnce(&WorkerGlobalRecord) -> R) -> R {
    #[cfg(any(zts, panic = "unwind"))]
    return GLOBAL.with(f);
    #[cfg(not(any(zts, panic = "unwind")))]
    f(&GLOBAL.0)
}

/// The original native field view for this thread.
/// The process-pool case fixture (tests/modules/hooks/system) reads it as `SwooleWG`.
#[cfg_attr(
    feature = "upstream-symbols",
    unsafe(export_name = "swr_v1_worker_globals")
)]
#[must_use]
pub extern "C" fn thread_globals() -> *mut WorkerGlobalRecord {
    with_globals(|globals| core::ptr::from_ref(globals).cast_mut())
}

/// `SwooleWG.worker`: this thread's current worker, or null.
#[must_use]
pub fn current_worker() -> *mut WorkerRecord {
    with_globals(|globals| globals.worker.get())
}

/// `SwooleWG.worker = worker`.
pub fn set_current_worker(worker: *mut WorkerRecord) {
    with_globals(|globals| globals.worker.set(worker));
}

/// Fields: 0 id, 1 pid, 2 type, 3 running, 4 shutdown, 5 max requests.
#[must_use]
pub extern "C" fn swr_v1_worker_global_get(field: u32) -> i64 {
    with_globals(|globals| match field {
        0 => i64::from(globals.id.get()),
        1 => i64::from(globals.pid.get()),
        2 => i64::from(globals.kind.get()),
        3 => i64::from(globals.running.get()),
        4 => i64::from(globals.shutdown.get()),
        5 => i64::from(globals.max_request.get()),
        _ => 0,
    })
}

/// Updates the current worker's identity, preserving native integer narrowing.
pub extern "C" fn swr_v1_worker_global_set(field: u32, value: i64) {
    let low = u32::try_from(value.cast_unsigned() & u64::from(u32::MAX)).unwrap_or_default();
    with_globals(|globals| match field {
        0 => globals.id.set(low),
        1 => globals.pid.set(c_int::from_ne_bytes(low.to_ne_bytes())),
        2 => globals
            .kind
            .set(u8::try_from(low & u32::from(u8::MAX)).unwrap_or_default()),
        3 => globals.running.set(value != 0),
        4 => globals.shutdown.set(value != 0),
        5 => globals.max_request.set(low),
        _ => {}
    });
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
    with_globals(|globals| globals.worker_copy.set(copy));
    set_current_worker(worker);
}

/// Stops the current worker and releases its local detached copy.
pub fn worker_global_stopped() {
    let copy = with_globals(|globals| {
        globals.running.set(false);
        // The owning slot is cleared before its allocation is released.
        let copy = globals.worker_copy.replace(core::ptr::null_mut());
        if !copy.is_null() {
            // The shared worker is a loan, no longer current once this copy is released.
            globals.worker.set(core::ptr::null_mut());
        }
        copy
    });
    if !copy.is_null() {
        // SAFETY: detach transferred this unique local record box into the slot just
        // cleared.
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
        unsafe { (*native_view).pid.set(123) };
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
            let copy = (*thread_globals()).worker_copy.get();
            assert_ne!(copy, &raw mut worker);
            assert_eq!(Worker::new(copy).id(), 11);
            assert_eq!(Worker::new(copy).pid(), 1234);
            swr_v1_worker_shutdown(Worker::new(&raw mut worker));
            assert_eq!(Worker::new(&raw const worker).status(), 3);
            assert_eq!(swr_v1_worker_global_get(4), 1);
            worker_global_stopped();
            assert_eq!(swr_v1_worker_global_get(3), 0);
            assert!((*thread_globals()).worker_copy.get().is_null());
            assert!((*thread_globals()).worker.get().is_null());
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
