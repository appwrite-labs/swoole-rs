//! The current worker's per-thread/process record and lifecycle, owned by Rust.
use crate::process::worker_record::WorkerRecord;
use core::ffi::c_int;
use core::sync::atomic::Ordering::Relaxed;
use core::sync::atomic::{AtomicBool, AtomicI32, AtomicI64, AtomicPtr, AtomicU8, AtomicU32};

/// `SwooleWG`: the current worker's identity and state. Each field is an atomic,
/// read and written one at a time (`Relaxed`, plain loads and stores), as signal
/// handlers and, without ZTS, the process's other threads read and write single
/// fields of the same record.
pub struct WorkerGlobalRecord {
    pub id: AtomicU32,
    pub kind: AtomicU8,
    pub pid: AtomicI32,
    pub shutdown: AtomicBool,
    pub running: AtomicBool,
    pub max_request: AtomicU32,
    worker: AtomicPtr<WorkerRecord>,
    /// Upstream's `worker_copy != nullptr`: the worker was separated from its pool. The
    /// copy itself is never read, so only the fact is kept.
    detached: AtomicBool,
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
        detached: AtomicBool::new(false),
        exit_time: AtomicI64::new(0),
    }
}

// `SwooleWG` is `SW_THREAD_LOCAL`: thread-local on a thread-safe PHP, one process
// record otherwise, which saves a TLS lookup per access on the request path.
// Tests (which unwind) keep it per thread, as they run in parallel. Only
// [`set_current_worker`] and [`worker_global_stopped`] write `worker`, so
// [`with_current`] can trust it.
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

/// `SwooleWG.worker`: this thread's current worker, or null.
#[must_use]
pub fn current_worker() -> *mut WorkerRecord {
    with_globals(|globals| globals.worker.load(Relaxed))
}

