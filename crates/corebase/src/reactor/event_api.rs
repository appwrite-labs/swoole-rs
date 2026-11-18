// SPDX-License-Identifier: Apache-2.0
// Compatibility derived from Swoole v6.2.4 src/wrapper/event.cc (Swoole contributors).
//! event.cc's operations over the thread's `Reactor` (`SwooleTG.reactor`), called
//! directly by Rust consumers and original fixtures.
//! `swoole_event_init` and `swoole_event_free` are `reactor_state.rs`'s, with the
//! `Reactor`'s constructor and destructor; deferring is deferred.rs's.
use crate::reactor::handlers::Handler;
use crate::reactor::{Events, ReactorFields};
use std::ffi::{c_int, c_void};
use std::io::IoSlice;
use std::rc::Rc;

/// `SwooleTG.reactor`.
fn current() -> *mut c_void {
    crate::thread_reactor()
}

/// The thread's `Reactor` as a handle for the caller's call, or `None` without one.
pub(crate) fn running<'a>() -> Option<crate::reactor::Running<'a>> {
    let reactor = current();
    // SAFETY: the thread's reactor stays live until `swoole_event_free`, which no
    // caller's use of the handle spans.
    (!reactor.is_null()).then(|| unsafe { crate::reactor::Running::new(reactor) })
}

/// A field of the thread's `Reactor`, or `None` without one.
pub(crate) fn field<T>(read: impl FnOnce(&ReactorFields) -> T) -> Option<T> {
    field_mut(|fields| read(fields))
}

/// The `interest` kind [`add_or_update`] adds for `event`, if it names one.
fn direction(event: Events) -> Option<crate::reactor::Interest> {
    match event {
        Events::READ => Some(crate::reactor::Interest::AddRead),
        Events::WRITE => Some(crate::reactor::Interest::AddWrite),
        _ => None,
    }
}

/// The thread's `Reactor` while `swoole_event_is_available()`, else null.
#[must_use]
pub fn swr_v1_event_reactor() -> *mut c_void {
    if swoole_event_is_available() {
        current()
    } else {
        std::ptr::null_mut()
    }
}

/// The thread's `Reactor`, available or not (`sw_reactor()`).
#[must_use]
pub fn swr_v1_event_current() -> *mut c_void {
    current()
}

/// `bool swoole_event_is_available()`.
#[must_use]
pub fn swoole_event_is_available() -> bool {
    field(|fields| !fields.destroyed).unwrap_or(false)
}

/// `bool swoole_event_is_running()`.
#[must_use]
pub fn swoole_event_is_running() -> bool {
    field(|fields| fields.running).unwrap_or(false)
}

/// [`crate::reactor::reactor_upstream::change`] of `socket` on the thread's reactor; false
/// without one.
fn change(socket: crate::reactor::Heap<'_>, op: crate::reactor::ChangeOp, events: Events) -> bool {
    running().is_some_and(|reactor| {
        crate::reactor::reactor_upstream::change(reactor, socket, op, events)
    })
}

/// `int swoole_event_add(Socket *socket, int events)` of a heap socket: whether it
/// worked, false without a reactor.
#[must_use]
pub fn event_add(socket: crate::reactor::Heap<'_>, events: Events) -> bool {
    change(socket, crate::reactor::ChangeOp::Add, events)
}

/// `int swoole_event_set(Socket *socket, int events)` of a heap socket (kqueue's set
/// registers its address again): whether it worked, false without a reactor.
#[must_use]
pub fn event_set(socket: crate::reactor::Heap<'_>, events: Events) -> bool {
    change(socket, crate::reactor::ChangeOp::Set, events)
}

/// `Reactor::interest(socket, kind, event)` on the thread's reactor, of a heap socket:
/// whether the change worked, true when nothing changes, false without a reactor.
#[must_use]
pub fn event_interest(
    socket: crate::reactor::Heap<'_>,
    kind: crate::reactor::Interest,
    event: Events,
) -> bool {
    running().is_some_and(|reactor| {
        crate::reactor::reactor_upstream::interest(reactor, socket, kind, event)
    })
}

/// `int swoole_event_del(Socket *socket)`: whether it worked, false without a reactor.
/// Any live socket: a removal leaves its address nowhere.
#[must_use]
pub fn event_del(socket: crate::reactor::Socket<'_>) -> bool {
    running().is_some_and(|reactor| crate::reactor::reactor_upstream::reactor_del(reactor, socket))
}

