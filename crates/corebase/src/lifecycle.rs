// SPDX-License-Identifier: Apache-2.0
// Compatibility derived from Swoole v6.2.4 src/core/base.cc (Swoole contributors).
//! base.cc's init, thread, clean and fork sequences, and `SwooleTG`. Each leaf in
//! a crate above this one (an object's `new`/`delete`, the timer, event, signal
//! and coroutine leaves) runs through the host crates/ffi's `swoole_init`
//! registers (`native.rs`); without it there is nothing to set up or tear down.
use crate::log;
use crate::native;
use std::cell::Cell;
use std::ffi::{c_int, c_void};
use std::ptr;

/// `SW_FORK_EXEC`, `SW_FORK_DAEMON` and `SW_FORK_PRECHECK`.
const FORK_EXEC: c_int = 2;
const FORK_DAEMON: c_int = 4;
const FORK_PRECHECK: c_int = 8;
/// `SW_ERROR_OPERATION_NOT_SUPPORT` and `SW_GLOBAL_HOOK_AFTER_FORK`.
const OPERATION_NOT_SUPPORT: c_int = 507;
const HOOK_AFTER_FORK: c_int = 12;

/// `swoole::ThreadGlobal`, `SwooleTG`: Rust holds each thread's, and native code
/// reaches it through `swr_v1_core_thread_global` (swoole.h's `SwooleTG`), whose
/// struct asserts this layout.
#[repr(C)]
pub struct ThreadGlobal {
    id: Cell<u16>,
    kind: Cell<u8>,
    main_thread: Cell<bool>,
    buffer_stack: Cell<*mut c_void>,
    reactor: Cell<*mut c_void>,
    timer: Cell<*mut c_void>,
    message_bus: Cell<*mut c_void>,
    async_threads: Cell<*mut c_void>,
}

const _: () = assert!(
    std::mem::offset_of!(ThreadGlobal, main_thread) == 3
        && std::mem::offset_of!(ThreadGlobal, buffer_stack) == 8
        && std::mem::offset_of!(ThreadGlobal, async_threads) == 40
        && size_of::<ThreadGlobal>() == 48,
    "swoole_rs_corebase_state.h asserts the same `swoole::ThreadGlobal` layout"
);

thread_local! {
    static THREAD: ThreadGlobal = const {
        ThreadGlobal {
            id: Cell::new(0),
            kind: Cell::new(0),
            main_thread: Cell::new(false),
            buffer_stack: Cell::new(ptr::null_mut()),
            reactor: Cell::new(ptr::null_mut()),
            timer: Cell::new(ptr::null_mut()),
            message_bus: Cell::new(ptr::null_mut()),
            async_threads: Cell::new(ptr::null_mut()),
        }
    };
}

/// This thread's `SwooleTG`, live for the thread's life (it has no destructor). Every
/// field is a cell, so native code may write through the pointer.
#[unsafe(no_mangle)]
pub extern "C" fn swr_v1_core_thread_global() -> *mut ThreadGlobal {
    THREAD.with(|thread| ptr::from_ref(thread).cast_mut())
}

/// `&SwooleTG.timer`, for crates/timer and crates/server.
#[must_use]
pub fn swr_v1_core_timer_slot() -> *mut *mut c_void {
    // The field's address inside this thread's live record; computing it reads nothing.
    swr_v1_core_thread_global()
        .wrapping_byte_add(std::mem::offset_of!(ThreadGlobal, timer))
        .cast()
}

/// `SwooleTG.timer`: this thread's timer, or null.
#[must_use]
pub fn thread_timer() -> *mut c_void {
    with(|thread| thread.timer.get())
}

/// Sets `SwooleTG.timer`.
pub fn set_thread_timer(timer: *mut c_void) {
    with(|thread| thread.timer.set(timer));
}

/// `field` over this thread's `SwooleTG`.
fn with<T>(field: impl FnOnce(&ThreadGlobal) -> T) -> T {
    THREAD.with(field)
}

/// `SwooleTG.reactor`.
#[must_use]
pub fn thread_reactor() -> *mut c_void {
    with(|thread| thread.reactor.get())
}

/// `swoole_get_thread_type()`: `SwooleTG.type`.
#[must_use]
pub fn thread_kind() -> u8 {
    with(|thread| thread.kind.get())
}

/// `swoole_set_thread_type(kind)` then `swoole_set_thread_id(id)`.
pub fn set_thread(kind: u8, id: u16) {
    with(|thread| {
        thread.kind.set(kind);
        thread.id.set(id);
    });
}

