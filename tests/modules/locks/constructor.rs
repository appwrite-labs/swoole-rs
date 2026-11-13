//! The former `constructor.cc` and `constructor.py`, row for row: a lock of every kind
//! made with a fault injected at the pthread attribute, pshared or init call it reaches.
//! This binary defines those pthread symbols, so crates/locks' calls bind to the fakes
//! below, which pass through to libc unless this thread is watching. The lock ABI's
//! `swr_v1_locks_init` is replaced by the safe constructors that were its callers'
//! (README.md maps the checks).
use std::alloc::{GlobalAlloc, Layout, System};
use std::cell::Cell;
use std::ffi::{CStr, c_int, c_void};
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};

use libc::{EAGAIN, EBUSY, EINVAL, ENOMEM, ENOSPC, ENOTSUP};
use swoole_rs_corebase::sys::{errno, set_errno};
use swoole_rs_locks::{Lock, SharedMutex};

/// The fault and what the faked calls counted, while this thread watches.
#[derive(Clone, Copy)]
struct Watch {
    watching: bool,
    kind: i32,
    stage: i32,
    allocated: u32,
    released: u32,
    attr_destroyed: u32,
    init_calls: u32,
    attr_init_calls: u32,
    shared_calls: u32,
}

thread_local! {
    static WATCH: Cell<Watch> = const {
        Cell::new(Watch {
            watching: false,
            kind: 0,
            stage: 0,
            allocated: 0,
            released: 0,
            attr_destroyed: 0,
            init_calls: 0,
            attr_init_calls: 0,
            shared_calls: 0,
        })
    };
}

/// This thread's watch, counted by `count` when it is watching; `None` otherwise (and
/// while the thread's storage is gone).
fn watching(count: impl FnOnce(&mut Watch)) -> Option<Watch> {
    WATCH
        .try_with(|cell| {
            let mut watch = cell.get();
            watch.watching.then(|| {
                count(&mut watch);
                cell.set(watch);
                watch
            })
        })
        .ok()
        .flatten()
}

/// Counts what a watching thread allocates and frees, as the gate's `operator new` and
/// `delete` did.
struct Counting;

// SAFETY: every request goes to `System` unchanged.
unsafe impl GlobalAlloc for Counting {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        watching(|watch| watch.allocated += 1);
        // SAFETY: the caller's contract, passed on.
        unsafe { System.alloc(layout) }
    }
    unsafe fn dealloc(&self, pointer: *mut u8, layout: Layout) {
        watching(|watch| watch.released += 1);
        // SAFETY: the caller's contract, passed on.
        unsafe { System.dealloc(pointer, layout) }
    }
}

#[global_allocator]
static ALLOCATOR: Counting = Counting;

/// libc's own `name`, found once.
fn next(name: &CStr, found: &AtomicUsize) -> *mut c_void {
    let mut address = found.load(Ordering::Relaxed);
    if address == 0 {
        // SAFETY: a NUL-terminated name.
        address = unsafe { libc::dlsym(libc::RTLD_NEXT, name.as_ptr()) }.addr();
        assert_ne!(address, 0, "{name:?}");
        found.store(address, Ordering::Relaxed);
    }
    std::ptr::without_provenance_mut(address)
}

/// A fake of one pthread call: `$fault` returns the injected error for a watching
/// thread, or `None` to pass through to libc's own, whose result `$after` sees.
macro_rules! fake {
    ($name:ident($($argument:ident: $type:ty),*), $fault:expr, $after:expr) => {
        #[unsafe(no_mangle)]
        unsafe extern "C" fn $name($($argument: $type),*) -> c_int {
            static REAL: AtomicUsize = AtomicUsize::new(0);
            let (fault, after): (fn() -> Option<c_int>, fn(c_int) -> c_int) = ($fault, $after);
            if let Some(error) = fault() {
                return error;
            }
            let name = concat!(stringify!($name), "\0").as_bytes();
            let real = next(CStr::from_bytes_with_nul(name).unwrap_or_default(), &REAL);
            // SAFETY: libc's function of this name and signature, called as this one was.
            after(unsafe {
                std::mem::transmute::<*mut c_void, unsafe extern "C" fn($($type),*) -> c_int>(real)(
                    $($argument),*
                )
            })
        }
    };
}

/// The injected `error`, with errno set to something else for the caller to keep.
fn fail(error: c_int) -> c_int {
    set_errno(EBUSY);
    error
}

fn attr_init(kind: i32) -> Option<c_int> {
    let watch = watching(|watch| watch.attr_init_calls += 1)?;
    (watch.stage == 1 && watch.kind == kind).then(|| fail(EAGAIN))
}

fn set_shared(kind: i32) -> Option<c_int> {
    let watch = watching(|watch| watch.shared_calls += 1)?;
    match watch.stage {
        1 if watch.kind == kind => Some(fail(EINVAL)),
        2 if watch.kind == kind => Some(fail(ENOTSUP)),
        _ => None,
    }
}

fn init(kind: i32) -> Option<c_int> {
    let watch = watching(|watch| watch.init_calls += 1)?;
    match watch.stage {
        1 if watch.kind == kind => Some(fail(EINVAL)),
        0 if watch.kind == kind => Some(fail(ENOMEM)),
        _ => None,
    }
}

/// A failed attribute init leaves nothing to destroy; the fake says so if asked.
fn attr_destroy(kind: i32) -> Option<c_int> {
    let watch = watching(|_| ())?;
    (watch.stage == 1 && watch.kind == kind).then(|| attr_destroyed(EINVAL))
}

/// An attribute destroy, counted, leaving an errno its caller must not keep.
fn attr_destroyed(result: c_int) -> c_int {
    if watching(|watch| watch.attr_destroyed += 1).is_some() {
        set_errno(ENOSPC);
    }
    result
}