/// `int swoole_event_add_or_update(Socket *socket, int event)` (`add_read_event` or
/// `add_write_event`) of a heap socket: whether it worked, false without a reactor or
/// for an event that is neither `SW_EVENT_READ` nor `SW_EVENT_WRITE`, where upstream
/// asserts.
#[must_use]
pub fn add_or_update(socket: crate::reactor::Heap<'_>, event: Events) -> bool {
    let (Some(reactor), Some(kind)) = (running(), direction(event)) else {
        return false;
    };
    crate::reactor::reactor_upstream::interest(reactor, socket, kind, Events::NONE)
}

/// `ssize_t swoole_event_write(Socket *socket, const void *data, size_t len)` of `data`
/// on a heap socket (a write that can't all go out registers it for writing): the
/// bytes taken, `None` when it failed.
#[must_use]
pub fn event_write(socket: crate::reactor::Heap<'_>, data: &[u8]) -> Option<usize> {
    let reactor = running().unwrap_or_else(|| crate::fatal("the reactor has no write function"));
    crate::tls::reactor_write::reactor_write(reactor, socket, data)
}

/// `ssize_t swoole_event_writev(Socket *socket, const iovec *iov, size_t iovcnt)` of
/// `vectors` on a heap socket (what can't all go out is copied and the socket
/// registered for writing): the bytes taken, `None` when it failed.
#[must_use]
pub fn swoole_event_writev(
    socket: crate::reactor::Heap<'_>,
    vectors: &[IoSlice<'_>],
) -> Option<usize> {
    let reactor = running().unwrap_or_else(|| crate::fatal("the reactor has no write function"));
    crate::tls::reactor_write::reactor_writev(reactor, socket, vectors)
}

/// Register a descriptor type on the current thread's reactor, when present.
pub fn set_handlers(
    fd_type: c_int,
    read: Option<Handler>,
    write: Option<Handler>,
    error: crate::reactor::ErrorHandler,
) {
    if let Some(reactor) = running() {
        crate::reactor::reactor_upstream::set_handlers(reactor, fd_type, read, write, error);
    }
}

/// Whether the thread's reactor has a handler for typed `event`.
#[must_use]
pub fn isset_handler(fd_type: c_int, event: Events) -> bool {
    running().is_some_and(|reactor| {
        crate::reactor::reactor_upstream::isset_handler(reactor, fd_type, event)
    })
}

/// `Socket *swoole_event_get_socket(int fd)`: null without a reactor.
#[must_use]
pub fn swoole_event_get_socket(fd: c_int) -> *mut c_void {
    with_registry(|registry| crate::reactor::swr_v1_reactor_registry_get(registry, fd))
        .unwrap_or(std::ptr::null_mut())
}

/// `f` over the thread's `Reactor`'s socket registry (null as `None`); `None`
/// without a reactor. `f` reaches no reactor: the registry is shared while it runs.
fn with_registry<T>(f: impl FnOnce(Option<&crate::reactor::Registry>) -> T) -> Option<T> {
    let reactor = running()?;
    Some(reactor.registry(|registry| f(Some(registry))))
}

/// The thread's `Reactor`'s prepared views (`SwooleTG.reactor`), or `None` without one.
#[must_use]
pub fn current_prepared() -> Option<crate::reactor::ReactorPrepared> {
    let reactor = current();
    (!reactor.is_null()).then(|| crate::reactor::ReactorPrepared::of(reactor))
}

/// `sw_reactor()->get_event_num()`: the sockets the thread's `Reactor` holds;
/// `None` without one.
#[must_use]
pub fn swoole_event_num() -> Option<usize> {
    with_registry(|registry| registry.map_or(0, crate::reactor::Registry::count))
}

/// The sockets the thread's `Reactor` holds, in fd order; none without one.
#[must_use]
pub fn swoole_event_sockets() -> Vec<crate::reactor::Heap<'static>> {
    with_registry(|registry| registry.map(crate::reactor::Registry::sockets))
        .flatten()
        .unwrap_or_default()
}

/// A field of the thread's `Reactor`, written; `None` without one. `write` reaches
/// no reactor (no `event_api` call, no handler or task): a nested call would be a
/// second `&mut` to the same fields, which debug builds refuse.
pub(crate) fn field_mut<T>(write: impl FnOnce(&mut ReactorFields) -> T) -> Option<T> {
    Some(running()?.with(|state| write(&mut state.fields)))
}

