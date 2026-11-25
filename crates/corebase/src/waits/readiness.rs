//! `System::init_reactor`'s readiness handlers for coroutine sockets
//! (`SW_FD_CO_SOCKET`), hooked polls (`SW_FD_CO_POLL`) and event waiters
//! (`SW_FD_CO_EVENT`), registered through `set`; a ready waiter's coroutine resumes
//! through [`wake`], deferred on the thread's reactor.
use crate::constants::FdType;
use crate::reactor::reactor_upstream::set_handlers;
use crate::reactor::{ErrorHandler, Event, Events, Handler, Running, UringOps};
use crate::waits::wait_event::{Parked, event_ready};
use core::ffi::{c_int, c_void};

use std::sync::OnceLock;

/// Handlers supplied by the hooks and clients crates at process initialization.
struct Init {
    socket: [Handler; 3],
    client: fn(Running<'_>),
}
static INIT_REACTOR: OnceLock<Init> = OnceLock::new();

/// Install the external handlers once, before creating reactors: each new reactor
/// gets them with the waits' own.
pub fn set_init_reactor(
    socket: [Handler; 3],
    uring: Option<&'static UringOps>,
    client: fn(Running<'_>),
) {
    INIT_REACTOR.get_or_init(|| Init { socket, client });
    crate::reactor::readiness::set_init_reactor(uring, init_reactor);
}

fn init_reactor(reactor: Running<'_>) {
    let Some(init) = INIT_REACTOR.get() else {
        return;
    };
    register(init.socket, |fd, read, write, error| {
        set_handlers(
            reactor,
            fd,
            Some(read),
            Some(write),
            ErrorHandler::Callback(error),
        );
    });
    set_handlers(
        reactor,
        FdType::SW_FD_AIO as i32,
        Some(crate::offload::threads::callback),
        None,
        ErrorHandler::ConnectionDefault,
    );
    if let Some(engine) = crate::reactor::readiness::uring() {
        set_handlers(
            reactor,
            FdType::SW_FD_IOURING as i32,
            Some(engine.ready),
            None,
            ErrorHandler::ConnectionDefault,
        );
    }
    (init.client)(reactor);
}

/// The fired socket's object and fd.
fn fired(event: &Event) -> (*mut c_void, c_int) {
    let object = event.socket().map_or(core::ptr::null_mut(), |socket| {
        socket.read(crate::reactor::SocketFields::attached_object)
    });
    (object, event.fd())
}

fn poll_ready<const KIND: i32>(_: Running<'_>, event: &mut Event) -> bool {
    let (task, fd) = fired(event);
    // SAFETY: a `SW_FD_CO_POLL` socket, whose object is its parked poll task.
    let task = unsafe { &*task.cast::<crate::waits::socket_poll::Task<'_>>() };
    crate::waits::socket_poll::ready(task, fd, Events::from_bits(KIND));
    true
}

fn waiter_ready<const KIND: i32>(_: Running<'_>, event: &mut Event) -> bool {
    let (waiter, _) = fired(event);
    // SAFETY: a `SW_FD_CO_EVENT` socket, whose object is its parked waiter.
    let waiter = unsafe { &*waiter.cast::<crate::waits::wait_event::Waiter>() };
    if let Some(co) = event_ready(waiter, Events::from_bits(KIND)) {
        wake(co);
    }
    true
}

/// Registers the coroutine socket (`Socket::init_reactor`), poll and event-waiter
/// handlers as complete read, write and error triples.
fn register(
    [read, write, error]: [Handler; 3],
    mut set: impl FnMut(c_int, Handler, Handler, Handler),
) {
    const READ: c_int = Events::READ.bits();
    const WRITE: c_int = Events::WRITE.bits();
    const ERROR: c_int = Events::ERROR.bits();
    let poll = FdType::SW_FD_CO_POLL as c_int;
    let waiter = FdType::SW_FD_CO_EVENT as c_int;
    let socket = FdType::SW_FD_CO_SOCKET as c_int;
    set(socket, read, write, error);
    set(
        poll,
        poll_ready::<READ>,
        poll_ready::<WRITE>,
        poll_ready::<ERROR>,
    );
    set(
        waiter,
        waiter_ready::<READ>,
        waiter_ready::<WRITE>,
        waiter_ready::<ERROR>,
    );
}

/// `sw_reactor()->defer([co] { co->resume(); })`, or the unit tests' recorder.
fn wake(co: Parked) {
    #[cfg(test)]
    if let Some(wake) = tests::WAKE {
        return wake(co);
    }
    // Upstream dereferences the thread's null reactor; abort instead.
    if crate::reactor::event_api::swoole_event_defer_value(co).is_err() {
        crate::fatal("no reactor to defer the resume to");
    }
}

#[cfg(test)]
#[expect(
    clippy::panic_in_result_fn,
    clippy::unnecessary_wraps,
    reason = "the fakes have EventWaitOps' signatures and assert what fired while parked"
)]
mod tests {
    use super::*;
    use crate::reactor::SocketRecord;
    use crate::waits::wait_event::{EventWaitOps, Registered, wait_with};
    use std::cell::{Cell, RefCell};