/// `swoole_get_thread_id()`: `SwooleTG.id`.
#[must_use]
pub fn thread_id() -> u16 {
    with(|thread| thread.id.get())
}

/// `SwooleTG.message_bus`.
#[must_use]
pub fn thread_message_bus() -> *mut c_void {
    with(|thread| thread.message_bus.get())
}

/// Sets `SwooleTG.message_bus`.
pub fn set_thread_message_bus(bus: *mut c_void) {
    with(|thread| thread.message_bus.set(bus));
}

/// Sets `SwooleTG.reactor`.
pub fn set_thread_reactor(reactor: *mut c_void) {
    with(|thread| thread.reactor.set(reactor));
}

/// `SwooleTG.async_threads`.
#[must_use]
pub fn thread_async_threads() -> *mut c_void {
    with(|thread| thread.async_threads.get())
}

/// Sets `SwooleTG.async_threads`.
pub fn set_thread_async_threads(threads: *mut c_void) {
    with(|thread| thread.async_threads.set(threads));
}

/// `void swoole_init()`'s body: the core record's defaults, `SwooleG`'s
/// configuration, the native leaves (`NativeHost::init`: streams, logger, memory
/// pool, signalfd, bug report) and the main thread. Once per process; later calls
/// do nothing. crates/ffi's `swoole_init` registers the host, then calls this.
#[unsafe(no_mangle)]
pub extern "C-unwind" fn swr_v1_core_swoole_init() {
    if !crate::swr_v1_core_init() {
        return;
    }
    crate::config_state::reset();
    crate::swr_v1_core_name_resolver_clear();
    crate::config_state::init_allocator();
    if let Some(host) = native::host() {
        (host.init)();
    }
    swoole_thread_init(true);
    crate::config_state::init();
}

/// `void swoole_thread_init(bool main_thread)`.
#[cfg_attr(
    feature = "upstream-symbols",
    unsafe(export_name = "_Z18swoole_thread_initb")
)]
pub extern "C-unwind" fn swoole_thread_init(main_thread: bool) {
    if let Some(host) = native::host()
        && with(|thread| thread.buffer_stack.get().is_null())
    {
        let buffer = (host.new_buffer)();
        with(|thread| thread.buffer_stack.set(buffer));
    }
    if !main_thread {
        crate::signal_block_all();
    }
    with(|thread| thread.main_thread.set(main_thread));
}

/// `void swoole_thread_clean(bool main_thread)`.
#[cfg_attr(
    feature = "upstream-symbols",
    unsafe(export_name = "_Z19swoole_thread_cleanb")
)]
pub extern "C-unwind" fn swoole_thread_clean(_main_thread: bool) {
    let Some(host) = native::host() else { return };
    if with(|thread| !thread.timer.get().is_null()) {
        (host.timer_free)();
    }
    if with(|thread| !thread.reactor.get().is_null()) {
        (host.event_free)();
    }
    if with(|thread| !thread.buffer_stack.get().is_null()) {
        (host.delete_buffer)();
        with(|thread| thread.buffer_stack.set(ptr::null_mut()));
    }
}

/// `bool swoole_is_main_thread()`.
#[cfg_attr(
    feature = "upstream-symbols",
    unsafe(export_name = "_Z21swoole_is_main_threadv")
)]
#[must_use]
pub extern "C" fn swoole_is_main_thread() -> bool {
    with(|thread| thread.main_thread.get())
}

/// `void swoole_clean()`: the hooks, signals and this thread first, then the
/// logger and the memory pool, and `SwooleG` last.
#[cfg_attr(
    feature = "upstream-symbols",
    unsafe(export_name = "_Z12swoole_cleanv")
)]
pub extern "C-unwind" fn swoole_clean() {
    crate::hook::clear();
    if let Some(host) = native::host() {
        (host.signal_clear)();
        swoole_thread_clean(true);
        (host.logger)(native::LOGGER_CLOSE);
        (host.logger)(native::LOGGER_DELETE);
        (host.delete_pool)();
    }
    crate::config_state::reset();
    crate::swr_v1_core_name_resolver_clear();
    crate::swr_v1_core_reset();
}

