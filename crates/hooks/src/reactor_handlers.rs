//! `System::init_reactor`'s readiness handlers for coroutine sockets
//! (`SW_FD_CO_SOCKET`), hooked polls (`SW_FD_CO_POLL`) and event waiters
//! (`SW_FD_CO_EVENT`). Native code registers them through `set`
//! and lends `wake`, which defers a parked coroutine's resume.
use crate::poll::swr_v1_hook_poll_ready;
use crate::socket_members;
use crate::wait_event::{ERROR, READ, WRITE, event_ready};
use core::ffi::{c_int, c_void};
use std::sync::OnceLock;
use swoole_rs_corebase::constants::FdType;
use swoole_rs_reactor::{Event, Handler, Running, SocketRecord};

/// `Reactor::set_handler(fd_type, events, handler)`.
pub type SetHandler = unsafe extern "C" fn(*mut c_void, c_int, c_int, Handler);
/// `reactor->defer(co->resume())`.
pub type Wake = unsafe extern "C" fn(*mut c_void);

/// Native's `wake`, the same function for every reactor.
static WAKE: OnceLock<Wake> = OnceLock::new();

/// The fired socket's object and fd.
fn fired(event: &Event) -> (*mut c_void, c_int) {
    // SAFETY: the reactor's event names a live socket.
    let object = unsafe { (*event.socket.cast::<SocketRecord>()).object };
    (object, event.fd)
}

extern "C" fn poll_ready<const KIND: i32>(_: Running<'_>, event: &mut Event) -> c_int {
    let (task, fd) = fired(event);
    // SAFETY: a `SW_FD_CO_POLL` socket, whose object is its poll task.
    unsafe { swr_v1_hook_poll_ready(task, fd, KIND) };
    0
}

extern "C" fn waiter_ready<const KIND: i32>(_: Running<'_>, event: &mut Event) -> c_int {
    let (waiter, _) = fired(event);
    // SAFETY: a `SW_FD_CO_EVENT` socket, whose object is its parked waiter.
    let co = unsafe { event_ready(waiter, KIND) };
    if let Some(wake) = WAKE.get().filter(|_| !co.is_null()) {
        // SAFETY: `co` stays parked until the deferred resume runs.
        unsafe { wake(co) };
    }
    0
}

/// Registers the coroutine socket (`Socket::init_reactor`), poll and event-waiter
/// handlers on `reactor`.
///
/// # Safety
/// `set` registers on `reactor`; `wake` defers a resume on the calling thread's reactor.
pub unsafe extern "C" fn swr_v1_hook_init_reactor(
    reactor: *mut c_void,
    set: SetHandler,
    wake: Wake,
) {
    WAKE.get_or_init(|| wake);
    let poll = FdType::SW_FD_CO_POLL as c_int;
    let waiter = FdType::SW_FD_CO_EVENT as c_int;
    let socket = FdType::SW_FD_CO_SOCKET as c_int;
    let handlers: [(c_int, c_int, Handler); 9] = [
        (socket, READ, socket_members::readable_event_callback),
        (socket, WRITE, socket_members::writable_event_callback),
        (socket, ERROR, socket_members::error_event_callback),
        (poll, READ, poll_ready::<READ>),
        (poll, WRITE, poll_ready::<WRITE>),
        (poll, ERROR, poll_ready::<ERROR>),
        (waiter, READ, waiter_ready::<READ>),
        (waiter, WRITE, waiter_ready::<WRITE>),
        (waiter, ERROR, waiter_ready::<ERROR>),
    ];
    for (fd_type, events, handler) in handlers {
        // SAFETY: the caller's registrar for `reactor`.
        unsafe { set(reactor, fd_type, events, handler) };
    }
}

/// `static void System::init_reactor(Reactor *reactor)`: the handlers above, waking
/// coroutines through the thread's deferred queue, then the async-thread pipe's (and
/// `io_uring`'s) completions.
///
/// # Safety
/// `reactor` is the calling thread's live `Reactor`.
#[cfg(feature = "upstream-symbols")]
#[unsafe(export_name = "_ZN6swoole9coroutine6System12init_reactorEPNS_7ReactorE")]
pub unsafe extern "C" fn system_init_reactor(reactor: *mut c_void) {
    // SAFETY: the caller's reactor; `defer_resume` defers onto this thread's.
    unsafe { swr_v1_hook_init_reactor(reactor, set_handler, defer_resume) };
    // SAFETY: as above.
    unsafe {
        set_handler(
            reactor,
            FdType::SW_FD_AIO as c_int,
            READ,
            crate::async_threads::callback,
        );
    };
    #[cfg(feature = "iouring")]
    // SAFETY: as above.
    unsafe {
        set_handler(reactor, FdType::SW_FD_IOURING as c_int, READ, iouring_ready);
    }
}

/// `Iouring::callback`.
#[cfg(all(feature = "upstream-symbols", feature = "iouring"))]
extern "C" fn iouring_ready(_: Running<'_>, event: &mut Event) -> c_int {
    let (iouring, _) = fired(event);
    // SAFETY: a `SW_FD_IOURING` socket, whose object is its instance.
    unsafe { crate::iouring::ready(iouring) }
}

