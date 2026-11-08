// SPDX-License-Identifier: Apache-2.0
// Compatibility derived from Swoole v6.2.4 src/coroutine/iouring.cc (Swoole contributors).
//! `Iouring`: the thread's engine ([`crate::uring`]) on its reactor, and the host
//! table the engine calls back through ([`HOST`], `swr_v1_uring_native` to native).
//! The instance is created on the first request, registers the ring fd, the exit
//! condition and the end-of-loop submit, and is freed with the reactor.
use crate::uring::{
    Config, Host, Uring, swr_v1_uring_cancel, swr_v1_uring_close, swr_v1_uring_fd,
    swr_v1_uring_open, swr_v1_uring_submit, swr_v1_uring_task_num, swr_v1_uring_waiting_num,
    swr_v1_uring_wakeup,
};
use std::cell::Cell;
use std::ffi::{c_int, c_void};
use std::ptr;
use swoole_rs_corebase::constants::FdType;
use swoole_rs_corebase::log;
use swoole_rs_reactor::event_api::{
    swoole_event_is_available, swoole_event_is_running, swr_v1_event_current,
};
use swoole_rs_reactor::reactor_upstream::{prepared, reactor_change};
use swoole_rs_reactor::{SocketRecord, swr_v1_reactor_deferred_push, swr_v1_socket_make};

/// `SW_EVENT_READ`, `Reactor::EXIT_CONDITION_IOURING` and `Reactor::PRIORITY_IOURING_SUBMIT`
const EVENT_READ: c_int = 1 << 9;
const EXIT_CONDITION_IOURING: c_int = 9;
const PRIORITY_IOURING_SUBMIT: c_int = 7;

// SAFETY: src/coroutine/base.cc's, native/phpcoroutines/coroutine.cc's and
// native/reactor/reactor.cc's.
unsafe extern "C-unwind" {
    fn swr_v1_coroutine_native_resume(co: *mut c_void);
}
// SAFETY: as above.
unsafe extern "C" {
    fn swr_cpp_phpcoroutines_yield_cancelable(
        co: *mut c_void,
        cancel: unsafe extern "C" fn(*mut c_void, *mut c_void) -> u8,
        data: *mut c_void,
    );
    fn swr_v1_event_set_exit_condition(
        id: c_int,
        condition: unsafe extern "C" fn(*mut usize) -> bool,
    );
    fn swr_v1_event_set_end_callback(id: c_int, callback: unsafe extern "C" fn());
}

/// The thread's `Iouring`: the engine (null if the ring could not be created) and
/// its fd's record on `reactor`.
struct Iouring {
    engine: *mut Uring,
    socket: *mut SocketRecord,
    reactor: *mut c_void,
}

thread_local! {
    /// `SwooleTG.iouring`.
    static INSTANCE: Cell<*mut Iouring> = const { Cell::new(ptr::null_mut()) };
}

/// The engine's callbacks into the coroutine scheduler, the thread's instance and
/// the log.
#[unsafe(export_name = "swr_v1_uring_native")]
pub static HOST: Host = Host {
    current: crate::wait_event::native::current,
    instance,
    suspend,
    resume,
    event_running,
    submit_failed,
    timeout_full,
};

/// `Iouring::get_instance()->engine`, creating the instance on first use.
unsafe extern "C" fn instance() -> *mut Uring {
    let mut iouring = INSTANCE.get();
    if iouring.is_null() {
        if !swoole_event_is_available() {
            log::error(
                c"Iouring::get_instance",
                format_args!("The event loop is unavailable, unable to create io_uring instance."),
            );
        }
        iouring = create(swr_v1_event_current());
        INSTANCE.set(iouring);
    }
    // SAFETY: the thread's live instance.
    unsafe { (*iouring).engine }
}

/// `co->yield(&cancel_fn)`, where cancelling cancels `event`'s request.
unsafe extern "C" fn suspend(co: *mut c_void, event: *mut c_void) {
    unsafe extern "C" fn cancel(_co: *mut c_void, event: *mut c_void) -> u8 {
        // SAFETY: the event stays suspended in its operation while its coroutine is.
        u8::from(unsafe { swr_v1_uring_cancel(&raw const HOST, event) })
    }
    // SAFETY: the current coroutine, suspended in `event`'s operation.
    unsafe { swr_cpp_phpcoroutines_yield_cancelable(co, cancel, event) };
}

