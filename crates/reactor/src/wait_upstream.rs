// SPDX-License-Identifier: Apache-2.0
// Compatibility derived from Swoole v6.2.5 src/os/wait.cc (Swoole contributors).
//! Coroutine `waitpid` (`System::wait`, `waitpid`, `waitpid_safe` and their C API) and
//! `swoole_waitpid`, under their upstream (Itanium) symbols, over crates/process's wait
//! registry, or the `io_uring` engine when one is registered. The process patch deletes
//! `src/os/wait.cc`.
use std::ffi::{c_int, c_void};
use swoole_rs_corebase::log;
use swoole_rs_process::signal::swoole_signal_dispatch;
use swoole_rs_process::wait::wait_count;

// SAFETY: crates/timer's. A declaration, since crates/timer depends on this crate.
unsafe extern "C-unwind" {
    #[link_name = "_Z19swoole_timer_selectv"]
    fn swoole_timer_select();
}

fn errno() -> c_int {
    std::io::Error::last_os_error().raw_os_error().unwrap_or(0)
}

/// The SIGCHLD registry: the coroutine effects crates/process's wait loop asks for.
mod registry {
    use super::{c_int, c_void, wait_count};
    use crate::signal_upstream::{swoole_signal_clear, swoole_signal_set};
    use crate::{ReactorPrepared, swr_v1_reactor_deferred_push};
    use std::ptr;
    use std::sync::atomic::{AtomicBool, Ordering};
    use swoole_rs_process::wait::{Coroutines, wait_reap};

    /// `Reactor::EXIT_CONDITION_WAIT_PID`.
    const EXIT_CONDITION_WAIT_PID: c_int = 2;

    /// SIGCHLD's handler, exit condition and destroy callback are installed.
    static SIGNAL_READY: AtomicBool = AtomicBool::new(false);

    unsafe extern "C" fn resume(co: *mut c_void) {
        // SAFETY: the registry hands back a coroutine it holds suspended.
        unsafe { swoole_rs_coroutine::resume(co) };
    }

    extern "C" fn signal_handler(signo: c_int) {
        // SAFETY: the signal dispatch runs on the loop's thread.
        unsafe { wait_reap(signo, resume) };
    }

    extern "C-unwind" fn no_waiters(_event_num: &mut usize) -> bool {
        wait_count() == 0
    }

    unsafe extern "C-unwind" fn destroyed(_data: *mut c_void) {
        SIGNAL_READY.store(false, Ordering::Relaxed);
        // The loop's thread, which owns the handlers.
        swoole_signal_clear();
    }

    fn register() {
        // SAFETY: the loop's thread, which owns the handlers.
        let _ = unsafe {
            swoole_signal_set(libc::SIGCHLD, signal_handler as *const () as usize, false)
        };
    }

    extern "C-unwind" fn register_deferred(_data: *mut c_void) {
        register();
        signal_handler(libc::SIGCHLD);
    }

    /// SIGCHLD reaps into the registry while the loop runs; the loop waits for the
    /// coroutines that wait, and forgets the handler when it is destroyed.
    unsafe extern "C" fn signal_init() {
        if SIGNAL_READY.load(Ordering::Relaxed) {
            return;
        }
        let reactor = crate::event_api::swr_v1_event_current().cast::<ReactorPrepared>();
        // SAFETY: a coroutine runs, so the thread has a live loop, which begins with its
        // prepared state.
        let fields = unsafe { (*reactor).fields };
        if swoole_rs_coroutine::thread_context_active() {
            // Coroutine threads block every signal: register SIGCHLD on the loop's
            // thread, then reap the children that exited before it ran.
            // SAFETY: the live loop's fields.
            unsafe { crate::deferred::defer(fields, register_deferred, ptr::null_mut(), None) };
        } else {
            register();
        }
        crate::swr_v1_event_set_exit_condition(EXIT_CONDITION_WAIT_PID, no_waiters);
        // SAFETY: the loop's live fields.
        let queue = unsafe { &raw mut (*fields).destroy_callbacks };
        // SAFETY: the loop's live destroy queue; the call keeps no data.
        if unsafe { swr_v1_reactor_deferred_push(queue, destroyed, ptr::null_mut(), None) } != 0 {
            std::process::abort();
        }
        SIGNAL_READY.store(true, Ordering::Relaxed);
    }

    /// The current coroutine while the thread has a loop.
    unsafe extern "C" fn current() -> *mut c_void {
        // SAFETY: no preconditions.
        if crate::event_api::swr_v1_event_current().is_null() {
            return ptr::null_mut();
        }
        swoole_rs_coroutine::current().cast()
    }

    unsafe extern "C" fn yield_for(co: *mut c_void, timeout: f64) {
        // SAFETY: the current coroutine.
        let _ = unsafe { swoole_rs_coroutine::yield_ex(co, timeout) };
    }

    unsafe extern "C" fn timed_out(co: *mut c_void) -> bool {
        // SAFETY: the coroutine that just resumed, live.
        let code = unsafe { (*co.cast::<swoole_rs_coroutine::Coroutine>()).resume_code };
        code == swoole_rs_coroutine::RC_TIMEDOUT
    }