    const CO: *mut c_void = 0x10 as *mut c_void;

    thread_local! {
        static HANDLERS: RefCell<Vec<(c_int, Events, Handler)>> = const { RefCell::new(Vec::new()) };
        static SOCKET: Cell<*mut SocketRecord> = const { Cell::new(core::ptr::null_mut()) };
        static WOKEN: Cell<u32> = const { Cell::new(0) };
    }

    fn set(fd_type: c_int, read: Handler, write: Handler, error: Handler) {
        HANDLERS.with_borrow_mut(|handlers| {
            handlers.extend([
                (fd_type, Events::READ, read),
                (fd_type, Events::WRITE, write),
                (fd_type, Events::ERROR, error),
            ]);
        });
    }
    /// Records the wake in place of deferring it.
    pub(super) const WAKE: Option<fn(Parked)> = Some(wake);
    #[expect(
        clippy::needless_pass_by_value,
        reason = "the native wake's signature, which defers it"
    )]
    fn wake(co: Parked) {
        assert_eq!(co.waker(), crate::coroutine::Waker::NONE);
        WOKEN.set(WOKEN.get() + 1);
    }
    fn handler(fd_type: FdType, events: Events) -> Handler {
        HANDLERS.with_borrow(|handlers| {
            handlers
                .iter()
                .find(|(t, e, _)| *t == fd_type as c_int && *e == events)
                .unwrap()
                .2
        })
    }

    extern "C" fn current() -> *mut c_void {
        CO
    }
    /// The fake reactor's socket record, freed on drop.
    struct Socket(*mut SocketRecord);

    impl Drop for Socket {
        fn drop(&mut self) {
            drop(unsafe { Box::from_raw(self.0) });
        }
    }

    fn add(fd: i32, _: Events, waiter: Registered) -> Result<Socket, i32> {
        let socket = SocketRecord::with_fd(fd, FdType::SW_FD_CO_EVENT as c_int).into_raw();
        unsafe { (*socket).fields.attach_object(waiter.as_ptr()) };
        SOCKET.set(socket);
        Ok(Socket(socket))
    }
    /// The reactor fires write, then read, on the waiter's socket.
    fn park(_: f64, waker: &crate::offload::WakerSlot) -> Result<(), i32> {
        waker.set(Some(crate::coroutine::Waker::NONE));
        let mut event = unsafe {
            Event::of(
                7,
                0,
                FdType::SW_FD_CO_EVENT as c_int,
                crate::reactor::Socket::from_ptr(SOCKET.get())
                    .map(crate::reactor::Heap::from_static),
            )
        };
        for events in [Events::WRITE, Events::READ] {
            let ready = handler(FdType::SW_FD_CO_EVENT, events);
            assert!(unsafe { ready(Running::new(core::ptr::null_mut()), &mut event) });
        }
        Ok(())
    }

    #[test]
    fn registers_nine_handlers_and_wakes_a_waiter_once() {
        register([|_, _| true; 3], set);
        assert_eq!(HANDLERS.with_borrow(Vec::len), 9);
        for fd_type in [
            FdType::SW_FD_CO_SOCKET,
            FdType::SW_FD_CO_POLL,
            FdType::SW_FD_CO_EVENT,
        ] {
            for events in [Events::READ, Events::WRITE, Events::ERROR] {
                handler(fd_type, events);
            }
        }
        let ops = EventWaitOps { current, add, park };
        let waited = wait_with(7, Events::READ | Events::WRITE, 1.0, &ops);
        assert_eq!(
            (waited.unwrap_or(Events::NONE).bits(), WOKEN.get()),
            (Events::RDWR.bits(), 1)
        );
    }
}
