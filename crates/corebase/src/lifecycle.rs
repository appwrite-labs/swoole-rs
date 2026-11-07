// SPDX-License-Identifier: Apache-2.0
// Compatibility derived from Swoole v6.2.4 src/core/base.cc (Swoole contributors).
//! base.cc's thread, clean and fork sequences. Each C++ effect (an object's
//! `new`/`delete`, `SwooleG = {}`, the timer, event, signal and coroutine leaves)
//! runs through the host `swoole_init` registers (`native.rs`); before that there
//! is nothing to set up or tear down.
use crate::log;
use crate::native::{self, NativeHost};
use std::ffi::{c_int, c_void};

/// `SW_FORK_EXEC`, `SW_FORK_DAEMON` and `SW_FORK_PRECHECK`.
const FORK_EXEC: c_int = 2;
const FORK_DAEMON: c_int = 4;
const FORK_PRECHECK: c_int = 8;
/// `SW_ERROR_OPERATION_NOT_SUPPORT` and `SW_GLOBAL_HOOK_AFTER_FORK`.
const OPERATION_NOT_SUPPORT: c_int = 507;
const HOOK_AFTER_FORK: c_int = 12;

/// The leading members of `swoole::ThreadGlobal` (`SwooleTG`) these sequences
/// touch; native/corebase/globals.cc asserts their offsets.
#[repr(C)]
struct ThreadGlobal {
    id: u16,
    kind: u8,
    main_thread: bool,
    buffer_stack: *mut c_void,
    reactor: *mut c_void,
    timer: *mut c_void,
    message_bus: *mut c_void,
    async_threads: *mut c_void,
}

/// This thread's `SwooleTG`. Native code changes it only inside host calls, so
/// a copy read between them is current.
fn thread(host: &NativeHost) -> *mut ThreadGlobal {
    host.thread().cast()
}

fn read<T: Copy>(host: &NativeHost, field: impl FnOnce(&ThreadGlobal) -> T) -> T {
    // SAFETY: this thread's live record; the borrow ends before the next host call.
    field(unsafe { &*thread(host) })
}

/// `SwooleTG.reactor`: null before `swoole_init`, as `SwooleTG` is then.
#[must_use]
pub fn thread_reactor() -> *mut c_void {
    native::host().map_or(std::ptr::null_mut(), |host| {
        read(host, |thread| thread.reactor)
    })
}

/// `swoole_get_thread_type()`: `SwooleTG.type`, 0 before `swoole_init`.
#[must_use]
pub fn thread_kind() -> u8 {
    native::host().map_or(0, |host| read(host, |thread| thread.kind))
}

/// `SwooleTG.async_threads != nullptr`.
#[must_use]
pub fn thread_async_threads() -> bool {
    native::host().is_some_and(|host| read(host, |thread| !thread.async_threads.is_null()))
}

/// `void swoole_thread_init(bool main_thread)`.
#[cfg_attr(
    feature = "upstream-symbols",
    unsafe(export_name = "_Z18swoole_thread_initb")
)]
pub extern "C-unwind" fn swoole_thread_init(main_thread: bool) {
    let Some(host) = native::host() else { return };
    if read(host, |thread| thread.buffer_stack.is_null()) {
        host.buffer(true);
    }
    if !main_thread {
        host.signal_block_all();
    }
    // SAFETY: this thread's live record, borrowed for the write alone.
    unsafe { (*thread(host)).main_thread = main_thread };
}

/// `void swoole_thread_clean(bool main_thread)`.
#[cfg_attr(
    feature = "upstream-symbols",
    unsafe(export_name = "_Z19swoole_thread_cleanb")
)]
pub extern "C-unwind" fn swoole_thread_clean(_main_thread: bool) {
    let Some(host) = native::host() else { return };
    if read(host, |thread| !thread.timer.is_null()) {
        host.timer_free();
    }
    if read(host, |thread| !thread.reactor.is_null()) {
        host.event_free();
    }
    if read(host, |thread| !thread.buffer_stack.is_null()) {
        host.buffer(false);
    }
}

