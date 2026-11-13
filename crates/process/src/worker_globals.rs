//! The current worker's per-thread/process record and lifecycle, owned by Rust.
use crate::worker_record::WorkerRecord;
use core::ffi::c_int;
use core::sync::atomic::Ordering::Relaxed;
use core::sync::atomic::{AtomicBool, AtomicI32, AtomicI64, AtomicPtr, AtomicU8, AtomicU32};

/// The compatibility view of the current worker; pointer fields are native loans,
/// except `worker_copy`, whose local record allocation is owned here. Each field is
/// an atomic of its C type's layout, read and written one at a time (`Relaxed`,
/// plain loads and stores), as native code, signal handlers and, without ZTS, the
/// process's other threads read and write single fields of the same record.
#[repr(C)]
pub struct WorkerGlobalRecord {
    pub id: AtomicU32,
    pub kind: AtomicU8,
    pub pid: AtomicI32,
    pub shutdown: AtomicBool,
    pub running: AtomicBool,
    pub max_request: AtomicU32,
    pub worker: AtomicPtr<WorkerRecord>,
    worker_copy: AtomicPtr<WorkerRecord>,
    pub exit_time: AtomicI64,
}

const _: () = assert!(
    size_of::<libc::time_t>() == size_of::<i64>(),
    "exit_time is a time_t"
);
const _: () = assert!(size_of::<c_int>() == size_of::<i32>(), "pid is an int");

const fn init() -> WorkerGlobalRecord {
    WorkerGlobalRecord {
        id: AtomicU32::new(0),
        kind: AtomicU8::new(0),
        pid: AtomicI32::new(0),
        shutdown: AtomicBool::new(false),
        running: AtomicBool::new(false),
        max_request: AtomicU32::new(0),
        worker: AtomicPtr::new(core::ptr::null_mut()),
        worker_copy: AtomicPtr::new(core::ptr::null_mut()),
        exit_time: AtomicI64::new(0),
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
static GLOBAL: WorkerGlobalRecord = init();

/// `f` over this thread's record (`SwooleWG`). It has no TLS destructor and stays
/// available during native thread teardown, as the original POD did.
pub fn with_globals<R>(f: impl FnOnce(&WorkerGlobalRecord) -> R) -> R {
    #[cfg(any(zts, panic = "unwind"))]
    return GLOBAL.with(f);
    #[cfg(not(any(zts, panic = "unwind")))]
    f(&GLOBAL)
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
    with_globals(|globals| globals.worker.load(Relaxed))
}

/// `f` over `SwooleWG.worker`, or `None` while this thread runs no worker. The
/// handle lives only for the call: `f` can't keep it.
pub fn with_current<R>(f: impl FnOnce(&crate::worker_record::Worker<'_>) -> R) -> Option<R> {
    // SAFETY: the slot holds a loan of the record the worker loop this thread runs
    // was given by the pool or server that owns it, which keeps it until it frees
    // its workers after the loop ends. `worker_global_stopped` frees only
    // `worker_copy`, a separate allocation, so stopping during `f` leaves the
    // lent record live. The loop orders the field writes made through the handle.
    let worker = unsafe { crate::worker_record::Worker::from_raw(current_worker()) };
    worker.as_ref().map(f)
}

/// `SwooleWG.worker = worker`.
pub fn set_current_worker(worker: *mut WorkerRecord) {
    with_globals(|globals| globals.worker.store(worker, Relaxed));
}

/// Fields: 0 id, 1 pid, 2 type, 3 running, 4 shutdown, 5 max requests.
#[must_use]
pub extern "C" fn swr_v1_worker_global_get(field: u32) -> i64 {
    with_globals(|globals| match field {
        0 => i64::from(globals.id.load(Relaxed)),
        1 => i64::from(globals.pid.load(Relaxed)),
        2 => i64::from(globals.kind.load(Relaxed)),
        3 => i64::from(globals.running.load(Relaxed)),
        4 => i64::from(globals.shutdown.load(Relaxed)),
        5 => i64::from(globals.max_request.load(Relaxed)),
        _ => 0,
    })
}

/// Updates the current worker's identity, preserving native integer narrowing.
pub extern "C" fn swr_v1_worker_global_set(field: u32, value: i64) {
    let low = u32::try_from(value.cast_unsigned() & u64::from(u32::MAX)).unwrap_or_default();
    with_globals(|globals| match field {
        0 => globals.id.store(low, Relaxed),
        1 => globals
            .pid
            .store(c_int::from_ne_bytes(low.to_ne_bytes()), Relaxed),
        2 => globals.kind.store(
            u8::try_from(low & u32::from(u8::MAX)).unwrap_or_default(),
            Relaxed,
        ),
        3 => globals.running.store(value != 0, Relaxed),
        4 => globals.shutdown.store(value != 0, Relaxed),
        5 => globals.max_request.store(low, Relaxed),
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
pub fn worker_global_detach(worker: crate::worker_record::Worker<'_>) {
    let copy = Box::into_raw(Box::new(worker.get()));
    with_globals(|globals| globals.worker_copy.store(copy, Relaxed));
    set_current_worker(worker.raw());
}

/// Stops the current worker and releases its local detached copy.
pub fn worker_global_stopped() {
    let copy = with_globals(|globals| {
        globals.running.store(false, Relaxed);
        // The owning slot is cleared before its allocation is released.
        let copy = globals.worker_copy.swap(core::ptr::null_mut(), Relaxed);
        if !copy.is_null() {
            // The shared worker is a loan, no longer current once this copy is released.
            globals.worker.store(core::ptr::null_mut(), Relaxed);
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
    use std::sync::atomic::AtomicUsize;

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
        unsafe { (*native_view).pid.store(123, Relaxed) };
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
            assert!(Worker::new(&raw const worker).is_idle());
            assert_eq!(swr_v1_worker_global_get(3), 1);
            assert_eq!(swr_v1_worker_global_get(4), 0);
            worker_global_max_requests(2, 0, maximum);
            Worker::new(&raw mut worker).add_request_count();
            assert!(!Worker::new(&raw const worker).exceeded_max_request());
            Worker::new(&raw mut worker).add_request_count();
            assert!(Worker::new(&raw const worker).exceeded_max_request());
            worker_global_detach(Worker::new(&raw mut worker));
            let copy = (*thread_globals()).worker_copy.load(Relaxed);
            assert_ne!(copy, &raw mut worker);
            assert_eq!(Worker::new(copy).id(), 11);
            assert_eq!(Worker::new(copy).pid(), 1234);
            swr_v1_worker_shutdown(Worker::new(&raw mut worker));
            assert_eq!(
                Worker::new(&raw const worker).status(),
                Some(crate::worker_record::WorkerStatus::Exit)
            );
            assert_eq!(swr_v1_worker_global_get(4), 1);
            worker_global_stopped();
            assert_eq!(swr_v1_worker_global_get(3), 0);
            assert!((*thread_globals()).worker_copy.load(Relaxed).is_null());
            assert!((*thread_globals()).worker.load(Relaxed).is_null());
        }
    }

    #[test]
    fn a_worker_stopped_during_with_current_stays_readable() {
        let mut worker: WorkerRecord = unsafe { std::mem::zeroed() };
        unsafe { Worker::new(&raw mut worker).set_id(7) };
        worker_global_detach(unsafe { Worker::new(&raw mut worker) });
        let id = with_current(|current| {
            // Frees the detached copy and clears the slot, not the lent record.
            worker_global_stopped();
            current.id()
        });
        assert_eq!(id, Some(7));
        assert!(with_current(|_| ()).is_none());
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