/// `pid_t swoole_fork(int flags)`: fork outside coroutines and async threads,
/// retrying each second while fork fails; the child drops the parent's timer,
/// pool, reactor and signals, then runs the after-fork hooks. -1 before
/// `swoole_init`.
///
/// Signals stay blocked from just before the fork until the child has dropped the
/// parent's handlers. Delivered earlier, a signal would run the parent's handler,
/// which takes it as the parent's or ignores it (the manager's ignores a SIGTERM
/// to another pid), and a worker the manager is stopping would never stop.
#[cfg_attr(feature = "upstream-symbols", unsafe(export_name = "_Z11swoole_forki"))]
#[must_use]
pub extern "C-unwind" fn swoole_fork(flags: c_int) -> libc::pid_t {
    let Some(host) = native::host() else {
        return -1;
    };
    if flags & FORK_EXEC == 0 {
        let refuse = |message| {
            // SAFETY: the frames a native longjmp skips own nothing.
            unsafe { crate::fatal_error(OPERATION_NOT_SUPPORT, message) };
        };
        // `SwooleG.fatal_error` may return, so each check runs.
        if (host.coroutine_is_in)() {
            refuse(c"must be forked outside the coroutine");
        }
        if with(|thread| !thread.async_threads.get().is_null()) {
            refuse(c"can not fork after using async-threads");
        }
    }
    if flags & FORK_PRECHECK != 0 {
        return 0;
    }
    let previous = crate::signal_mask::block_for_fork();
    let pid = loop {
        // The child continues here.
        let pid = crate::sys::fork();
        if pid >= 0 {
            break pid;
        }
        log::sys_warning(c"swoole_fork", format_args!("fork() failed"));
        std::thread::sleep(std::time::Duration::from_secs(1));
    };
    if pid > 0 || flags & FORK_DAEMON != 0 {
        crate::signal_mask::restore_after_fork(previous, false);
        return pid;
    }
    if with(|thread| !thread.timer.get().is_null()) {
        (host.timer_free)();
    }
    if flags & FORK_EXEC == 0 {
        // The pool starts its own pages at its first use here (crates/memory's `current`).
        (host.logger)(native::LOGGER_REOPEN);
        if (host.event_available)() {
            (host.event_free)();
        }
    } else {
        (host.logger)(native::LOGGER_CLOSE);
    }
    (host.signal_clear)();
    crate::signal_mask::restore_after_fork(previous, true);
    if crate::swoole_isset_hook(HOOK_AFTER_FORK) {
        crate::hook::call_after_fork();
    }
    0
}

/// `swoole_fork_exec`: the child runs `child` and exits 0; the parent gets the
/// pid, and 0 when fork fails (upstream returns `false`).
pub fn fork_exec(child: impl FnOnce()) -> libc::pid_t {
    match crate::sys::fork() {
        pid if pid < 0 => 0,
        0 => {
            child();
            // The child ends here, as upstream's exit(0).
            std::process::exit(0)
        }
        pid => pid,
    }
}

/// `sw_tg_buffer()`: `SwooleTG.buffer_stack`.
#[must_use]
pub fn thread_buffer() -> *mut c_void {
    with(|thread| thread.buffer_stack.get())
}

thread_local! {
    /// Whether a [`lend_thread_buffer`] is running on this thread.
    static BUFFER_LENT: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
}

/// Runs `f` as the one Rust borrower of [`thread_buffer`] on this thread: a lend
/// inside `f` panics instead of aliasing the buffer. A thread-context coroutine's
/// thread shares the buffer, but no other runs while `f` does.
/// # Panics
/// Inside another lend's `f` on this thread.
pub fn lend_thread_buffer<R>(f: impl FnOnce() -> R) -> R {
    struct Lent;
    impl Drop for Lent {
        fn drop(&mut self) {
            BUFFER_LENT.set(false);
        }
    }
    assert!(!BUFFER_LENT.replace(true), "sw_tg_buffer() is lent already");
    let _lent = Lent;
    f()
}

/// The `SwooleTG` members a thread-context coroutine's thread shares with the
/// thread that activated coroutines: the buffer stack, reactor, timer and async
/// threads.
#[derive(Clone, Copy)]
pub struct SharedThread([*mut c_void; 4]);

// SAFETY: plain pointers, handed to threads that run one at a time.
unsafe impl Send for SharedThread {}

/// This thread's [`SharedThread`].
#[must_use]
pub fn shared_thread() -> SharedThread {
    SharedThread(with(|thread| {
        [
            thread.buffer_stack.get(),
            thread.reactor.get(),
            thread.timer.get(),
            thread.async_threads.get(),
        ]
    }))
}