/// Runs `call` with the thread's `Reactor` when it is destroyed: `call` reaches the
/// reactor through that handle. Nothing without one, and the process ends if it
/// can't allocate.
pub fn on_destroy(call: fn(crate::reactor::Running<'_>)) {
    field_mut(|fields| crate::reactor::deferred::defer_destroy_call(fields, call));
}

/// `sw_reactor()->running = running`, when there is a reactor.
pub fn swoole_event_set_running(running: bool) {
    field_mut(|fields| fields.running = running);
}

/// `sw_reactor()->timeout_msec = msec`, when there is a reactor.
#[inline]
pub fn swoole_event_set_timeout_msec(msec: c_int) {
    field_mut(|fields| fields.timeout_msec = msec);
}

/// `sw_reactor()->once = once`, when there is a reactor.
pub fn swoole_event_set_once(once: bool) {
    field_mut(|fields| fields.once = once);
}

/// `sw_reactor()->running = false; sw_reactor()->bailout = true`, when there is
/// a reactor: the loop stops as after a bailout.
pub fn swoole_event_bail_out() {
    field_mut(|fields| {
        fields.running = false;
        fields.bailout = true;
    });
}

/// `sw_reactor()->bailout`: false without a reactor.
#[must_use]
pub fn swoole_event_bailed_out() -> bool {
    field(|fields| fields.bailout).unwrap_or(false)
}

/// `sw_reactor()->if_exit()`: true without a reactor. Exit conditions may run PHP.
#[must_use]
pub fn swoole_event_if_exit() -> bool {
    running().is_none_or(crate::reactor::callbacks::if_exit)
}

/// `sw_reactor()->wait()`: whether it succeeded, false without a reactor. Handlers may
/// run PHP.
#[must_use]
pub fn swoole_event_run() -> bool {
    running().is_some_and(crate::reactor::callbacks::wait)
}

/// A reactor handler over the fd of the event it is called with.
pub trait FdHandler {
    /// Whether handling `fd`'s event succeeded.
    fn handle(fd: c_int) -> bool;
}

/// Adapt a descriptor callback for registration on the reactor.
pub fn fd_handler<H: FdHandler>(
    _: crate::reactor::Running<'_>,
    event: &mut crate::reactor::Event,
) -> c_int {
    if H::handle(event.fd()) { 0 } else { -1 }
}

/// A reactor cycle task: `Reactor::future_task` or `idle_task`. The reactor
/// holds it, so it runs on whichever thread drives the loop.
pub trait CycleTask: 'static {
    /// Runs it through a reference of its own (the task may clear or replace itself),
    /// which it gives up before anything that leaves the loop without returning (a
    /// PHP bailout), so the reactor can still free it.
    fn run(self: Rc<Self>);
    /// The reactor gave it up, and this is the last reference to it.
    fn retire(self: Rc<Self>);
}

/// A replaced task, retired once the loop is out of the pass and every `run` of it
/// has given its reference up; dropped if the reactor is freed first.
struct Retired(Rc<dyn CycleTask>);

impl Deferred for Retired {
    fn run(self) {
        if Rc::strong_count(&self.0) > 1 {
            if let Err(running) = swoole_event_defer_value(self) {
                running.dispose();
            }
            return;
        }
        self.0.retire();
    }

    fn dispose(self) {}
}

/// Set (`Some`) or clear the thread reactor's future (`before`) or idle task;
/// the task it held is retired after the current pass, once no `run` of it is
/// under way. Without a reactor, nothing is set and `task` is dropped.
pub fn swoole_event_cycle(before: bool, task: Option<Rc<dyn CycleTask>>) {
    if current_prepared().is_none() {
        return;
    }
    let replaced = field_mut(|fields| {
        let slot = if before {
            &mut fields.future_task
        } else {
            &mut fields.idle_task
        };
        std::mem::replace(slot, task)
    });
    if let Some(replaced) = replaced.flatten() {
        // The reactor was just there.
        let _kept = swoole_event_defer_value(Retired(replaced));
    }
}

/// The reactor is being freed: its cycle tasks, taken out of it for the caller to
/// drop once it no longer borrows the reactor. No pass of a reactor is under way while
/// it is freed, so no `run` holds a reference to them.
pub(crate) fn take_cycle_tasks(fields: &mut crate::reactor::ReactorFields) -> impl Sized + use<> {
    [fields.idle_task.take(), fields.future_task.take()]
}

/// Whether the thread reactor's future (`before`) or idle task is set.
#[must_use]
pub fn swoole_event_cycle_isset(before: bool) -> bool {
    field(|fields| {
        if before {
            fields.future_task.is_some()
        } else {
            fields.idle_task.is_some()
        }
    })
    .unwrap_or(false)
}