unsafe extern "C" fn resume(co: *mut c_void) {
    // SAFETY: a coroutine suspended in a request.
    unsafe { swr_v1_coroutine_native_resume(co) };
}

unsafe extern "C" fn event_running() -> bool {
    swoole_event_is_running()
}

unsafe extern "C" fn submit_failed() {
    log::sys_error(c"Iouring::submit", format_args!("io_uring_submit() failed"));
}

unsafe extern "C" fn timeout_full(ring_fd: c_int) {
    log::warning(
        c"Iouring::dispatch",
        format_args!("timeout setting failed, the iouring queue[{ring_fd}] is full"),
    );
}

/// `new Iouring(reactor)`: open the engine, logging why it failed, then register
/// the ring fd and the reactor callbacks.
fn create(reactor: *mut c_void) -> *mut Iouring {
    let host = crate::stdio::host();
    let config = Config {
        entries: swoole_rs_corebase::swr_v1_core_iouring_entries(),
        flag: swoole_rs_corebase::swr_v1_core_iouring_flag(),
        workers: swoole_rs_corebase::swr_v1_core_iouring_workers(),
        probe_futex: host.is_some_and(|host| host.uring_futex),
        probe_ftruncate: host.is_some_and(|host| host.uring_ftruncate),
    };
    // SAFETY: HOST is static.
    let opened = unsafe { swr_v1_uring_open(config, &raw const HOST) };
    let iouring = Box::into_raw(Box::new(Iouring {
        engine: opened.uring,
        socket: ptr::null_mut(),
        reactor,
    }));
    let origin = c"Iouring::Iouring";
    match opened.status {
        1 => log::error(
            origin,
            format_args!(
                "unsupported liburing version: Swoole requires liburing {}.{} or later with ABI major {}, but \
                 liburing {}.{} was loaded at runtime",
                opened.major,
                opened.minor,
                opened.major,
                opened.runtime_major,
                opened.runtime_minor
            ),
        ),
        2 => {
            log::sys_error(
                origin,
                format_args!(
                    "Failed to initialize io_uring instance; grant the process permission to use io_uring \
                     (for Docker, use privileged mode), or rebuild Swoole without --enable-iouring"
                ),
            );
            return iouring;
        }
        3 => log::error(
            origin,
            format_args!("Failed to query the io_uring opcodes supported by the running kernel"),
        ),
        4 => log::error(
            origin,
            format_args!(
                "The running kernel does not support IORING_OP_FUTEX_WAIT and IORING_OP_FUTEX_WAKE"
            ),
        ),
        5 => log::error(
            origin,
            format_args!("The running kernel does not support IORING_OP_FTRUNCATE"),
        ),
        6 => {
            log::sys_error(
                origin,
                format_args!("Failed to set the maximum of io_uring async workers"),
            );
            return iouring;
        }
        _ => {}
    }
    // SAFETY: the engine opened.
    let fd = unsafe { swr_v1_uring_fd(opened.uring) };
    let socket = swr_v1_socket_make(fd, FdType::SW_FD_IOURING as c_int);
    // SAFETY: the new record.
    unsafe { (*socket).object = iouring.cast() };
    // SAFETY: the new instance, not yet shared.
    unsafe { (*iouring).socket = socket };
    // SAFETY: the thread's live reactor.
    unsafe { swr_v1_event_set_exit_condition(EXIT_CONDITION_IOURING, idle) };
    // SAFETY: as above.
    let fields = unsafe { prepared(reactor) }.fields;
    // SAFETY: its live fields.
    let queue = unsafe { &raw mut (*fields).destroy_callbacks };
    // SAFETY: the reactor's live destroy queue; the call keeps no data.
    if unsafe { swr_v1_reactor_deferred_push(queue, destroyed, ptr::null_mut(), None) } != 0 {
        std::alloc::handle_alloc_error(std::alloc::Layout::new::<usize>());
    }
    // SAFETY: the thread's live reactor.
    unsafe { swr_v1_event_set_end_callback(PRIORITY_IOURING_SUBMIT, submit) };
    // SAFETY: the record and its live reactor.
    if unsafe { reactor_change(reactor, socket, 0, EVENT_READ) } < 0 {
        log::sys_error(
            origin,
            format_args!("Failed to add io_uring ring fd to the event loop"),
        );
    }
    iouring
}