/// `Reactor::set_handler`.
#[cfg(feature = "upstream-symbols")]
unsafe extern "C" fn set_handler(
    reactor: *mut c_void,
    fd_type: c_int,
    events: c_int,
    handler: Handler,
) {
    // SAFETY: the caller's live reactor.
    unsafe {
        swoole_rs_reactor::reactor_upstream::reactor_set_handler(
            reactor,
            fd_type,
            events,
            Some(handler),
        );
    }
}

/// `sw_reactor()->defer([co] { co->resume(); })`.
#[cfg(feature = "upstream-symbols")]
unsafe extern "C" fn defer_resume(co: *mut c_void) {
    // SAFETY: the coroutine stays parked until this deferred resume runs.
    unsafe { defer(swoole_rs_coroutine::resume, co, None) };
}

/// `sw_reactor()->defer(call, data)` on the calling thread's loop; `dispose(data)`
/// instead if the loop goes first.
///
/// # Safety
/// The thread's reactor loop is running; `call(data)` is sound when it runs.
#[cfg(any(feature = "upstream-symbols", not(test)))]
pub(crate) unsafe fn defer(
    call: swoole_rs_reactor::DeferCall,
    data: *mut c_void,
    dispose: Option<swoole_rs_reactor::DeferDispose>,
) {
    let reactor = swoole_rs_corebase::thread_reactor().cast::<swoole_rs_reactor::ReactorPrepared>();
    // SAFETY: the thread's loop is live; it begins with its prepared state.
    let fields = unsafe { (*reactor).fields };
    // SAFETY: as above.
    let queue = unsafe { &raw mut (*fields).defer_tasks };
    // SAFETY: the loop's live deferred queue; the caller vouches for `call(data)`.
    if unsafe { swoole_rs_reactor::swr_v1_reactor_deferred_push(queue, call, data, dispose) } != 0 {
        // Upstream throws std::bad_alloc, which no frame here could catch.
        std::process::abort();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::wait_event::{EventWaitOps, swr_v1_hook_wait_event};
    use std::cell::{Cell, RefCell};
    use swoole_rs_reactor::SocketRecord;

    const CO: *mut c_void = 0x10 as *mut c_void;

    thread_local! {
        static HANDLERS: RefCell<Vec<(c_int, c_int, Handler)>> = const { RefCell::new(Vec::new()) };
        static SOCKET: Cell<*mut SocketRecord> = const { Cell::new(core::ptr::null_mut()) };
        static WOKEN: Cell<u32> = const { Cell::new(0) };
    }

    unsafe extern "C" fn set(_: *mut c_void, fd_type: c_int, events: c_int, handler: Handler) {
        HANDLERS.with_borrow_mut(|handlers| handlers.push((fd_type, events, handler)));
    }
    unsafe extern "C" fn wake(co: *mut c_void) {
        assert_eq!(co, CO);
        WOKEN.set(WOKEN.get() + 1);
    }
    fn handler(fd_type: FdType, events: c_int) -> Handler {
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
    unsafe extern "C" fn add(fd: i32, _: i32, waiter: *mut c_void, _: *mut i32) -> *mut c_void {
        let socket = SocketRecord::with_fd(fd, FdType::SW_FD_CO_EVENT as c_int).into_raw();
        unsafe { (*socket).object = waiter };
        SOCKET.set(socket);
        socket.cast()
    }
    unsafe extern "C" fn remove(socket: *mut c_void) {
        unsafe { SocketRecord::free(socket.cast()) };
    }
    /// The reactor fires write, then read, on the waiter's socket.
    unsafe extern "C" fn park(_: *mut c_void, _: f64) -> i32 {
        let mut event = Event {
            fd: 7,
            reactor_id: 0,
            fd_type: FdType::SW_FD_CO_EVENT as c_int,
            socket: SOCKET.get().cast(),
        };
        for events in [WRITE, READ] {
            let ready = handler(FdType::SW_FD_CO_EVENT, events);
            assert_eq!(
                unsafe { ready(Running::new(core::ptr::null_mut()), &mut event) },
                0
            );
        }
        0
    }

    #[test]
    fn registers_nine_handlers_and_wakes_a_waiter_once() {
        unsafe { swr_v1_hook_init_reactor(core::ptr::null_mut(), set, wake) };
        assert_eq!(HANDLERS.with_borrow(Vec::len), 9);
        for fd_type in [
            FdType::SW_FD_CO_SOCKET,
            FdType::SW_FD_CO_POLL,
            FdType::SW_FD_CO_EVENT,
        ] {
            for events in [READ, WRITE, ERROR] {
                handler(fd_type, events);
            }
        }
        let ops = EventWaitOps {
            current,
            add,
            park,
            remove,
        };
        let waited = unsafe { swr_v1_hook_wait_event(7, READ | WRITE, 1.0, &raw const ops) };
        assert_eq!((waited.result, WOKEN.get()), (READ | WRITE, 1));
    }
}
