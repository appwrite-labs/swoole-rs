// SPDX-License-Identifier: Apache-2.0
// Compatibility derived from Swoole v6.2.4 src/coroutine/iouring.cc (Swoole contributors).
//! `Iouring`: the thread's engine ([`crate::uring`]) on its reactor, and the host
//! table the engine calls back through ([`HOST`]).
//! The instance is created on the first request, registers the ring fd, the exit
//! condition and the end-of-loop submit, and is freed with the reactor.
use crate::uring::{self, Config, Host, Uring};
use std::cell::RefCell;
use std::ffi::{c_int, c_void};
use std::ptr;
use std::rc::Rc;
use std::sync::atomic::AtomicU32;
use swoole_rs_corebase::constants::FdType;
use swoole_rs_corebase::log;
use swoole_rs_reactor::event_api::{
    event_add, event_del, swoole_event_is_available, swoole_event_is_running,
};
use swoole_rs_reactor::{Socket, SocketRecord, UringOps};

/// `SW_EVENT_READ`.
const EVENT_READ: c_int = swoole_rs_corebase::constants::EventType::SW_EVENT_READ as c_int;

/// The thread's `Iouring`: the engine (none if the ring could not be created) and
/// its fd's record on the thread's reactor, whose destroy callback takes the instance.
struct Iouring {
    engine: Option<Rc<Uring>>,
    /// The ring fd's record, which this instance owns until `destroyed` frees it.
    socket: Option<Socket<'static>>,
}

thread_local! {
    /// `SwooleTG.iouring`.
    static INSTANCE: RefCell<Option<Iouring>> = const { RefCell::new(None) };
}

/// The engine's callbacks into the coroutine scheduler, the thread's instance and
/// the log.
pub(crate) static HOST: Host = Host {
    current: crate::wait_event::native::current,
    instance,
    suspend,
    resume: swoole_rs_coroutine::resume,
    event_running: swoole_event_is_running,
    submit_failed,
    timeout_full,
};

/// `Iouring::get_instance()->engine`, creating the instance on first use.
fn instance() -> Option<Rc<Uring>> {
    if INSTANCE.with_borrow(Option::is_none) {
        if !swoole_event_is_available() {
            log::error(
                c"Iouring::get_instance",
                format_args!("The event loop is unavailable, unable to create io_uring instance."),
            );
        }
        let iouring = create();
        INSTANCE.set(Some(iouring));
    }
    current()
}

/// `co->yield(&cancel_fn)` of the running coroutine `co`, where cancelling cancels
/// `event`'s request.
fn suspend(co: *mut c_void, event: *mut c_void) {
    swoole_rs_coroutine::yield_with_cancel(co, move |_| uring::cancel(&HOST, event));
}

fn submit_failed() {
    log::sys_error(c"Iouring::submit", format_args!("io_uring_submit() failed"));
}

fn timeout_full(ring_fd: c_int) {
    log::warning(
        c"Iouring::dispatch",
        format_args!("timeout setting failed, the iouring queue[{ring_fd}] is full"),
    );
}

