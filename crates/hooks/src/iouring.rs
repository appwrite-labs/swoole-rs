// SPDX-License-Identifier: Apache-2.0
// Compatibility derived from Swoole v6.2.4 src/coroutine/iouring.cc (Swoole contributors).
//! `Iouring`: the thread's engine ([`crate::uring`]) on its reactor, and the host
//! table the engine calls back through ([`HOST`]).
//! The instance is created on the first request, registers the ring fd, the exit
//! condition and the end-of-loop submit, and is freed with the reactor.
use crate::uring::{self, Config, Host, OpenError, Uring};
use std::cell::Cell;
use std::ffi::{c_int, c_void};
use std::rc::Rc;
use std::sync::atomic::AtomicU32;
use swoole_rs_corebase::constants::FdType;
use swoole_rs_corebase::log;
use swoole_rs_corebase::reactor::event_api::{swoole_event_is_available, swoole_event_is_running};
use swoole_rs_corebase::reactor::{Events, SocketRecord, UringOps};
use swoole_rs_corebase::tls::event_socket::EventSocket;

/// The thread's `Iouring`: the engine (none if the ring could not be created) and
/// its fd's record on the thread's reactor, whose destroy callback takes the instance.
struct Iouring {
    engine: Option<Rc<Uring>>,
    /// The ring fd's record, which this instance owns until `destroyed` frees it.
    socket: Option<EventSocket>,
}

thread_local! {
    /// `SwooleTG.iouring`. Taken out and put back around a look at it
    /// ([`with_instance`]), which runs no coroutine.
    static INSTANCE: Cell<Option<Iouring>> = const { Cell::new(None) };
}

/// `f` over the thread's instance, taken out meanwhile.
fn with_instance<R>(f: impl FnOnce(Option<&Iouring>) -> R) -> R {
    INSTANCE.with(|cell| {
        let instance = cell.take();
        let result = f(instance.as_ref());
        // `set` would call the drop glue for the `None` it replaces.
        if let Some(nested) = cell.replace(instance) {
            drop(nested);
        }
        result
    })
}

/// The engine's callbacks into the coroutine scheduler, the thread's instance and
/// the log.
pub(crate) static HOST: Host = Host {
    current: swoole_rs_corebase::coroutine::current_safe,
    instance,
    suspend,
    resume: swoole_rs_corebase::coroutine::Waker::wake,
    event_running: swoole_event_is_running,
    submit_failed,
    timeout_full,
};

/// `Iouring::get_instance()->engine`, creating the instance on first use.
fn instance() -> Option<Rc<Uring>> {
    if with_instance(|instance| instance.is_none()) {
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
fn suspend(co: *mut c_void, event: u64, arm: &mut dyn FnMut(swoole_rs_corebase::coroutine::Waker)) {
    let cancel = move || {
        if swoole_rs_corebase::coroutine::current().is_null() {
            uring::cancel_nowait(&HOST, event)
        } else {
            uring::cancel(&HOST, event)
        }
    };
    swoole_rs_corebase::coroutine::yield_with_cancel_armed(co, cancel, arm);
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
        Err(OpenError::Create) => {
            log::sys_error(
                origin,
                format_args!(
                    "Failed to initialize io_uring instance; grant the process permission to use io_uring \
                     (for Docker, use privileged mode), or rebuild Swoole without --enable-iouring"
                ),
            );
            return iouring;
        }
        Err(OpenError::Probe) => log::error(
            origin,
            format_args!("Failed to query the io_uring opcodes supported by the running kernel"),
        ),
        Err(OpenError::Futex) => log::error(
            origin,
            format_args!(
                "The running kernel does not support IORING_OP_FUTEX_WAIT and IORING_OP_FUTEX_WAKE"
            ),
        ),
        Err(OpenError::Ftruncate) => log::error(
            origin,
            format_args!("The running kernel does not support IORING_OP_FTRUNCATE"),
        ),
        Err(OpenError::Workers) => {
            log::sys_error(
                origin,
                format_args!("Failed to set the maximum of io_uring async workers"),
            );
            return iouring;
        }
        Ok(()) => {}
    }
    let Some(engine) = &iouring.engine else {
        return iouring;
    };
    let record = SocketRecord::with_fd(engine.fd(), FdType::SW_FD_IOURING as c_int);
    let socket = iouring.socket.insert(EventSocket::adopt(Box::new(record)));
    swoole_rs_corebase::reactor::swr_v1_event_set_exit_condition(
        swoole_rs_corebase::reactor::ExitCondition::Iouring,
        idle,
    );
    swoole_rs_corebase::reactor::event_api::on_destroy(destroyed);
    swoole_rs_corebase::reactor::swr_v1_event_set_end_callback(
        swoole_rs_corebase::reactor::Priority::IouringSubmit,
        submit,
    );
    if !socket.add(Events::READ) {
        log::sys_error(
            origin,
            format_args!("Failed to add io_uring ring fd to the event loop"),
        );
    }
    iouring
}

/// The exit condition: requests in flight or waiting keep the loop running.
fn idle(event_num: &mut usize) -> bool {
    if with_instance(|instance| instance.is_some())
        && with_engine(|engine| engine.task_num() == 0 && engine.waiting_num() == 0).unwrap_or(true)
    {
        *event_num -= 1;
    }
    true
}

/// The end-of-loop callback: submit what the loop queued.
fn submit() {
    with_engine(Uring::submit_queued);
}

/// The reactor's destroy callback: `delete SwooleTG.iouring`.
fn destroyed() {
    let Some(iouring) = INSTANCE.take() else {
        return;
    };
    // Off the reactor, keeping the fd, which the engine closes.
    if let Some(socket) = iouring.socket {
        socket.free(true);
    }
    // The engine goes with it, with nothing in flight as the loop has ended.
}

/// `Iouring::callback`: the ring fd is readable. The thread's instance owns the ring
/// fd's record, so the event's object isn't needed.
pub(crate) fn ready() -> bool {
    current().is_some_and(|engine| uring::wakeup(&engine))
}

/// The engine's operations `corebase::reactor` calls; it sits below this crate, so
/// `swoole_init` registers them (`stdio::swr_v1_hook_install`).
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
    uring::waitpid(&HOST, pid, status, options, timeout).unwrap_or(-1)
}

/// The thread's engine, or none before the first request (`SwooleTG.iouring`).
/// Resuming coroutines re-enters the instance, so callers hold their own handle.
fn current() -> Option<Rc<Uring>> {
    with_instance(|iouring| iouring?.engine.clone())
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

/// `Iouring::futex_wait` on `word`, in a coroutine: false with errno set when
/// it failed (upstream's nonzero).
#[must_use]
pub fn futex_wait(word: &AtomicU32) -> bool {
    uring::futex_wait(&HOST, word).is_ok_and(|count| count == 0)
}

/// `Iouring::futex_wakeup` on `word`, in a coroutine: false with errno set when
/// it failed (upstream's negative).
#[must_use]
pub fn futex_wakeup(word: &AtomicU32) -> bool {
    uring::futex_wakeup(&HOST, word).is_ok()
}
