//! `src/os/wait.cc`: `System::waitpid` and its SIGCHLD reaper. Exited children
//! nobody waits for yet, coroutines waiting for any child (first come, first served)
//! and coroutines waiting for one PID live here. Native code passes the coroutine
//! operations in `Coroutines`, keeps the SIGCHLD and reactor registration and sets
//! errno and the last error on a timeout.
//!
//! The registry is process-wide, as upstream's statics are: under thread contexts
//! a coroutine waits on its own thread and the loop's thread reaps. Signals reach
//! the reaper through `swoole_signal_dispatch`, never in a signal frame.
use std::collections::{BTreeMap, VecDeque};
use std::ffi::{c_int, c_void};
use std::sync::atomic::{AtomicI32, AtomicPtr, Ordering::Relaxed};
use std::sync::{Arc, Mutex, PoisonError};

/// A coroutine waiting for a child (upstream's `WaitTask`), shared by the waiting
/// coroutine and the registry while it is registered. The reaper hands it a child
/// (`pid` > 0, its `status`) and resumes `co`.
struct Task {
    co: AtomicPtr<c_void>,
    pid: AtomicI32,
    status: AtomicI32,
}

impl Task {
    fn pid(&self) -> libc::pid_t {
        self.pid.load(Relaxed)
    }
}

struct Waits {
    any: VecDeque<Arc<Task>>,
    by_pid: BTreeMap<libc::pid_t, Arc<Task>>,
    exited: BTreeMap<libc::pid_t, c_int>,
}

static WAITS: Mutex<Waits> = Mutex::new(Waits {
    any: VecDeque::new(),
    by_pid: BTreeMap::new(),
    exited: BTreeMap::new(),
});

fn waits<T>(f: impl FnOnce(&mut Waits) -> T) -> T {
    f(&mut WAITS.lock().unwrap_or_else(PoisonError::into_inner))
}

/// The native coroutine operations a wait needs. Made only by [`Coroutines::new`],
/// whose caller vouches that they suspend and resume as a wait expects, so
/// [`wait_pid`] and [`wait_reap`] ask nothing more of theirs.
pub struct Coroutines {
    /// The current coroutine if this thread has a reactor, else null.
    current: fn() -> *mut c_void,
    /// Registers the SIGCHLD reaper and the reactor callbacks, once.
    signal_init: fn(),
    /// `co->yield_ex(timeout)` of the current coroutine, if any.
    yield_for: fn(f64),
    /// `co->is_timedout()` of the current coroutine, if any.
    timed_out: fn() -> bool,
    /// `co->resume()`.
    resume: unsafe extern "C-unwind" fn(*mut c_void),
}

impl Coroutines {
    /// The operations, in their fields' order.
    ///
    /// # Safety
    /// `yield_for` suspends the coroutine `current` returns (when not null) until
    /// `resume` of it or the timeout, and `resume` resumes a coroutine suspended so.
    #[must_use]
    pub const unsafe fn new(
        current: fn() -> *mut c_void,
        signal_init: fn(),
        yield_for: fn(f64),
        timed_out: fn() -> bool,
        resume: unsafe extern "C-unwind" fn(*mut c_void),
    ) -> Self {
        Self {
            current,
            signal_init,
            yield_for,
            timed_out,
            resume,
        }
    }
}

/// What `System::waitpid` returns.
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Waited {
    pub pid: libc::pid_t,
    /// The wait gave up at its timeout (native sets errno and the last error).
    pub timed_out: bool,
}

fn waitpid(pid: libc::pid_t, status: &mut c_int, options: c_int) -> libc::pid_t {
    swoole_rs_corebase::sys::waitpid(pid, Some(status), options)
}

/// `System::waitpid(pid, status, options, timeout)`: an already reaped child first;
/// outside a coroutine, or with `WNOHANG`, plain waitpid; otherwise the coroutine
/// yields until the reaper hands it a child or `timeout` passes.
pub fn wait_pid(
    pid: libc::pid_t,
    status: &mut c_int,
    options: c_int,
    timeout: f64,
    ops: &Coroutines,
) -> Waited {
    let exited = waits(|waits| {
        if pid < 0 {
            waits.exited.pop_first()
        } else {
            waits.exited.remove_entry(&pid)
        }
    });
    if let Some((child, code)) = exited {
        *status = code;
        return Waited {
            pid: child,
            timed_out: false,
        };
    }
    let co = (ops.current)();
    if co.is_null() || options & libc::WNOHANG != 0 {
        return Waited {
            pid: waitpid(pid, status, options),
            timed_out: false,
        };
    }
    // SIGCHLD must be registered before this last try, or its signal may be lost.
    (ops.signal_init)();
    let child = waitpid(pid, status, options | libc::WNOHANG);
    if child != 0 {
        return Waited {
            pid: child,
            timed_out: false,
        };
    }
    let task = Arc::new(Task {
        co: AtomicPtr::new(co),
        pid: AtomicI32::new(-1),
        status: AtomicI32::new(0),
    });
    waits(|waits| {
        if pid < 0 {
            waits.any.push_back(Arc::clone(&task));
        } else {
            waits.by_pid.insert(pid, Arc::clone(&task));
        }
    });
    // The reaper hands `task` a child while this frame is suspended here.
    (ops.yield_for)(timeout);
    let child = task.pid();
    waits(|waits| {
        if pid >= 0 {
            waits.by_pid.remove(&pid);
        } else if child > 0 {
            waits.any.pop_front();
        } else {
            waits.any.retain(|waiting| !Arc::ptr_eq(waiting, &task));
        }
    });
    if child > 0 {
        *status = task.status.load(Relaxed);
    }
    Waited {
        pid: child,
        // `co` is the current coroutine again.
        timed_out: child <= 0 && (ops.timed_out)(),
    }
}