/// Adopts `shared` in this new thread.
pub fn adopt_shared_thread(shared: SharedThread) {
    let [buffer_stack, reactor, timer, async_threads] = shared.0;
    with(|thread| {
        thread.buffer_stack.set(buffer_stack);
        thread.reactor.set(reactor);
        thread.timer.set(timer);
        thread.async_threads.set(async_threads);
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    extern "C" fn ignore(_signo: c_int) {}

    fn signal(signo: c_int, handler: libc::sighandler_t) {
        unsafe { libc::signal(signo, handler) };
    }

    fn blocked(signo: c_int) -> bool {
        let mut now = unsafe { std::mem::zeroed() };
        unsafe { libc::pthread_sigmask(libc::SIG_BLOCK, ptr::null(), &raw mut now) };
        unsafe { libc::sigismember(&raw const now, signo) == 1 }
    }

    fn change(how: c_int, signo: c_int) {
        let mut set = unsafe { std::mem::zeroed() };
        unsafe { libc::sigaddset(&raw mut set, signo) };
        unsafe { libc::pthread_sigmask(how, &raw const set, ptr::null_mut()) };
    }

    /// The reset unblocks SIGUSR1, as signalfd's does.
    extern "C" fn signal_clear() {
        signal(libc::SIGTERM, libc::SIG_DFL);
        change(libc::SIG_UNBLOCK, libc::SIGUSR1);
    }

    /// The child takes a while to reopen the logger, as on a busy machine.
    fn host() {
        native::set_native_host(native::NativeHost {
            init: || {},
            new_buffer: ptr::null_mut,
            delete_buffer: || {},
            delete_pool: || {},
            logger: |op| {
                if op == native::LOGGER_REOPEN {
                    std::thread::sleep(std::time::Duration::from_millis(200));
                }
            },
            signal_clear,
            timer_free: || {},
            event_available: || false,
            event_free: || {},
            coroutine_is_in: || false,
        });
    }

    fn wait(pid: libc::pid_t) -> c_int {
        assert!(pid > 0);
        let mut status = 0;
        assert_eq!(unsafe { libc::waitpid(pid, &raw mut status, 0) }, pid);
        status
    }

    /// A SIGTERM sent to a new child before it drops the parent's handlers reaches
    /// it once it has, instead of going to the parent's handler, which ignores it.
    #[test]
    #[cfg_attr(miri, ignore = "Miri has no signal")]
    fn fork_holds_signals_until_the_child_resets_handlers() {
        host();
        std::thread::spawn(|| {
            signal(libc::SIGTERM, ignore as *const () as libc::sighandler_t);
            let pid = swoole_fork(0);
            if pid == 0 {
                unsafe { libc::_exit(0) };
            }
            assert_eq!(unsafe { libc::kill(pid, libc::SIGTERM) }, 0);
            let status = wait(pid);
            signal(libc::SIGTERM, libc::SIG_DFL);
            assert!(
                libc::WIFSIGNALED(status) && libc::WTERMSIG(status) == libc::SIGTERM,
                "the child outlived SIGTERM: status {status:#x}"
            );
            assert!(!blocked(libc::SIGTERM), "the parent's mask is back");
        })
        .join()
        .unwrap();
    }

    /// The child keeps what the parent blocked, less what its reset unblocked.
    #[test]
    #[cfg_attr(miri, ignore = "Miri has no sigaddset")]
    fn fork_child_keeps_the_mask_its_reset_leaves() {
        host();
        std::thread::spawn(|| {
            change(libc::SIG_BLOCK, libc::SIGUSR1);
            change(libc::SIG_BLOCK, libc::SIGUSR2);
            let pid = swoole_fork(0);
            if pid == 0 {
                let wrong = i32::from(blocked(libc::SIGUSR1))
                    | (i32::from(!blocked(libc::SIGUSR2)) << 1)
                    | (i32::from(blocked(libc::SIGTERM)) << 2);
                unsafe { libc::_exit(wrong) };
            }
            let status = wait(pid);
            assert!(libc::WIFEXITED(status));
            assert_eq!(libc::WEXITSTATUS(status), 0, "the child's mask");
            assert!(
                blocked(libc::SIGUSR1) && blocked(libc::SIGUSR2),
                "the parent's"
            );
            assert!(!blocked(libc::SIGTERM));
        })
        .join()
        .unwrap();
    }
}
