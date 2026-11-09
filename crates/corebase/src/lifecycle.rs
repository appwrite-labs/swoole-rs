// SPDX-License-Identifier: Apache-2.0
// Compatibility derived from Swoole v6.2.4 src/core/base.cc (Swoole contributors).
//! base.cc's init, thread, clean and fork sequences, and `SwooleTG`. Each C++
//! effect (an object's `new`/`delete`, the timer, event, signal and coroutine
//! leaves) runs through the host native/corebase/globals.cc registers
//! (`native.rs`); without it there is nothing native to set up or tear down.
use crate::log;
use crate::native;
use std::cell::UnsafeCell;
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
    id: u16,
    kind: u8,
    main_thread: bool,
    buffer_stack: *mut c_void,
    reactor: *mut c_void,
    timer: *mut c_void,
    message_bus: *mut c_void,
    async_threads: *mut c_void,
}

const _: () = assert!(
    std::mem::offset_of!(ThreadGlobal, main_thread) == 3
        && std::mem::offset_of!(ThreadGlobal, buffer_stack) == 8
        && std::mem::offset_of!(ThreadGlobal, async_threads) == 40
        && size_of::<ThreadGlobal>() == 48,
    "swoole_rs_corebase_state.h asserts the same `swoole::ThreadGlobal` layout"
);

thread_local! {
    static THREAD: UnsafeCell<ThreadGlobal> = const {
        UnsafeCell::new(ThreadGlobal {
            id: 0,
            kind: 0,
            main_thread: false,
            buffer_stack: ptr::null_mut(),
            reactor: ptr::null_mut(),
            timer: ptr::null_mut(),
            message_bus: ptr::null_mut(),
            async_threads: ptr::null_mut(),
        })
    };
}

/// This thread's `SwooleTG`, live for the thread's life (it has no destructor).
#[unsafe(no_mangle)]
pub extern "C" fn swr_v1_core_thread_global() -> *mut ThreadGlobal {
    THREAD.with(UnsafeCell::get)
}

/// `&SwooleTG.timer`, for crates/timer and crates/server.
#[unsafe(no_mangle)]
pub extern "C" fn swr_v1_core_timer_slot() -> *mut *mut c_void {
    // SAFETY: this thread's live record; only the field's address is taken.
    unsafe { &raw mut (*swr_v1_core_thread_global()).timer }
}

fn read<T: Copy>(field: impl FnOnce(&ThreadGlobal) -> T) -> T {
    // SAFETY: this thread's live record; the borrow ends before any native call.
    field(unsafe { &*swr_v1_core_thread_global() })
}

fn write(field: impl FnOnce(&mut ThreadGlobal)) {
    // SAFETY: as `read`.
    field(unsafe { &mut *swr_v1_core_thread_global() });
}

/// `SwooleTG.reactor`.
#[must_use]
pub fn thread_reactor() -> *mut c_void {
    read(|thread| thread.reactor)
}

/// `swoole_get_thread_type()`: `SwooleTG.type`.
#[must_use]
pub fn thread_kind() -> u8 {
    read(|thread| thread.kind)
}

/// `swoole_set_thread_type(kind)` then `swoole_set_thread_id(id)`.
pub fn set_thread(kind: u8, id: u16) {
    write(|thread| {
        thread.kind = kind;
        thread.id = id;
    });
}

/// `swoole_get_thread_id()`: `SwooleTG.id`.
#[must_use]
pub fn thread_id() -> u16 {
    read(|thread| thread.id)
}

/// `SwooleTG.message_bus`.
#[must_use]
pub fn thread_message_bus() -> *mut c_void {
    read(|thread| thread.message_bus)
}

/// Sets `SwooleTG.message_bus`.
pub fn set_thread_message_bus(bus: *mut c_void) {
    write(|thread| thread.message_bus = bus);
}

/// Sets `SwooleTG.reactor`.
pub fn set_thread_reactor(reactor: *mut c_void) {
    write(|thread| thread.reactor = reactor);
}