/// The SIGCHLD reaper: hands each exited child to the coroutine waiting for its PID,
/// else to the first coroutine waiting for any child, else keeps it for a later wait.
pub fn wait_reap(signo: c_int, ops: &Coroutines) {
    if signo != libc::SIGCHLD {
        return;
    }
    loop {
        let mut code = 0;
        let child = waitpid(-1, &mut code, libc::WNOHANG);
        if child <= 0 {
            break;
        }
        let task = waits(|waits| {
            let task = waits
                .by_pid
                .get(&child)
                .or_else(|| waits.any.front())
                .cloned();
            if task.is_none() {
                waits.exited.insert(child, code);
            }
            task
        });
        if let Some(task) = task {
            task.status.store(code, Relaxed);
            task.pid.store(child, Relaxed);
            let co = task.co.load(Relaxed);
            // SAFETY: `co` is suspended in `wait_pid`'s `yield_for` (only it registers
            // a task), which `Coroutines::new`'s caller vouched `resume` resumes.
            unsafe { (ops.resume)(co) };
        }
    }
}

/// `swoole_coroutine_wait_count`: coroutines waiting for a child.
#[must_use]
pub fn wait_count() -> usize {
    waits(|waits| waits.any.len() + waits.by_pid.len())
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use std::cell::Cell;

    /// Held by each test that spawns children: a wait for any child would reap
    /// another test's.
    pub(crate) static CHILDREN: Mutex<()> = Mutex::new(());

    thread_local! {
        static YIELDED: Cell<bool> = const { Cell::new(false) };
    }
    fn no_coroutine() -> *mut c_void {
        std::ptr::null_mut()
    }
    fn a_coroutine() -> *mut c_void {
        std::ptr::dangling_mut()
    }
    fn nothing() {}
    unsafe extern "C-unwind" fn resume(_: *mut c_void) {}
    static PLAIN: Coroutines = unsafe {
        Coroutines::new(
            no_coroutine,
            nothing,
            reap_while_yielded,
            not_timed_out,
            resume,
        )
    };
    fn not_timed_out() -> bool {
        false
    }
    /// Stands in for a suspension during which the child exits and SIGCHLD is reaped.
    fn reap_while_yielded(_: f64) {
        YIELDED.set(true);
        while wait_count() > 0 {
            wait_reap(libc::SIGCHLD, &PLAIN);
            let handed = waits(|waits| waits.any.front().is_some_and(|task| task.pid() > 0));
            if handed {
                break;
            }
            unsafe { libc::usleep(1000) };
        }
    }

    fn spawn(code: c_int) -> libc::pid_t {
        let pid = unsafe { libc::fork() };
        if pid == 0 {
            unsafe { libc::_exit(code) };
        }
        pid
    }

    #[test]
    #[cfg_attr(miri, ignore = "Miri can't spawn a process")]
    fn plain_and_coroutine_waits() {
        let _children = CHILDREN.lock().unwrap_or_else(PoisonError::into_inner);
        let child = spawn(3);
        let mut status = 0;
        let waited = wait_pid(child, &mut status, 0, -1.0, &PLAIN);
        assert_eq!((waited.pid, libc::WEXITSTATUS(status)), (child, 3));

        let coroutine = unsafe {
            Coroutines::new(
                a_coroutine,
                nothing,
                reap_while_yielded,
                not_timed_out,
                resume,
            )
        };
        let child = spawn(5);
        let waited = wait_pid(-1, &mut status, 0, -1.0, &coroutine);
        assert_eq!((waited.pid, waited.timed_out), (child, false));
        assert_eq!(libc::WEXITSTATUS(status), 5);
        assert_eq!(wait_count(), 0);

        let child = spawn(7);
        unsafe { libc::usleep(50_000) };
        wait_reap(libc::SIGCHLD, &coroutine);
        let waited = wait_pid(child, &mut status, 0, -1.0, &coroutine);
        assert_eq!((waited.pid, libc::WEXITSTATUS(status)), (child, 7));
    }
}
