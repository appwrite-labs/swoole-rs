// SPDX-License-Identifier: Apache-2.0
// Compatibility derived from Swoole v6.2.5 src/os/wait.cc (Swoole contributors).
//! Coroutine `waitpid` (`System::wait`, `waitpid`) and
//! `swoole_waitpid` over `corebase::process`'s wait
//! registry, or the `io_uring` engine when one is registered. The process patch deletes
//! `src/os/wait.cc`. Hooks owns the polling `System::waitpid_safe` used by threaded `proc_open`.
use crate::process::signal::swoole_signal_dispatch;
use crate::process::wait::wait_count;
use crate::{log, sys};
use std::ffi::{c_int, c_void};

use crate::sys::errno;

/// The SIGCHLD registry: the coroutine effects `corebase::process`'s wait loop asks for.
mod registry {
    use super::{c_int, c_void, wait_count};
    use crate::process::signal_upstream::{self, Handler};
    use crate::process::wait::{Coroutines, Wake, wait_reap};
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

    fn destroyed(reactor: crate::reactor::Running<'_>) {
        SIGNAL_READY.store(false, Ordering::Relaxed);
        signal_upstream::clear_destroyed(reactor);
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

    fn yield_for(deadline: crate::timer::Deadline, arm: &dyn Fn(Wake)) {
        let co = crate::coroutine::current();
        if !co.is_null() {
            // `co->yield_ex(timeout)` of the running coroutine, its waker armed first.
            let code = crate::coroutine::yield_timed_armed(co.cast(), deadline, |waker| {
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
#[must_use]
pub fn iouring_waitpid(
    pid: libc::pid_t,
    status: Option<&mut c_int>,
    options: c_int,
    timeout: f64,
) -> Option<libc::pid_t> {
    let uring = crate::reactor::readiness::uring()?;
    let result = (uring.waitpid)(pid, status, options, timeout);
    if result < 0 {
        log::set_last_error(errno());
    }
    Some(result)
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
    if let Some(result) = iouring_waitpid(pid, Some(status), options, timeout) {
        return result;
    }
    #[expect(
        clippy::disallowed_methods,
        reason = "waitpid's timeout, from its PHP caller"
    )]
    let deadline = crate::timer::Deadline::from_timeout(timeout);
    let waited =
        crate::process::wait::wait_pid(pid, status, options, deadline, &registry::COROUTINES);
    if waited.timed_out {
        crate::set_errno(libc::ETIMEDOUT);
        log::set_last_error(libc::ETIMEDOUT);
    }
    waited.pid.map_or(-1, |pid| pid.unwrap_or(0))
}

/// `pid_t System::wait(int *, double timeout)`: any child.
pub extern "C" fn system_wait(status: &mut c_int, timeout: f64) -> libc::pid_t {
    system_waitpid(-1, status, 0, timeout)
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
        crate::timer::swoole_timer_select();
    }
}