/// `SwooleTG.async_threads`.
#[must_use]
pub fn thread_async_threads() -> *mut c_void {
    read(|thread| thread.async_threads)
}

/// Sets `SwooleTG.async_threads`.
pub fn set_thread_async_threads(threads: *mut c_void) {
    write(|thread| thread.async_threads = threads);
}

/// `void swoole_init()`'s body: the core record's defaults, `SwooleG`'s
/// configuration, the native leaves (`NativeHost::init`: streams, logger, memory
/// pool, signalfd, bug report) and the main thread. Once per process; later calls
/// do nothing. globals.cc's `swoole_init` registers the host, then calls this:
/// registering at load would keep every C++ leaf linked in the native gates.
#[unsafe(no_mangle)]
pub extern "C-unwind" fn swr_v1_core_swoole_init() {
    if !crate::swr_v1_core_init() {
        return;
    }
    crate::config_state::reset();
    crate::swr_v1_core_name_resolver_clear();
    crate::config_state::init_allocator();
    if let Some(host) = native::host() {
        host.init();
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
        && read(|thread| thread.buffer_stack.is_null())
    {
        host.buffer(true);
    }
    if !main_thread {
        crate::signal_block_all();
    }
    write(|thread| thread.main_thread = main_thread);
}

/// `void swoole_thread_clean(bool main_thread)`.
#[cfg_attr(
    feature = "upstream-symbols",
    unsafe(export_name = "_Z19swoole_thread_cleanb")
)]
pub extern "C-unwind" fn swoole_thread_clean(_main_thread: bool) {
    let Some(host) = native::host() else { return };
    if read(|thread| !thread.timer.is_null()) {
        host.timer_free();
    }
    if read(|thread| !thread.reactor.is_null()) {
        host.event_free();
    }
    if read(|thread| !thread.buffer_stack.is_null()) {
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
    read(|thread| thread.main_thread)
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
        host.signal_clear();
        swoole_thread_clean(true);
        host.logger(native::LOGGER_CLOSE);
        host.logger(native::LOGGER_DELETE);
        host.delete_pool(crate::swr_v1_core_memory_pool());
    }
    crate::config_state::reset();
    crate::swr_v1_core_name_resolver_clear();
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
        if read(|thread| !thread.async_threads.is_null()) {
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
        host.logger(native::LOGGER_REOPEN);
        if host.event_available() {
            host.event_free();
        }
    } else {
        host.logger(native::LOGGER_CLOSE);
    }
    host.signal_clear();
    if crate::swoole_isset_hook(HOOK_AFTER_FORK) {
        // SAFETY: the after-fork hooks take no argument.
        unsafe { crate::swoole_call_hook(HOOK_AFTER_FORK, std::ptr::null_mut()) };
    }
    0
}

/// `swoole_fork_exec`: the child runs `child` and exits 0; the parent gets the
/// pid, and 0 when fork fails (upstream returns `false`).
pub fn fork_exec(child: impl FnOnce()) -> libc::pid_t {
    // SAFETY: fork has no memory preconditions.
    match unsafe { libc::fork() } {
        pid if pid < 0 => 0,
        0 => {
            child();
            // SAFETY: the child ends here, as upstream's exit(0).
            unsafe { libc::exit(0) }
        }
        pid => pid,
    }
}

/// `sw_tg_buffer()`: `SwooleTG.buffer_stack`.
#[must_use]
pub fn thread_buffer() -> *mut c_void {
    read(|thread| thread.buffer_stack)
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
    SharedThread(read(|thread| {
        [
            thread.buffer_stack,
            thread.reactor,
            thread.timer,
            thread.async_threads,
        ]
    }))
}

/// Adopts `shared` in this new thread.
pub fn adopt_shared_thread(shared: SharedThread) {
    let [buffer_stack, reactor, timer, async_threads] = shared.0;
    write(|thread| {
        thread.buffer_stack = buffer_stack;
        thread.reactor = reactor;
        thread.timer = timer;
        thread.async_threads = async_threads;
    });
}
