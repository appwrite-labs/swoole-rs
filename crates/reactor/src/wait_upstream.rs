// SPDX-License-Identifier: Apache-2.0
// Compatibility derived from Swoole v6.2.5 src/os/wait.cc (Swoole contributors).
//! Coroutine `waitpid` (`System::wait`, `waitpid`, `waitpid_safe` and their C API) and
//! `swoole_waitpid`, under their upstream (Itanium) symbols, over crates/process's wait
//! registry, or the `io_uring` engine when one is registered. The process patch deletes
//! `src/os/wait.cc`.
use std::ffi::{c_int, c_void};
use swoole_rs_corebase::{log, sys};
use swoole_rs_process::signal::swoole_signal_dispatch;
use swoole_rs_process::wait::wait_count;

// crates/timer's, a safe function there. A declaration, since crates/timer
// depends on this crate.
unsafe extern "C-unwind" {
    #[link_name = "_Z19swoole_timer_selectv"]
    safe fn swoole_timer_select();
}

fn errno() -> c_int {
    std::io::Error::last_os_error().raw_os_error().unwrap_or(0)
}

/// The SIGCHLD registry: the coroutine effects crates/process's wait loop asks for.
mod registry {
    use super::{c_int, c_void, wait_count};
    use crate::signal_upstream::{self, Handler};
    use std::ptr;
    use std::sync::atomic::{AtomicBool, Ordering};
    use swoole_rs_process::wait::{Coroutines, wait_reap};

    /// SIGCHLD's handler, exit condition and destroy callback are installed.
    static SIGNAL_READY: AtomicBool = AtomicBool::new(false);

    extern "C" fn signal_handler(signo: c_int) {
        // SAFETY: the signal dispatch runs on the loop's thread; the registry hands
        // `resume` the coroutines it holds suspended.
        unsafe { wait_reap(signo, swoole_rs_coroutine::resume) };
    }

    extern "C-unwind" fn no_waiters(_event_num: &mut usize) -> bool {
        wait_count() == 0
    }

    extern "C-unwind" fn destroyed(_data: *mut c_void) {
        SIGNAL_READY.store(false, Ordering::Relaxed);
        signal_upstream::clear();
    }

    fn register() {
        _ = signal_upstream::set(libc::SIGCHLD, Handler::new(signal_handler), false);
    }

    /// SIGCHLD's registration, deferred to the loop's thread.
    struct Register;

    impl crate::event_api::Deferred for Register {
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
        if swoole_rs_coroutine::thread_context_active() {
            // Coroutine threads block every signal: register SIGCHLD on the loop's
            // thread, then reap the children that exited before it ran. A coroutine
            // runs, so the thread has a live loop.
            _ = crate::event_api::swoole_event_defer_value(Register);
        } else {
            register();
        }
        crate::swr_v1_event_set_exit_condition(crate::ExitCondition::WaitPid, no_waiters);
        crate::event_api::on_destroy(destroyed);
        SIGNAL_READY.store(true, Ordering::Relaxed);
    }

    /// The current coroutine while the thread has a loop.
    fn current() -> *mut c_void {
        if crate::event_api::swr_v1_event_current().is_null() {
            return ptr::null_mut();
        }
        swoole_rs_coroutine::current().cast()
    }

    fn yield_for(timeout: f64) {
        let co = swoole_rs_coroutine::current();
        if !co.is_null() {
            // `co->yield_ex(timeout)` of the running coroutine.
            let code = swoole_rs_coroutine::yield_timed(co.cast(), timeout);
            _ = swoole_rs_coroutine::yield_resumed(code);
        }
    }

    fn timed_out() -> bool {
        swoole_rs_coroutine::current_resume_code() == Some(swoole_rs_coroutine::RC_TIMEDOUT)
    }

    pub(super) static COROUTINES: Coroutines = Coroutines {
        current,
        signal_init,
        yield_for,
        timed_out,
    };
}