/// `new Iouring(reactor)` on the thread's reactor: open the engine, logging why it
/// failed, then register the ring fd and the reactor callbacks.
fn create() -> Iouring {
    let host = crate::stdio::host();
    let config = Config {
        entries: swoole_rs_corebase::swr_v1_core_iouring_entries(),
        flag: swoole_rs_corebase::swr_v1_core_iouring_flag(),
        workers: swoole_rs_corebase::swr_v1_core_iouring_workers(),
        probe_futex: host.is_some_and(|host| host.uring_futex),
        probe_ftruncate: host.is_some_and(|host| host.uring_ftruncate),
    };
    let opened = uring::open(config, &HOST);
    let mut iouring = Iouring {
        engine: opened.uring,
        socket: None,
    };
    let origin = c"Iouring::Iouring";
    match opened.status {
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
    let Some(engine) = &iouring.engine else {
        return iouring;
    };
    let record = SocketRecord::with_fd(engine.fd(), FdType::SW_FD_IOURING as c_int);
    let socket = Socket::new(Box::leak(Box::new(record)));
    iouring.socket = Some(socket);
    swoole_rs_reactor::swr_v1_event_set_exit_condition(
        swoole_rs_reactor::ExitCondition::Iouring,
        idle,
    );
    swoole_rs_reactor::event_api::on_destroy(destroyed);
    swoole_rs_reactor::swr_v1_event_set_end_callback(
        swoole_rs_reactor::Priority::IouringSubmit,
        submit,
    );
    if !event_add(socket, EVENT_READ) {
        log::sys_error(
            origin,
            format_args!("Failed to add io_uring ring fd to the event loop"),
        );
    }
    iouring
}

/// The exit condition: requests in flight or waiting keep the loop running.
extern "C-unwind" fn idle(event_num: &mut usize) -> bool {
    if INSTANCE.with_borrow(Option::is_some)
        && with_engine(|engine| engine.task_num() == 0 && engine.waiting_num() == 0).unwrap_or(true)
    {
        *event_num -= 1;
    }
    true
}

/// The end-of-loop callback: submit what the loop queued.
extern "C-unwind" fn submit() {
    with_engine(Uring::submit_queued);
}

/// The reactor's destroy callback: `delete SwooleTG.iouring`.
extern "C-unwind" fn destroyed(_data: *mut c_void) {
    let Some(iouring) = INSTANCE.take() else {
        return;
    };
    let socket = iouring.socket.map_or(ptr::null_mut(), |record| {
        if !record.read(|record| record.removed) {
            _ = event_del(record);
        }
        // The engine closes the fd.
        record.edit(|record| record.fd = -1);
        record.as_ptr()
    });
    // SAFETY: null or the record `create` leaked, whose only handle went with the
    // instance; nothing uses it afterwards.
    unsafe { swoole_rs_tls::upstream::socket_free(socket) };
    // The engine goes with it, with nothing in flight as the loop has ended.
}

/// `Iouring::callback`: the ring fd is readable. The thread's instance owns the ring
/// fd's record, so the event's object isn't needed.
pub(crate) fn ready() -> c_int {
    let woken = current().is_some_and(|engine| uring::wakeup(&engine));
    if woken { 0 } else { -1 }
}

/// The engine's operations crates/reactor calls; it sits below this crate, so
/// `swoole_init` registers them (`stdio::swr_v1_hook_install`).
#[cfg_attr(
    test,
    expect(
        dead_code,
        reason = "swr_v1_hook_install registers it, and test builds leave that out"
    )
)]
pub(crate) static REACTOR: UringOps = UringOps {
    interrupted,
    interrupt,
    waitpid,
};

/// The epoll wait was interrupted (EINTR) on a thread with an instance.
fn interrupted() -> bool {
    let eintr = std::io::Error::last_os_error().raw_os_error() == Some(libc::EINTR);
    eintr && current().is_some()
}

/// `reactor->iouring_interrupt_handler`: harvest the completions.
fn interrupt() {
    if let Some(engine) = current() {
        uring::wakeup(&engine);
    }
}

/// `Iouring::waitpid`.
fn waitpid(
    pid: libc::pid_t,
    status: Option<&mut c_int>,
    options: c_int,
    timeout: f64,
) -> libc::pid_t {
    uring::waitpid(&HOST, pid, status, options, timeout)
}

/// The thread's engine, or none before the first request (`SwooleTG.iouring`).
/// Resuming coroutines re-enters the instance, so callers hold their own handle.
fn current() -> Option<Rc<Uring>> {
    INSTANCE.with_borrow(|iouring| iouring.as_ref()?.engine.clone())
}

/// `f` over the thread's engine once it has one.
fn with_engine<R>(f: impl FnOnce(&Uring) -> R) -> Option<R> {
    current().map(|engine| f(&engine))
}

/// `Co::stats()`'s `io_uring` counts once the thread has an engine: requests in
/// flight, requests waiting for room in the ring, and the submission queue's use
/// in percent.
#[must_use]
pub fn stats() -> Option<(u32, usize, f32)> {
    with_engine(|engine| {
        (
            engine.task_num(),
            engine.waiting_num(),
            engine.sq_usage_percent(),
        )
    })
}

/// `Iouring::futex_wait` on `word`, in a coroutine.
#[must_use]
pub fn futex_wait(word: &AtomicU32) -> c_int {
    uring::futex_wait(&HOST, word)
}

/// `Iouring::futex_wakeup` on `word`, in a coroutine.
#[must_use]
pub fn futex_wakeup(word: &AtomicU32) -> c_int {
    uring::futex_wakeup(&HOST, word)
}