fn same(result: c_int) -> c_int {
    result
}

fake!(
    pthread_mutexattr_init(attr: *mut libc::pthread_mutexattr_t),
    || attr_init(3),
    same
);
fake!(
    pthread_rwlockattr_init(attr: *mut libc::pthread_rwlockattr_t),
    || attr_init(1),
    same
);
fake!(
    pthread_mutexattr_setpshared(attr: *mut libc::pthread_mutexattr_t, shared: c_int),
    || set_shared(3),
    same
);
fake!(
    pthread_rwlockattr_setpshared(attr: *mut libc::pthread_rwlockattr_t, shared: c_int),
    || set_shared(1),
    same
);
fake!(
    pthread_mutex_init(lock: *mut libc::pthread_mutex_t, attr: *const libc::pthread_mutexattr_t),
    || init(3),
    same
);
fake!(
    pthread_rwlock_init(lock: *mut libc::pthread_rwlock_t, attr: *const libc::pthread_rwlockattr_t),
    || init(1),
    same
);
#[cfg(target_os = "linux")]
fake!(
    pthread_spin_init(lock: *mut libc::pthread_spinlock_t, shared: c_int),
    || {
        let watch = watching(|watch| watch.init_calls += 1)?;
        (watch.stage == 0 && watch.kind == 5).then(|| fail(ENOMEM))
    },
    same
);
fake!(
    pthread_mutexattr_destroy(attr: *mut libc::pthread_mutexattr_t),
    || attr_destroy(3),
    attr_destroyed
);
fake!(
    pthread_rwlockattr_destroy(attr: *mut libc::pthread_rwlockattr_t),
    || attr_destroy(1),
    attr_destroyed
);

/// What one faulted init left: whether a lock was made, its result where the
/// constructor returns one, errno and the counts.
struct Row {
    made: bool,
    result: Option<c_int>,
    errno: c_int,
    watch: Watch,
}

/// Runs `make` with a fault at `stage` for `kind`: 0 the lock's init, 1 the attribute's
/// init (and every later call, which must not be reached), 2 its pshared setting.
fn faulted(kind: i32, stage: i32, make: impl FnOnce() -> (bool, Option<c_int>)) -> Row {
    WATCH.set(Watch {
        watching: true,
        kind,
        stage,
        allocated: 0,
        released: 0,
        attr_destroyed: 0,
        init_calls: 0,
        attr_init_calls: 0,
        shared_calls: 0,
    });
    let (made, result) = make();
    let errno = errno();
    let watch = WATCH.get();
    WATCH.set(Watch {
        watching: false,
        ..watch
    });
    Row {
        made,
        result,
        errno,
        watch,
    }
}

/// constructor.py's row check. `storage` is what the constructor allocates and frees
/// itself: the ABI's caller supplied it, so there it was none.
fn check(row: &Row, kind: i32, shared: bool, stage: i32, storage: u32) {
    let at = format!("kind {kind} shared {shared} stage {stage}");
    let expected = match stage {
        0 => ENOMEM,
        1 => EAGAIN,
        _ => ENOTSUP,
    };
    assert!(!row.made, "{at}");
    if let Some(result) = row.result {
        assert_eq!(result, expected, "{at}");
    }
    assert_eq!(row.errno, EBUSY, "{at}");
    assert_eq!(row.watch.allocated, storage, "{at}");
    assert_eq!(row.watch.released, storage, "{at}");
    assert_eq!(row.watch.init_calls, u32::from(stage == 0), "{at}");
    assert_eq!(row.watch.attr_init_calls, u32::from(kind != 5), "{at}");
    assert_eq!(
        row.watch.shared_calls,
        u32::from(kind != 5 && shared && stage != 1),
        "{at}"
    );
    assert_eq!(
        row.watch.attr_destroyed,
        u32::from(kind != 5 && stage != 1),
        "{at}"
    );
}

/// `direct(kind, shared, stage)`: `Lock::private`, which owns its storage, or
/// `Lock::shared` in storage made before the fault, as the gate's stack buffer was.
fn direct(kind: i32, shared: bool, stage: i32) {
    let row = if shared {
        let storage: &'static mut [AtomicU64] = Box::leak(
            (0..Lock::words(kind))
                .map(|_| AtomicU64::new(0))
                .collect::<Box<[_]>>(),
        );
        faulted(kind, stage, || {
            (Lock::shared(kind, storage).is_some(), None)
        })
    } else {
        faulted(kind, stage, || (Lock::private(kind).is_some(), None))
    };
    check(&row, kind, shared, stage, u32::from(!shared));
}

/// `main`'s rows: 10, and the spinlock's two on Linux.
#[test]
#[cfg_attr(miri, ignore = "Miri has no dlsym or pthread_mutexattr_setpshared")]
fn every_kind_reports_a_faulted_init_and_cleans_up() {
    for kind in [1, 3] {
        for shared in [false, true] {
            for stage in [0, 1] {
                direct(kind, shared, stage);
            }
        }
        direct(kind, true, 2);
    }
    #[cfg(target_os = "linux")]
    for shared in [false, true] {
        direct(5, shared, 0);
    }
}

/// The shared mutex rows again through `SharedMutex::init`, which returns the first
/// error as `swr_v1_locks_init` did.
#[test]
#[cfg_attr(miri, ignore = "Miri has no dlsym or pthread_mutexattr_setpshared")]
fn a_shared_mutex_returns_the_first_error() {
    for stage in [0, 1, 2] {
        let mut mutex = SharedMutex::new();
        let row = faulted(3, stage, || {
            let result = mutex.init();
            (result == 0, Some(result))
        });
        check(&row, 3, true, stage, 0);
    }
}