/// `Iouring::waitpid`, the last error set from errno when it fails.
fn iouring_waitpid(
    uring: &crate::UringOps,
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
    if let Some(uring) = crate::wait_loop::uring() {
        return iouring_waitpid(uring, pid, Some(status), options, timeout);
    }
    // SAFETY: the static table of native coroutine operations.
    let waited = unsafe {
        swoole_rs_process::wait::wait_pid(pid, status, options, timeout, &registry::COROUTINES)
    };
    if waited.timed_out {
        crate::socket_sync::set_errno(libc::ETIMEDOUT);
        log::set_last_error(libc::ETIMEDOUT);
    }
    waited.pid
}

/// `pid_t System::wait(int *, double timeout)`: any child.
#[unsafe(export_name = "_ZN6swoole9coroutine6System4waitEPid")]
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
    /// `SW_OK`, `SW_FILE_LOCK_DEFAULT_SECOND` and `SW_FILE_LOCK_MAX_SECOND`.
    const OK: c_int = swoole_rs_corebase::constants::ResultCode::SW_OK as c_int;
    const LOCK_DEFAULT_SECOND: f64 = 0.001;
    const LOCK_MAX_SECOND: f64 = 0.1;
    // crates/hooks's, a safe function there; declared, since crates/hooks
    // depends on this crate.
    unsafe extern "C-unwind" {
        #[link_name = "_ZN6swoole9coroutine6System5sleepEd"]
        safe fn system_sleep(seconds: f64) -> c_int;
    }
    let mut status = status;
    let outside = crate::event_api::swr_v1_event_current().is_null()
        || swoole_rs_coroutine::current().is_null();
    if outside || options & libc::WNOHANG != 0 {
        return sys::waitpid(pid, status, options);
    }
    if let Some(uring) = crate::wait_loop::uring() {
        return iouring_waitpid(uring, pid, status, options, -1.0);
    }
    let mut second = LOCK_DEFAULT_SECOND;
    loop {
        let result = sys::waitpid(pid, status.as_deref_mut(), libc::WNOHANG);
        // In a coroutine, checked above.
        if result != 0 || system_sleep(second) != OK {
            return result;
        }
        second = crate::backoff(second, LOCK_DEFAULT_SECOND, LOCK_MAX_SECOND);
    }
}

/// `size_t swoole_coroutine_wait_count()`: coroutines waiting for a child.
#[must_use]
#[unsafe(no_mangle)]
pub extern "C" fn swoole_coroutine_wait_count() -> usize {
    wait_count()
}

/// `pid_t swoole_coroutine_wait(int *)`.
#[unsafe(no_mangle)]
pub extern "C" fn swoole_coroutine_wait(status: &mut c_int) -> libc::pid_t {
    system_wait(status, -1.0)
}

/// `pid_t swoole_coroutine_waitpid(pid_t, int *, int)`.
#[unsafe(no_mangle)]
pub extern "C" fn swoole_coroutine_waitpid(
    pid: libc::pid_t,
    status: &mut c_int,
    options: c_int,
) -> libc::pid_t {
    system_waitpid(pid, status, options, -1.0)
}

/// `pid_t swoole_waitpid(pid_t, int *, int)`: `waitpid`, dispatching signals and
/// running due timers whenever a signal interrupts it. `status` may be null.
#[unsafe(export_name = "_Z14swoole_waitpidiPii")]
#[must_use]
pub extern "C" fn swoole_waitpid(
    pid: libc::pid_t,
    status: Option<&mut c_int>,
    options: c_int,
) -> libc::pid_t {
    let mut status = status;
    loop {
        let result = sys::waitpid(pid, status.as_deref_mut(), options);
        if result >= 0 || errno() != libc::EINTR {
            return result;
        }
        swoole_signal_dispatch();
        // The thread's timer, if any.
        swoole_timer_select();
    }
}
