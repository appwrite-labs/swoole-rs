// SPDX-License-Identifier: Apache-2.0
// Compatibility derived from Swoole v6.2.5 src/os/wait.cc (Swoole contributors).
//! Coroutine `waitpid` (`System::wait`, `waitpid`, `waitpid_safe`) and
//! `swoole_waitpid` over `corebase::process`'s wait
//! registry, or the `io_uring` engine when one is registered. The process patch deletes
//! `src/os/wait.cc`. Only `System::waitpid` and `waitpid_safe` retain native exports
//! for bundled `proc_open`, selected by its `SW_THREAD` build configuration.
use crate::process::signal::swoole_signal_dispatch;
use crate::process::wait::wait_count;
use crate::{log, sys};
use std::ffi::{c_int, c_void};

use crate::sys::errno;

/// The SIGCHLD registry: the coroutine effects `corebase::process`'s wait loop asks for.
mod registry {
    use super::{c_int, c_void, wait_count};
    use crate::process::wait::{Coroutines, Wake, wait_reap};
    use crate::reactor::signal_upstream::{self, Handler};
    use std::ptr;
    use std::sync::atomic::{AtomicBool, Ordering};

    /// SIGCHLD's handler, exit condition and destroy callback are installed.
    static SIGNAL_READY: AtomicBool = AtomicBool::new(false);

    extern "C" fn signal_handler(signo: c_int) {
        wait_reap(signo, &COROUTINES);
    }

    fn no_waiters(_event_num: &mut usize) -> bool {
        wait_count() == 0
    }

    fn destroyed() {
        SIGNAL_READY.store(false, Ordering::Relaxed);
        signal_upstream::clear();
    }

    fn register() {
        _ = signal_upstream::set(libc::SIGCHLD, Handler::new(signal_handler), false);
    }

    /// SIGCHLD's registration, deferred to the loop's thread.
    struct Register;

    impl crate::reactor::event_api::Deferred for Register {
        fn run(self) {
            register();
            signal_handler(libc::SIGCHLD);
        }

        fn dispose(self) {}
    }

    /// SIGCHLD reaps into the registry while the loop runs; the loop waits for the
    /// coroutines that wait, and forgets the handler when it is destroyed.
    fn signal_init() {
        if SIGNAL_READY.load(Ordering::Relaxed) {
            return;
        }
        if crate::coroutine::thread_context_active() {
            // Coroutine threads block every signal: register SIGCHLD on the loop's
            // thread, then reap the children that exited before it ran. A coroutine
            // runs, so the thread has a live loop.
            _ = crate::reactor::event_api::swoole_event_defer_value(Register);
        } else {
            register();
        }
        crate::reactor::swr_v1_event_set_exit_condition(
            crate::reactor::ExitCondition::WaitPid,
            no_waiters,
        );
        crate::reactor::event_api::on_destroy(destroyed);
        SIGNAL_READY.store(true, Ordering::Relaxed);
    }

    /// The current coroutine while the thread has a loop.
    fn current() -> *mut c_void {
        if crate::reactor::event_api::swr_v1_event_current().is_null() {
            return ptr::null_mut();
        }
        crate::coroutine::current().cast()
    }

    fn yield_for(timeout: f64, arm: &dyn Fn(Wake)) {
        let co = crate::coroutine::current();
        if !co.is_null() {
            // `co->yield_ex(timeout)` of the running coroutine, its waker armed first.
            let code = crate::coroutine::yield_timed_armed(co.cast(), timeout, |waker| {
                let (cid, park) = waker.parts();
                arm(Wake { cid, park });
            });
            _ = crate::coroutine::yield_resumed(code);
        }
    }

    fn timed_out() -> bool {
        crate::coroutine::current_resume_code() == Some(crate::coroutine::Resume::TimedOut)
    }

    /// `co->resume()` on the loop's thread, where the signal dispatch reaps.
    fn wake(wake: Wake) {
        crate::coroutine::Waker::of_parts((wake.cid, wake.park)).wake();
    }

    pub(super) static COROUTINES: Coroutines = Coroutines {
        current,
        signal_init,
        yield_for,
        timed_out,
        wake,
    };
}