/// `f` over `SwooleWG.worker`, or `None` while this thread runs no worker. The
/// handle lives only for the call: `f` can't keep it.
#[inline(always)]
pub fn with_current<R>(
    f: impl FnOnce(&crate::process::worker_record::Worker<'_>) -> R,
) -> Option<R> {
    let raw = current_worker();
    if raw.is_null() {
        return None;
    }
    // SAFETY: not null, so the slot holds a loan of the record the worker loop this thread runs
    // was given by the pool or server that owns it, which keeps it until it frees
    // its workers after the loop ends. `worker_global_stopped` frees only
    // the detached flag, so stopping during `f` leaves the
    // lent record live. The loop orders the field writes made through the handle.
    let worker = unsafe { crate::process::worker_record::Worker::from(&*raw) };
    Some(f(&worker))
}

/// `SwooleWG.worker = worker`.
///
/// # Safety
/// `worker` is null, or the live record of the worker loop this thread runs, which
/// its pool or server keeps until the loop ends and the slot is cleared or replaced:
/// [`with_current`] lends it to safe code until then.
pub unsafe fn set_current_worker(worker: *mut WorkerRecord) {
    with_globals(|globals| globals.worker.store(worker, Relaxed));
}

/// Apply the randomized worker grace. A failed draw subtracts one, as in the native request limit.
pub fn worker_global_max_requests(
    max_request: u32,
    grace: u32,
    random: impl FnOnce(c_int, c_int) -> std::io::Result<c_int>,
) {
    let mut limit = max_request;
    if max_request > 0 && grace > 0 {
        let extra = random(1, c_int::from_ne_bytes(grace.to_ne_bytes()));
        limit = extra.map_or_else(|_| limit.wrapping_sub(1), |n| limit.wrapping_add_signed(n));
    }
    with_globals(|globals| globals.max_request.store(limit, Relaxed));
}

/// Separates this thread's current worker from its pool, as upstream's copy into
/// `worker_copy` does; nothing reads that copy, so only the separation is kept.
/// Upstream also sets `SwooleWG.worker = worker`, which the slot already holds: every
/// caller stops the worker whose loop set it current.
pub fn worker_global_detach(_worker: crate::process::worker_record::Worker<'_>) {
    with_globals(|globals| globals.detached.store(true, Relaxed));
}

/// Stops the current worker; a detached one is no longer current.
pub fn worker_global_stopped() {
    with_globals(|globals| {
        globals.running.store(false, Relaxed);
        if globals.detached.swap(false, Relaxed) {
            // The shared worker is a loan, no longer current once detached.
            globals.worker.store(core::ptr::null_mut(), Relaxed);
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::process::methods::{
        swoole_get_worker_id, swoole_get_worker_pid, swoole_get_worker_type, swoole_set_worker_id,
        swoole_set_worker_type, worker_is_running, worker_is_shutdown,
    };
    use crate::process::worker_record::{Worker, swr_v1_worker_init, swr_v1_worker_shutdown};
    use std::sync::atomic::AtomicUsize;

    fn max_request() -> u32 {
        with_globals(|globals| globals.max_request.load(Relaxed))
    }

    static RANDOM_CALLS: AtomicUsize = AtomicUsize::new(0);
    const MAXIMUM: fn(c_int, c_int) -> std::io::Result<c_int> = |_, max| {
        RANDOM_CALLS.fetch_add(1, Relaxed);
        Ok(max)
    };

    #[test]
    fn worker_identity_is_thread_local() {
        swoole_set_worker_id(7);
        swoole_set_worker_type(-1);
        assert_eq!(swoole_get_worker_type(), 255);
        with_globals(|globals| globals.pid.store(123, Relaxed));
        assert_eq!(swoole_get_worker_pid(), 123);
        std::thread::spawn(|| {
            assert_eq!(swoole_get_worker_id(), 0);
            swoole_set_worker_id(22);
            assert_eq!(swoole_get_worker_id(), 22);
        })
        .join()
        .unwrap();
        assert_eq!(swoole_get_worker_id(), 7);
    }

    #[test]
    fn lifecycle_limit_and_detach_keep_the_record_and_clear_the_slot() {
        let mut worker: WorkerRecord = unsafe { core::mem::zeroed() };
        unsafe {
            Worker::from(&worker).set_id(11);
            Worker::from(&worker).set_pid(1234);
            swr_v1_worker_init(Worker::from(&worker));
            assert!(Worker::from(&worker).is_idle());
            assert!(worker_is_running());
            assert!(!worker_is_shutdown());
            worker_global_max_requests(2, 0, MAXIMUM);
            Worker::from(&worker).add_request_count();
            assert!(!Worker::from(&worker).exceeded_max_request());
            Worker::from(&worker).add_request_count();
            assert!(Worker::from(&worker).exceeded_max_request());
            set_current_worker(&raw mut worker);
            worker_global_detach(Worker::from(&worker));
            assert!(with_globals(|globals| globals.detached.load(Relaxed)));
            assert_eq!(Worker::from(&worker).id(), 11);
            assert_eq!(Worker::from(&worker).pid(), 1234);
            swr_v1_worker_shutdown(Worker::from(&worker));
            assert_eq!(
                Worker::from(&worker).status(),
                Some(crate::process::worker_record::WorkerStatus::Exit)
            );
            assert!(worker_is_shutdown());
            worker_global_stopped();
            assert!(!worker_is_running());
            assert!(!with_globals(|globals| globals.detached.load(Relaxed)));
            assert!(current_worker().is_null());
        }
    }

    #[test]
    fn a_worker_stopped_during_with_current_stays_readable() {
        let mut worker: WorkerRecord = unsafe { std::mem::zeroed() };
        Worker::from(&worker).set_id(7);
        unsafe { set_current_worker(&raw mut worker) };
        worker_global_detach(Worker::from(&worker));
        let id = with_current(|current| {
            // Clears the slot, not the lent record.
            worker_global_stopped();
            current.id()
        });
        assert_eq!(id, Some(7));
        assert!(with_current(|_| ()).is_none());
    }

    #[test]
    fn request_grace_keeps_native_unsigned_wrap_and_zero_short_circuit() {
        RANDOM_CALLS.store(0, Relaxed);
        worker_global_max_requests(0, 10, MAXIMUM);
        assert_eq!(max_request(), 0);
        worker_global_max_requests(10, 0, MAXIMUM);
        assert_eq!(max_request(), 10);
        assert_eq!(RANDOM_CALLS.load(Relaxed), 0);
        worker_global_max_requests(u32::MAX - 2, 10, MAXIMUM);
        assert_eq!(max_request(), 7);
        assert_eq!(RANDOM_CALLS.load(Relaxed), 1);
        worker_global_max_requests(10, 3, |_, _| Err(std::io::ErrorKind::UnexpectedEof.into()));
        assert_eq!(max_request(), 9);
    }
}