    pub(super) static COROUTINES: Coroutines = Coroutines {
        current,
        signal_init,
        yield_for,
        resume,
        timed_out,
    };
}

/// `Iouring::waitpid`, the last error set from errno when it fails.
///
/// # Safety
/// `status` is writable.
unsafe fn iouring_waitpid(
    uring: &crate::UringOps,
    pid: libc::pid_t,
    status: *mut c_int,
    options: c_int,
    timeout: f64,
) -> libc::pid_t {
    // SAFETY: as the caller promises.
    let result = unsafe { (uring.waitpid)(pid, status, options, timeout) };
    if result < 0 {
        log::set_last_error(errno());
    }
    result
}

/// `pid_t System::waitpid(pid_t, int *, int, double timeout)`: in a coroutine, yields
/// until the child exits or `timeout` passes (`ETIMEDOUT`, as errno and last error).
///
/// # Safety
/// `status` is writable.
#[unsafe(export_name = "_ZN6swoole9coroutine6System7waitpidEiPiid")]
pub unsafe extern "C" fn system_waitpid(
    pid: libc::pid_t,
    status: *mut c_int,
    options: c_int,
    timeout: f64,
) -> libc::pid_t {
    if let Some(uring) = crate::wait_loop::uring() {
        // SAFETY: as the caller promises.
        return unsafe { iouring_waitpid(uring, pid, status, options, timeout) };
    }
    // SAFETY: as the caller promises; the static table.
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
///
/// # Safety
/// `status` is writable.
#[unsafe(export_name = "_ZN6swoole9coroutine6System4waitEPid")]
pub unsafe extern "C" fn system_wait(status: *mut c_int, timeout: f64) -> libc::pid_t {
    // SAFETY: as the caller promises.
    unsafe { system_waitpid(-1, status, 0, timeout) }
}

/// `pid_t System::waitpid_safe(pid_t, int *, int)`: in a coroutine, polls
/// `waitpid(WNOHANG)` between coroutine sleeps that back off, without SIGCHLD.
///
/// # Safety
/// `status` is writable.
#[unsafe(export_name = "_ZN6swoole9coroutine6System12waitpid_safeEiPii")]
pub unsafe extern "C" fn system_waitpid_safe(
    pid: libc::pid_t,
    status: *mut c_int,
    options: c_int,
) -> libc::pid_t {
    /// `SW_OK`, `SW_FILE_LOCK_DEFAULT_SECOND` and `SW_FILE_LOCK_MAX_SECOND`.
    const OK: c_int = 0;
    const LOCK_DEFAULT_SECOND: f64 = 0.001;
    const LOCK_MAX_SECOND: f64 = 0.1;
    // SAFETY: crates/hooks's.
    unsafe extern "C" {
        #[link_name = "_ZN6swoole9coroutine6System5sleepEd"]
        fn system_sleep(seconds: f64) -> c_int;
    }
    // SAFETY: no preconditions.
    let outside = crate::event_api::swr_v1_event_current().is_null()
        || swoole_rs_coroutine::current().is_null();
    if outside || options & libc::WNOHANG != 0 {
        // SAFETY: as the caller promises.
        return unsafe { libc::waitpid(pid, status, options) };
    }
    if let Some(uring) = crate::wait_loop::uring() {
        // SAFETY: as the caller promises.
        return unsafe { iouring_waitpid(uring, pid, status, options, -1.0) };
    }
    let mut second = LOCK_DEFAULT_SECOND;
    loop {
        // SAFETY: as the caller promises.
        let result = unsafe { libc::waitpid(pid, status, libc::WNOHANG) };
        // SAFETY: in a coroutine.
        if result != 0 || unsafe { system_sleep(second) } != OK {
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
///
/// # Safety
/// `status` is writable.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn swoole_coroutine_wait(status: *mut c_int) -> libc::pid_t {
    // SAFETY: as the caller promises.
    unsafe { system_wait(status, -1.0) }
}

/// `pid_t swoole_coroutine_waitpid(pid_t, int *, int)`.
///
/// # Safety
/// `status` is writable.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn swoole_coroutine_waitpid(
    pid: libc::pid_t,
    status: *mut c_int,
    options: c_int,
) -> libc::pid_t {
    // SAFETY: as the caller promises.
    unsafe { system_waitpid(pid, status, options, -1.0) }
}

/// `pid_t swoole_waitpid(pid_t, int *, int)`: `waitpid`, dispatching signals and
/// running due timers whenever a signal interrupts it.
///
/// # Safety
/// `status` is writable.
#[unsafe(export_name = "_Z14swoole_waitpidiPii")]
pub unsafe extern "C" fn swoole_waitpid(
    pid: libc::pid_t,
    status: *mut c_int,
    options: c_int,
) -> libc::pid_t {
    loop {
        // SAFETY: as the caller promises.
        let result = unsafe { libc::waitpid(pid, status, options) };
        if result >= 0 || errno() != libc::EINTR {
            return result;
        }
        swoole_signal_dispatch();
        // SAFETY: the thread's timer, if any.
        unsafe { swoole_timer_select() };
    }
}