/// `bool swoole_is_main_thread()`.
#[cfg_attr(
    feature = "upstream-symbols",
    unsafe(export_name = "_Z21swoole_is_main_threadv")
)]
#[must_use]
pub extern "C" fn swoole_is_main_thread() -> bool {
    native::host().is_some_and(|host| read(host, |thread| thread.main_thread))
}

/// `void swoole_clean()`: the hooks, signals and this thread first, then the
/// logger and the memory pool, and `SwooleG` last.
#[cfg_attr(
    feature = "upstream-symbols",
    unsafe(export_name = "_Z12swoole_cleanv")
)]
pub extern "C-unwind" fn swoole_clean() {
    crate::hook::clear();
    let Some(host) = native::host() else { return };
    host.signal_clear();
    swoole_thread_clean(true);
    if !crate::swr_v1_core_logger().is_null() {
        host.close_logger();
    }
    host.delete_logger();
    host.delete_pool(crate::swr_v1_core_memory_pool());
    host.reset();
    crate::swr_v1_core_reset();
}

/// `pid_t swoole_fork(int flags)`: fork outside coroutines and async threads,
/// retrying each second while fork fails; the child drops the parent's timer,
/// pool, reactor and signals, then runs the after-fork hooks. -1 before
/// `swoole_init`.
#[cfg_attr(feature = "upstream-symbols", unsafe(export_name = "_Z11swoole_forki"))]
#[must_use]
pub extern "C-unwind" fn swoole_fork(flags: c_int) -> libc::pid_t {
    let Some(host) = native::host() else {
        return -1;
    };
    if flags & FORK_EXEC == 0 {
        // `SwooleG.fatal_error` may return, so each check runs.
        if host.coroutine_is_in() {
            let message = c"must be forked outside the coroutine";
            // SAFETY: the frames a native longjmp skips own nothing.
            unsafe { crate::fatal_error(OPERATION_NOT_SUPPORT, message) };
        }
        if read(host, |thread| !thread.async_threads.is_null()) {
            let message = c"can not fork after using async-threads";
            // SAFETY: as above.
            unsafe { crate::fatal_error(OPERATION_NOT_SUPPORT, message) };
        }
    }
    if flags & FORK_PRECHECK != 0 {
        return 0;
    }
    let pid = loop {
        // SAFETY: fork has no memory preconditions; the child continues here.
        let pid = unsafe { libc::fork() };
        if pid >= 0 {
            break pid;
        }
        log::sys_warning(c"swoole_fork", format_args!("fork() failed"));
        // SAFETY: sleep has no memory preconditions.
        unsafe { libc::sleep(1) };
    };
    if pid > 0 || flags & FORK_DAEMON != 0 {
        return pid;
    }
    if host.timer_available() {
        host.timer_free();
    }
    if flags & FORK_EXEC == 0 {
        // The parent's shared pages stay with the OS.
        crate::swr_v1_core_set_memory_pool(host.new_pool());
        host.reopen_logger();
        if host.event_available() {
            host.event_free();
        }
    } else {
        host.close_logger();
    }
    host.signal_clear();
    if crate::swoole_isset_hook(HOOK_AFTER_FORK) {
        // SAFETY: the after-fork hooks take no argument.
        unsafe { crate::swoole_call_hook(HOOK_AFTER_FORK, std::ptr::null_mut()) };
    }
    0
}

/// `swoole_fork_exec`'s fork: the child runs `call(function)` and exits 0; the
/// parent gets the pid, and 0 when fork fails (upstream returns `false`).
///
/// # Safety
/// `call` accepts `function`; a C++ exception it throws unwinds through here.
#[unsafe(no_mangle)]
pub unsafe extern "C-unwind" fn swr_v1_core_fork_exec(
    call: unsafe extern "C-unwind" fn(*const c_void),
    function: *const c_void,
) -> libc::pid_t {
    // SAFETY: fork has no memory preconditions.
    match unsafe { libc::fork() } {
        pid if pid < 0 => 0,
        0 => {
            // SAFETY: the caller's contract.
            unsafe { call(function) };
            // SAFETY: the child ends here, as upstream's exit(0).
            unsafe { libc::exit(0) }
        }
        pid => pid,
    }
}