/// A value the reactor hands back once: to [`Deferred::run`] after the current
/// pass, or to [`Deferred::dispose`] if the reactor is freed first.
pub trait Deferred: Sized + 'static {
    fn run(self);
    fn dispose(self);
}

/// `sw_reactor()->defer(...)` of `value`, which comes back without a reactor.
/// # Errors
/// There is no reactor.
pub fn swoole_event_defer_value<T: Deferred>(value: T) -> Result<(), T> {
    if current_prepared().is_none() {
        return Err(value);
    }
    field_mut(|fields| crate::reactor::deferred::defer_value(fields, value));
    Ok(())
}

/// `int swoole_event_wait()`: run the loop unless it may already exit, then free it.
/// # Errors
/// Returns the loop failure or a deferred bailout.
pub fn swoole_event_wait() -> Result<(), crate::reactor::LoopError> {
    // `if_exit` runs only under wait_exit, as upstream's `or`.
    let wait_exit = field(|fields| fields.wait_exit).unwrap_or(false);
    let ran = (wait_exit && swoole_event_if_exit()) || swoole_event_run();
    // A deferred escape leaves the reactor for request teardown, just as a
    // native bailout did. Server shutdown still needs its live connections.
    if field(|fields| fields.bailout).unwrap_or(false) {
        return Err(crate::reactor::LoopError::Bailout);
    }
    // The thread's reactor, which nothing here borrows any more.
    crate::reactor::swoole_event_free();
    ran.then_some(()).ok_or(crate::reactor::LoopError::Failed)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::reactor::reactor_state::tests::running;

    #[test]
    fn add_or_update_names_the_read_or_write_interest() {
        // SW_EVENT_READ is 512 and SW_EVENT_WRITE 1024; nothing else is accepted.
        for event in -2048..=2048 {
            let expected = match event {
                512 => Some(crate::reactor::Interest::AddRead),
                1024 => Some(crate::reactor::Interest::AddWrite),
                _ => None,
            };
            assert_eq!(direction(Events::from_bits(event)), expected, "{event}");
        }
    }

    thread_local! {
        static TRACE: std::cell::RefCell<Vec<String>> = const { std::cell::RefCell::new(Vec::new()) };
        /// A socket registered with the reactor, which a [`Dropped`] task frees.
        static REGISTERED: std::cell::Cell<Option<crate::reactor::OwnedSocket>> =
            const { std::cell::Cell::new(None) };
    }

    fn push(entry: &str) {
        TRACE.with_borrow_mut(|trace| trace.push(entry.to_owned()));
    }

    fn take() -> Vec<String> {
        TRACE.with_borrow_mut(std::mem::take)
    }

    /// A cycle task that clears itself, runs a nested pass, then reads itself.
    struct Reentrant(String);

    impl CycleTask for Reentrant {
        fn run(self: Rc<Self>) {
            swoole_event_cycle(false, None);
            let owner = crate::thread_reactor().cast::<crate::reactor::ReactorState>();
            crate::reactor::deferred::run_defer(running(owner));
            push(&self.0);
        }

        fn retire(self: Rc<Self>) {
            push("retired");
        }
    }

    /// The idle task, run as the end callback runs it.
    fn run_idle(fields: *mut ReactorFields) {
        if let Some(task) = unsafe { (*fields).idle_task.clone() } {
            task.run();
        }
    }

    #[test]
    fn a_task_cleared_in_its_run_outlives_a_nested_pass() {
        let state = crate::reactor::reactor_state::tests::state(3, 1);
        crate::set_thread_reactor(state.owner.cast());
        swoole_event_cycle(false, Some(Rc::new(Reentrant("ran".to_owned()))));
        run_idle(state.fields);
        assert_eq!(take(), ["ran"]);
        assert!(unsafe { (*state.fields).idle_task.is_none() });
        crate::reactor::deferred::run_defer(running(state.owner));
        assert_eq!(take(), ["retired"]);
        crate::set_thread_reactor(std::ptr::null_mut());
        unsafe { crate::reactor::reactor_state::tests::free(state.owner) };
    }

    struct Stops;

    impl CycleTask for Stops {
        fn run(self: Rc<Self>) {
            swoole_event_bail_out();
        }
        fn retire(self: Rc<Self>) {}
    }

    #[test]
    fn wait_keeps_a_bailed_reactor_for_request_teardown() {
        let state = crate::reactor::reactor_state::tests::state(3, 1);
        crate::set_thread_reactor(state.owner.cast());
        swoole_event_cycle(true, Some(Rc::new(Stops)));
        assert_eq!(swoole_event_wait(), Err(crate::reactor::LoopError::Bailout));
        assert_eq!(crate::thread_reactor(), state.owner.cast());
        assert!(unsafe { (*state.fields).bailout });
        crate::set_thread_reactor(std::ptr::null_mut());
        unsafe { crate::reactor::reactor_state::tests::free(state.owner) };
    }

    /// A cycle task that records being dropped.
    struct Dropped(&'static str);

    impl CycleTask for Dropped {
        fn run(self: Rc<Self>) {}

        fn retire(self: Rc<Self>) {}
    }

    impl Drop for Dropped {
        fn drop(&mut self) {
            // Dropped as `swoole_event_free` deletes the reactor: the thread names
            // none, and none is made meanwhile.
            let refused = crate::thread_reactor().is_null()
                && crate::reactor::swoole_event_deleting()
                && crate::reactor::swoole_event_init(0) == Err(crate::reactor::InitError::Deleting);
            push(if refused { self.0 } else { "reached" });
            // A socket still registered with the reactor being deleted leaves its
            // registry as it is freed, rather than ending the process.
            if let Some(socket) = REGISTERED.take() {
                let record = socket.into_box();
                push(if record.removed {
                    "deregistered"
                } else {
                    "kept"
                });
                unsafe { libc::close(record.fd) };
            }
        }
    }

    #[test]
    fn freeing_the_reactor_drops_its_cycle_tasks() {
        assert_eq!(crate::reactor::swoole_event_init(0), Ok(()));
        let mut fds = [0; 2];
        assert_eq!(unsafe { libc::pipe(fds.as_mut_ptr()) }, 0);
        let socket = crate::reactor::OwnedSocket::new(Box::new(
            crate::reactor::SocketRecord::with_fd(fds[0], 0),
        ));
        assert!(event_add(socket.heap(), crate::reactor::Events::READ));
        REGISTERED.set(Some(socket));
        swoole_event_cycle(false, Some(Rc::new(Dropped("idle"))));
        swoole_event_cycle(true, Some(Rc::new(Dropped("future"))));
        assert_eq!(take(), Vec::<String>::new());
        assert!(crate::reactor::swoole_event_free());
        assert_eq!(take(), ["idle", "deregistered", "future"]);
        assert!(!crate::reactor::swoole_event_deleting());
        unsafe { libc::close(fds[1]) };
    }

    /// A cycle task that gives its reference up, then leaves the loop without
    /// returning, as `exit()` in a PHP callback does (a panic stands for it here).
    struct Escapes;

    impl CycleTask for Escapes {
        fn run(self: Rc<Self>) {
            drop(self);
            panic!("escape");
        }

        fn retire(self: Rc<Self>) {}
    }

    impl Drop for Escapes {
        fn drop(&mut self) {
            push("idle");
        }
    }

    #[test]
    fn freeing_the_reactor_drops_a_cycle_task_whose_run_left_the_loop() {
        let state = crate::reactor::reactor_state::tests::state(3, 1);
        crate::set_thread_reactor(state.owner.cast());
        swoole_event_cycle(false, Some(Rc::new(Escapes)));
        let fields = std::panic::AssertUnwindSafe(state.fields);
        assert!(std::panic::catch_unwind(move || run_idle(*fields)).is_err());
        assert_eq!(take(), Vec::<String>::new());
        crate::set_thread_reactor(std::ptr::null_mut());
        unsafe { crate::reactor::reactor_state::tests::free(state.owner) };
        assert_eq!(take(), ["idle"]);
    }

    fn noop(_: crate::reactor::Running<'_>) {}

    /// A second `&mut` to the fields while the first is live is refused before it
    /// is made; Miri reports the nested borrow without the check (#3248).
    #[test]
    fn field_mut_refuses_a_nested_borrow() {
        let state = crate::reactor::reactor_state::tests::state(3, 1);
        crate::set_thread_reactor(state.owner.cast());
        let nested = std::panic::catch_unwind(|| {
            field_mut(|fields| {
                on_destroy(noop);
                fields.running = true;
            })
        });
        assert!(nested.is_err());
        crate::set_thread_reactor(std::ptr::null_mut());
        unsafe { crate::reactor::reactor_state::tests::free(state.owner) };
    }

    #[test]
    fn without_a_reactor_the_loop_is_neither_available_nor_running() {
        // No swoole_init: SwooleTG.reactor is null.
        assert!(!swoole_event_is_available());
        assert!(!swoole_event_is_running());
        assert!(swr_v1_event_reactor().is_null());
        assert!(swr_v1_event_current().is_null());
    }
}
