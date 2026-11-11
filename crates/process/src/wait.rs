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
use std::sync::{Mutex, PoisonError};

/// A coroutine waiting for a child (upstream's `WaitTask`). It lives on the waiting
/// coroutine's stack while it is registered.
struct Task {
    co: *mut c_void,
    pid: libc::pid_t,
    status: c_int,
}

struct Waits {
    any: VecDeque<*mut Task>,
    by_pid: BTreeMap<libc::pid_t, *mut Task>,
    exited: BTreeMap<libc::pid_t, c_int>,
}

// SAFETY: each task pointer is used only by the thread holding the coroutine
// baton, while its coroutine is suspended.
unsafe impl Send for Waits {}

static WAITS: Mutex<Waits> = Mutex::new(Waits {
    any: VecDeque::new(),
    by_pid: BTreeMap::new(),
    exited: BTreeMap::new(),
});

fn waits<T>(f: impl FnOnce(&mut Waits) -> T) -> T {
    f(&mut WAITS.lock().unwrap_or_else(PoisonError::into_inner))
}

/// The native coroutine operations a wait needs.
#[repr(C)]
pub struct Coroutines {
    /// The current coroutine if this thread has a reactor, else null.
    pub current: unsafe extern "C" fn() -> *mut c_void,
    /// Registers the SIGCHLD reaper and the reactor callbacks, once.
    pub signal_init: unsafe extern "C" fn(),
    /// `co->yield_ex(timeout)`.
    pub yield_for: unsafe extern "C" fn(*mut c_void, f64),
    /// `co->resume()`.
    pub resume: unsafe extern "C" fn(*mut c_void),
    /// `co->is_timedout()`.
    pub timed_out: unsafe extern "C" fn(*mut c_void) -> bool,
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
    // SAFETY: `status` is writable.
    unsafe { libc::waitpid(pid, status, options) }
}

/// `System::waitpid(pid, status, options, timeout)`: an already reaped child first;
/// outside a coroutine, or with `WNOHANG`, plain waitpid; otherwise the coroutine
/// yields until the reaper hands it a child or `timeout` passes.
///
/// # Safety
/// `ops` holds the native coroutine operations.
pub unsafe fn wait_pid(
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
    // SAFETY: the native accessor has no preconditions.
    let co = unsafe { (ops.current)() };
    if co.is_null() || options & libc::WNOHANG != 0 {
        return Waited {
            pid: waitpid(pid, status, options),
            timed_out: false,
        };
    }
    // SIGCHLD must be registered before this last try, or its signal may be lost.
    // SAFETY: as above.
    unsafe { (ops.signal_init)() };
    let child = waitpid(pid, status, options | libc::WNOHANG);
    if child != 0 {
        return Waited {
            pid: child,
            timed_out: false,
        };
    }
    let mut task = Task {
        co,
        pid: -1,
        status: 0,
    };
    let registered = &raw mut task;
    waits(|waits| {
        if pid < 0 {
            waits.any.push_back(registered);
        } else {
            waits.by_pid.insert(pid, registered);
        }
    });
    // SAFETY: `co` is the current coroutine; the reaper writes `task` through
    // `registered` while this frame is suspended here.
    unsafe { (ops.yield_for)(co, timeout) };
    waits(|waits| {
        if pid >= 0 {
            waits.by_pid.remove(&pid);
        } else if task.pid > 0 {
            waits.any.pop_front();
        } else {
            waits.any.retain(|&waiting| waiting != registered);
        }
    });
    if task.pid > 0 {
        *status = task.status;
    }
    Waited {
        pid: task.pid,
        // SAFETY: `co` is the current coroutine again.
        timed_out: task.pid <= 0 && unsafe { (ops.timed_out)(co) },
    }
}

/// The SIGCHLD reaper: hands each exited child to the coroutine waiting for its PID,
/// else to the first coroutine waiting for any child, else keeps it for a later wait.
///
/// # Safety
/// `resume` is `co->resume()`.
pub unsafe fn wait_reap(signo: c_int, resume: unsafe extern "C" fn(*mut c_void)) {
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
                .copied();
            if task.is_none() {
                waits.exited.insert(child, code);
            }
            task
        });
        if let Some(task) = task {
            // SAFETY: a registered task lives on its suspended coroutine's stack.
            let task = unsafe { &mut *task };
            task.status = code;
            task.pid = child;
            let co = task.co;
            // SAFETY: `co` is suspended in `wait_pid`.
            unsafe { resume(co) };
        }
    }
}

/// `swoole_coroutine_wait_count`: coroutines waiting for a child.
#[must_use]
pub fn wait_count() -> usize {
    waits(|waits| waits.any.len() + waits.by_pid.len())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::Cell;

    thread_local! {
        static YIELDED: Cell<bool> = const { Cell::new(false) };
    }
    unsafe extern "C" fn no_coroutine() -> *mut c_void {
        std::ptr::null_mut()
    }
    unsafe extern "C" fn a_coroutine() -> *mut c_void {
        std::ptr::dangling_mut()
    }
    unsafe extern "C" fn nothing() {}
    unsafe extern "C" fn resume(_: *mut c_void) {}
    unsafe extern "C" fn not_timed_out(_: *mut c_void) -> bool {
        false
    }
    /// Stands in for a suspension during which the child exits and SIGCHLD is reaped.
    unsafe extern "C" fn reap_while_yielded(_: *mut c_void, _: f64) {
        YIELDED.set(true);
        while wait_count() > 0 {
            unsafe { wait_reap(libc::SIGCHLD, resume) };
            let handed = waits(|waits| {
                waits
                    .any
                    .front()
                    .is_some_and(|&task| unsafe { (*task).pid } > 0)
            });
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
        let plain = Coroutines {
            current: no_coroutine,
            signal_init: nothing,
            yield_for: reap_while_yielded,
            resume,
            timed_out: not_timed_out,
        };
        let child = spawn(3);
        let mut status = 0;
        let waited = unsafe { wait_pid(child, &mut status, 0, -1.0, &plain) };
        assert_eq!((waited.pid, libc::WEXITSTATUS(status)), (child, 3));

        let coroutine = Coroutines {
            current: a_coroutine,
            ..plain
        };
        let child = spawn(5);
        let waited = unsafe { wait_pid(-1, &mut status, 0, -1.0, &coroutine) };
        assert_eq!((waited.pid, waited.timed_out), (child, false));
        assert_eq!(libc::WEXITSTATUS(status), 5);
        assert_eq!(wait_count(), 0);

        let child = spawn(7);
        unsafe { libc::usleep(50_000) };
        unsafe { wait_reap(libc::SIGCHLD, resume) };
        let waited = unsafe { wait_pid(child, &mut status, 0, -1.0, &coroutine) };
        assert_eq!((waited.pid, libc::WEXITSTATUS(status)), (child, 7));
    }
}