/// `Iouring::waitpid`, the last error set from errno when it fails.
fn iouring_waitpid(
    uring: &crate::reactor::UringOps,
    pid: libc::pid_t,
    status: Option<&mut c_int>,
    options: c_int,
    timeout: f64,
) -> libc::pid_t {
    let result = (uring.waitpid)(pid, status, options, timeout);
    if result < 0 {
        log::set_last_error(errno());
    }
    result
}

/// `pid_t System::waitpid(pid_t, int *, int, double timeout)`: in a coroutine, yields
/// until the child exits or `timeout` passes (`ETIMEDOUT`, as errno and last error).
#[unsafe(export_name = "_ZN6swoole9coroutine6System7waitpidEiPiid")]
pub extern "C" fn system_waitpid(
    pid: libc::pid_t,
    status: &mut c_int,
    options: c_int,
    timeout: f64,
) -> libc::pid_t {
    if let Some(uring) = crate::reactor::wait_loop::uring() {
        return iouring_waitpid(uring, pid, Some(status), options, timeout);
    }
    let waited =
        crate::process::wait::wait_pid(pid, status, options, timeout, &registry::COROUTINES);
    if waited.timed_out {
        crate::reactor::socket_sync::set_errno(libc::ETIMEDOUT);
        log::set_last_error(libc::ETIMEDOUT);
    }
    waited.pid.map_or(-1, |pid| pid.unwrap_or(0))
}

/// `pid_t System::wait(int *, double timeout)`: any child.
pub extern "C" fn system_wait(status: &mut c_int, timeout: f64) -> libc::pid_t {
    system_waitpid(-1, status, 0, timeout)
}

/// `pid_t System::waitpid_safe(pid_t, int *, int)`: in a coroutine, polls
/// `waitpid(WNOHANG)` between coroutine sleeps that back off, without SIGCHLD.
/// `status` may be null, as `waitpid`'s.
#[unsafe(export_name = "_ZN6swoole9coroutine6System12waitpid_safeEiPii")]
#[must_use]
pub extern "C" fn system_waitpid_safe(
    pid: libc::pid_t,
    status: Option<&mut c_int>,
    options: c_int,
) -> libc::pid_t {
    /// `SW_FILE_LOCK_DEFAULT_SECOND` and `SW_FILE_LOCK_MAX_SECOND`.
    const LOCK_DEFAULT_SECOND: f64 = 0.001;
    const LOCK_MAX_SECOND: f64 = 0.1;
    let mut status = status;
    let outside = crate::reactor::event_api::swr_v1_event_current().is_null()
        || crate::coroutine::current().is_null();
    if outside || options & libc::WNOHANG != 0 {
        return sys::waitpid(pid, status, options).map_or(-1, |pid| pid.unwrap_or(0));
    }
    if let Some(uring) = crate::reactor::wait_loop::uring() {
        return iouring_waitpid(uring, pid, status, options, -1.0);
    }
    let mut second = LOCK_DEFAULT_SECOND;
    loop {
        match sys::waitpid(pid, status.as_deref_mut(), libc::WNOHANG) {
            Ok(Some(child)) => return child,
            Err(_) => return -1,
            // In a coroutine, checked above. A failed sleep keeps its errno and
            // returns the wait's no-child result, as upstream does.
            Ok(None) if !(crate::reactor::host::get().sleep)(second) => return 0,
            Ok(None) => {}
        }
        second = crate::reactor::backoff(second, LOCK_DEFAULT_SECOND, LOCK_MAX_SECOND);
    }
}

/// `pid_t swoole_waitpid(pid_t, int *, int)`: `waitpid`, dispatching signals and
/// running due timers whenever a signal interrupts it. `status` may be null.
#[must_use]
pub fn swoole_waitpid(pid: libc::pid_t, status: Option<&mut c_int>, options: c_int) -> libc::pid_t {
    let mut status = status;
    loop {
        match sys::waitpid(pid, status.as_deref_mut(), options) {
            Ok(child) => return child.unwrap_or(0),
            Err(error) if error.raw_os_error() == Some(libc::EINTR) => {}
            Err(_) => return -1,
        }
        swoole_signal_dispatch();
        // The thread's timer, if any.
        (crate::reactor::host::get().timer_select)();
    }
}