/// The exit condition: requests in flight or waiting keep the loop running.
unsafe extern "C" fn idle(event_num: *mut usize) -> bool {
    let iouring = INSTANCE.get();
    // SAFETY: null or the thread's live instance.
    if let Some(iouring) = unsafe { iouring.as_ref() }
        // SAFETY: the instance's engine, live or null.
        && unsafe { swr_v1_uring_task_num(iouring.engine) } == 0
        // SAFETY: as above.
        && unsafe { swr_v1_uring_waiting_num(iouring.engine) } == 0
    {
        // SAFETY: the reactor passes its live count.
        unsafe { *event_num -= 1 };
    }
    true
}

/// The end-of-loop callback: submit what the loop queued.
unsafe extern "C" fn submit() {
    let engine = swr_v1_uring_current();
    if !engine.is_null() {
        // SAFETY: the thread's live engine.
        unsafe { swr_v1_uring_submit(engine) };
    }
}

/// The reactor's destroy callback: `delete SwooleTG.iouring`.
unsafe extern "C-unwind" fn destroyed(_data: *mut c_void) {
    let iouring = INSTANCE.replace(ptr::null_mut());
    if iouring.is_null() {
        return;
    }
    // SAFETY: `create` boxed it; nothing refers to it any more.
    let iouring = unsafe { Box::from_raw(iouring) };
    let socket = iouring.socket;
    // SAFETY: the record `create` registered on the closing reactor.
    if unsafe { (*socket).removed } == 0 {
        // SAFETY: as above.
        unsafe { reactor_change(iouring.reactor, socket, 2, 0) };
    }
    // SAFETY: the record, whose fd the engine closes.
    unsafe { (*socket).fd = -1 };
    // SAFETY: as above; nothing uses it afterwards.
    unsafe { swoole_rs_tls::upstream::socket_free(socket) };
    // SAFETY: the engine, with nothing in flight as the loop has ended.
    unsafe { swr_v1_uring_close(iouring.engine) };
}

/// `Iouring::callback`: the ring fd is readable; `object` is its record's.
///
/// # Safety
/// `object` is the ring fd record's object, a live instance.
pub(crate) unsafe fn ready(object: *mut c_void) -> c_int {
    // SAFETY: as the caller promises.
    let engine = unsafe { (*object.cast::<Iouring>()).engine };
    // SAFETY: the instance's live engine, from the event loop.
    if unsafe { swr_v1_uring_wakeup(engine) } {
        0
    } else {
        -1
    }
}

/// The epoll wait was interrupted (EINTR) on a thread with an instance.
#[unsafe(no_mangle)]
pub extern "C" fn swr_v1_uring_interrupted(_reactor: *mut c_void) -> c_int {
    let eintr = std::io::Error::last_os_error().raw_os_error() == Some(libc::EINTR);
    c_int::from(eintr && !swr_v1_uring_current().is_null())
}

/// `reactor->iouring_interrupt_handler`: harvest the completions.
#[unsafe(no_mangle)]
pub extern "C" fn swr_v1_uring_interrupt(_reactor: *mut c_void) {
    let engine = swr_v1_uring_current();
    if !engine.is_null() {
        // SAFETY: the thread's live engine, from the event loop.
        unsafe { swr_v1_uring_wakeup(engine) };
    }
}

/// The thread's engine, or null before the first request (`SwooleTG.iouring`).
#[unsafe(no_mangle)]
pub extern "C" fn swr_v1_uring_current() -> *mut Uring {
    // SAFETY: null or the thread's live instance.
    unsafe { INSTANCE.get().as_ref() }.map_or(ptr::null_mut(), |iouring| iouring.engine)
}
