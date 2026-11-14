// SPDX-License-Identifier: Apache-2.0
// Compatibility derived from Swoole v6.2.4 src/wrapper/event.cc (Swoole contributors).
//! event.cc's `swoole_event_*` C API over the thread's `Reactor` (`SwooleTG.reactor`).
//! With `upstream-symbols` those C++ or a test crate calls are defined under their
//! upstream (Itanium) symbols.
//! `swoole_event_init` and `swoole_event_free` are `reactor_state.rs`'s, with the
//! `Reactor`'s constructor and destructor; `swoole_event_defer` is deferred.rs's.
use crate::handlers::Handler;
use crate::reactor_upstream::reactor_change;
use crate::{ReactorFields, SocketRecord};
use std::cell::Cell;
use std::ffi::{c_int, c_void};
use std::rc::Rc;

/// `SW_EVENT_READ` and `SW_EVENT_WRITE`.
const EVENT_READ: c_int = swoole_rs_corebase::constants::EventType::SW_EVENT_READ as c_int;
const EVENT_WRITE: c_int = swoole_rs_corebase::constants::EventType::SW_EVENT_WRITE as c_int;

/// `SwooleTG.reactor`.
fn current() -> *mut c_void {
    swoole_rs_corebase::thread_reactor()
}

/// The thread's `Reactor` as a handle for the caller's call, or `None` without one.
pub(crate) fn running<'a>() -> Option<crate::Running<'a>> {
    let reactor = current();
    // SAFETY: the thread's reactor stays live until `swoole_event_free`, which no
    // caller's use of the handle spans.
    (!reactor.is_null()).then(|| unsafe { crate::Running::new(reactor) })
}

/// A field of the thread's `Reactor`, or `None` without one.
pub(crate) fn field<T>(read: impl FnOnce(&ReactorFields) -> T) -> Option<T> {
    field_mut(|fields| read(fields))
}

/// The `interest` kind [`add_or_update`] adds for `event`, if it names one.
fn direction(event: c_int) -> Option<crate::Interest> {
    match event {
        EVENT_READ => Some(crate::Interest::AddRead),
        EVENT_WRITE => Some(crate::Interest::AddWrite),
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
#[cfg_attr(
    feature = "upstream-symbols",
    unsafe(export_name = "_Z25swoole_event_is_availablev")
)]
#[must_use]
pub extern "C" fn swoole_event_is_available() -> bool {
    field(|fields| !fields.destroyed).unwrap_or(false)
}

/// `bool swoole_event_is_running()`.
#[must_use]
pub fn swoole_event_is_running() -> bool {
    field(|fields| fields.running).unwrap_or(false)
}

/// `int swoole_event_add(Socket *socket, int events)`.
///
/// # Safety
/// As `Reactor::add`, with a live `SwooleTG.reactor`.
#[cfg_attr(
    feature = "upstream-symbols",
    unsafe(export_name = "_Z16swoole_event_addPN6swoole7network6SocketEi")
)]
pub unsafe extern "C" fn swoole_event_add(socket: *mut SocketRecord, events: c_int) -> c_int {
    // SAFETY: as the caller promises.
    unsafe { reactor_change(current(), socket, 0, events) }
}

/// [`crate::reactor_upstream::change`] of `socket` on the thread's reactor; false
/// without one.
fn change(socket: crate::Socket<'static>, op: crate::ChangeOp, events: c_int) -> bool {
    running().is_some_and(|reactor| crate::reactor_upstream::change(reactor, socket, op, events))
}

/// [`swoole_event_add`] of a socket that is never freed while registered: whether it
/// worked, false without a reactor.
#[must_use]
pub fn event_add(socket: crate::Socket<'static>, events: c_int) -> bool {
    change(socket, crate::ChangeOp::Add, events)
}

/// `int swoole_event_set(Socket *socket, int events)` of a socket that is never freed
/// while registered (kqueue's set registers its address again): whether it worked,
/// false without a reactor.
#[must_use]
pub fn event_set(socket: crate::Socket<'static>, events: c_int) -> bool {
    change(socket, crate::ChangeOp::Set, events)
}

/// `Reactor::interest(socket, kind, event)` on the thread's reactor, of a socket that
/// is never freed while registered: whether the change worked, true when nothing
/// changes, false without a reactor.
#[must_use]
pub fn event_interest(socket: crate::Socket<'static>, kind: crate::Interest, event: c_int) -> bool {
    running().is_some_and(|reactor| crate::reactor_upstream::interest(reactor, socket, kind, event))
}

/// `int swoole_event_del(Socket *socket)`: whether it worked, false without a reactor.
/// Any live socket: a removal leaves its address nowhere.
#[must_use]
pub fn event_del(socket: crate::Socket<'_>) -> bool {
    running().is_some_and(|reactor| crate::reactor_upstream::reactor_del(reactor, socket))
}

/// `int swoole_event_add_or_update(Socket *socket, int event)` (`add_read_event` or
/// `add_write_event`) of a socket that is never freed while registered: whether it
/// worked, false without a reactor or for an event that is neither `SW_EVENT_READ` nor
/// `SW_EVENT_WRITE`, where upstream asserts.
#[must_use]
pub fn add_or_update(socket: crate::Socket<'static>, event: c_int) -> bool {
    let (Some(reactor), Some(kind)) = (running(), direction(event)) else {
        return false;
    };
    crate::reactor_upstream::interest(reactor, socket, kind, 0)
}

/// `ssize_t swoole_event_write(Socket *socket, const void *data, size_t len)`.
///
/// # Safety
/// As `Reactor::write`, with a live `SwooleTG.reactor`.
#[cfg_attr(
    feature = "upstream-symbols",
    unsafe(export_name = "_Z18swoole_event_writePN6swoole7network6SocketEPKvm")
)]
pub unsafe extern "C-unwind" fn swoole_event_write(
    socket: *mut SocketRecord,
    data: *const c_void,
    len: usize,
) -> isize {
    let reactor = current();
    // The constructor sets it; upstream calls it unchecked.
    let Some(write) = field(|fields| fields.write).flatten() else {
        swoole_rs_corebase::fatal("the reactor has no write function")
    };
    // SAFETY: as the caller promises.
    unsafe { write(reactor, socket, data, len) }
}

/// [`swoole_event_write`] of `data` on a socket that is never freed while registered
/// (a write that can't all go out registers it for writing).
#[must_use]
pub fn event_write(socket: crate::Socket<'static>, data: &[u8]) -> isize {
    let reactor = current();
    let Some(write) = field(|fields| fields.write).flatten() else {
        swoole_rs_corebase::fatal("the reactor has no write function")
    };
    // SAFETY: `Reactor::write` of the thread's live reactor, which only its
    // constructor sets (`ReactorFields` is written through nothing public): a live
    // record that outlives its registration, and the slice's bytes, which it copies.
    unsafe { write(reactor, socket.as_ptr(), data.as_ptr().cast(), data.len()) }
}

/// `ssize_t swoole_event_writev(Socket *socket, const iovec *iov, size_t iovcnt)`.
///
/// # Safety
/// As `Reactor::writev`, with a live `SwooleTG.reactor`.
#[cfg_attr(
    feature = "upstream-symbols",
    unsafe(export_name = "_Z19swoole_event_writevPN6swoole7network6SocketEPK5iovecm")
)]
pub unsafe extern "C-unwind" fn swoole_event_writev(
    socket: *mut SocketRecord,
    iov: *const libc::iovec,
    iovcnt: usize,
) -> isize {
    let reactor = current();
    let Some(writev) = field(|fields| fields.writev).flatten() else {
        swoole_rs_corebase::fatal("the reactor has no writev function")
    };
    // SAFETY: as the caller promises.
    unsafe { writev(reactor, socket, iov, iovcnt) }
}

/// `void swoole_event_set_handler(int fd_type, int event, ReactorHandler handler)`;
/// nothing without a reactor. A `Handler` is a safe function, sound with the live
/// reactor and event the loop calls it with.
#[cfg_attr(
    feature = "upstream-symbols",
    unsafe(export_name = "_Z24swoole_event_set_handleriiPFiPN6swoole7ReactorEPNS_5EventEE")
)]
pub extern "C" fn swoole_event_set_handler(fd_type: c_int, event: c_int, handler: Option<Handler>) {
    if let Some(reactor) = running() {
        crate::reactor_upstream::reactor_set_handler(reactor, fd_type, event, handler);
    }
}

/// `bool swoole_event_isset_handler(int fd_type, int event)`: false without a reactor.
#[cfg_attr(
    feature = "upstream-symbols",
    unsafe(export_name = "_Z26swoole_event_isset_handlerii")
)]
#[must_use]
pub extern "C" fn swoole_event_isset_handler(fd_type: c_int, event: c_int) -> bool {
    running().is_some_and(|reactor| {
        crate::reactor_upstream::reactor_isset_handler(reactor, fd_type, event)
    })
}

/// `Socket *swoole_event_get_socket(int fd)`: null without a reactor.
#[cfg_attr(
    feature = "upstream-symbols",
    unsafe(export_name = "_Z23swoole_event_get_socketi")
)]
#[must_use]
pub extern "C" fn swoole_event_get_socket(fd: c_int) -> *mut c_void {
    with_registry(|registry| crate::swr_v1_reactor_registry_get(registry, fd))
        .unwrap_or(std::ptr::null_mut())
}

/// `f` over the thread's `Reactor`'s socket registry (null as `None`); `None`
/// without a reactor. `f` reaches no reactor: the registry is shared while it runs.
fn with_registry<T>(f: impl FnOnce(Option<&crate::Registry>) -> T) -> Option<T> {
    let reactor = running()?;
    Some(f(Some(reactor.registry())))
}

/// The thread's `Reactor`'s prepared views (`SwooleTG.reactor`), or `None` without one.
#[must_use]
pub fn current_prepared() -> Option<crate::ReactorPrepared> {
    let reactor = current();
    (!reactor.is_null()).then(|| crate::ReactorPrepared::of(reactor))
}

/// `sw_reactor()->get_event_num()`: the sockets the thread's `Reactor` holds;
/// `None` without one.
#[must_use]
pub fn swoole_event_num() -> Option<usize> {
    with_registry(|registry| registry.map_or(0, crate::Registry::count))
}

/// A field of the thread's `Reactor`, written; `None` without one. `write` reaches
/// no reactor (no `event_api` call, no handler or task): a nested call would be a
/// second `&mut` to the same fields, which debug builds refuse.
pub(crate) fn field_mut<T>(write: impl FnOnce(&mut ReactorFields) -> T) -> Option<T> {
    Some(running()?.with(|state| write(&mut state.fields)))
}

/// Runs `call` (with null data) when the thread's `Reactor` is destroyed;
/// nothing without one, and the process ends if it can't allocate.
pub fn on_destroy(call: extern "C-unwind" fn(*mut c_void)) {
    field_mut(|fields| crate::deferred::defer_destroy_any(fields, call));
}

/// `sw_reactor()->running = running`, when there is a reactor.
pub fn swoole_event_set_running(running: bool) {
    field_mut(|fields| fields.running = running);
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
    running().is_none_or(crate::callbacks::if_exit)
}

/// `sw_reactor()->wait()`: -1 without a reactor. Handlers may run PHP.
#[must_use]
pub fn swoole_event_run() -> c_int {
    running().map_or(-1, crate::callbacks::wait)
}

/// A reactor handler over the fd of the event it is called with.
pub trait FdHandler {
    fn handle(fd: c_int) -> c_int;
}

extern "C" fn fd_handler<H: FdHandler>(_: crate::Running<'_>, event: &mut crate::Event) -> c_int {
    H::handle(event.fd())
}

/// `swoole_event_set_handler(fd_type, event, H)`, when there is a reactor.
pub fn swoole_event_set_fd_handler<H: FdHandler>(fd_type: c_int, event: c_int) {
    swoole_event_set_handler(fd_type, event, Some(fd_handler::<H>));
}

/// A reactor cycle task: `Reactor::future_task` or `idle_task`. The reactor
/// holds it, so it runs on whichever thread drives the loop.
pub trait CycleTask: 'static {
    fn run(&self);
    /// The reactor gave it up, and no call of [`CycleTask::run`] is under way.
    fn retire(self: Box<Self>);
}

/// What `ReactorTask::data` holds (from `Rc::into_raw`) while `cycle_task` is its
/// callback: the task, and how many calls of its `run` are under way (a nested
/// pass may replace it). Shared, so a replaced slot is never claimed uniquely
/// while a run still borrows it.
struct Slot {
    running: Cell<u32>,
    task: Box<dyn CycleTask>,
}

/// # Safety
/// `data` is the live `Slot` `swoole_event_cycle` gave the reactor.
unsafe extern "C-unwind" fn cycle_task(data: *mut c_void) {
    // SAFETY: as above; the slot outlives `run`, since a replaced slot is freed
    // only once `running` is back to 0.
    let slot = unsafe { &*data.cast::<Slot>() };
    slot.running.set(slot.running.get() + 1);
    slot.task.run();
    // Skipped when `run` bails out, so the count stays up: `take_cycle_tasks`
    // frees the slot if the reactor still holds it when freed; once replaced, it is
    // deferred pass after pass and kept (`Retired`), never freed.
    slot.running.set(slot.running.get() - 1);
}

/// A replaced task, retired once the loop is out of the pass and its `run`;
/// dropped if the reactor is freed first (kept if it is still running).
struct Retired(Rc<Slot>);

impl Deferred for Retired {
    fn run(self) {
        if self.0.running.get() != 0 {
            if let Err(running) = swoole_event_defer_value(self) {
                running.dispose();
            }
            return;
        }
        // The only owner: nothing else holds the slot once the reactor gave it up.
        if let Ok(slot) = Rc::try_unwrap(self.0) {
            slot.task.retire();
        }
    }

    fn dispose(self) {
        if self.0.running.get() != 0 {
            #[expect(
                clippy::mem_forget,
                reason = "its task is still running and no reactor is left to retire it later"
            )]
            std::mem::forget(self);
        }
    }
}

/// Set (`Some`) or clear the thread reactor's future (`before`) or idle task;
/// the task it held is retired after the current pass, once no `run` of it is
/// under way. Without a reactor, nothing is set and `task` is dropped.
pub fn swoole_event_cycle(before: bool, task: Option<Box<dyn CycleTask>>) {
    if current_prepared().is_none() {
        return;
    }
    let data = task.map(|task| {
        let running = Cell::new(0);
        Rc::into_raw(Rc::new(Slot { running, task })).cast_mut()
    });
    let replaced = field_mut(|fields| {
        let slot = if before {
            &mut fields.future_task
        } else {
            &mut fields.idle_task
        };
        replace_slot(slot, data)
    });
    if let Some(replaced) = replaced.flatten() {
        // The reactor was just there.
        let _kept = swoole_event_defer_value(replaced);
    }
}

thread_local! {
    /// The slots [`swoole_event_cycle`] put in this thread's reactors and that are
    /// still there. Native code may set a cycle task of its own, so a slot's data is
    /// an `Rc<Slot>` only when it is listed here.
    static GIVEN: std::cell::RefCell<Vec<*const Slot>> = const { std::cell::RefCell::new(Vec::new()) };
}

/// Put `data` (or nothing) in a cycle slot, and hand back the task it held if
/// [`swoole_event_cycle`] put it there; any other task is left to its owner.
fn replace_slot(task: &mut crate::ReactorTask, data: Option<*mut Slot>) -> Option<Retired> {
    let replaced = std::mem::replace(
        task,
        crate::ReactorTask {
            callback: data.map(|_| -> crate::DeferCall { cycle_task }),
            data: data.map_or(std::ptr::null_mut(), <*mut Slot>::cast),
        },
    );
    let old = replaced.data.cast::<Slot>().cast_const();
    let ours = GIVEN.with_borrow_mut(|given| {
        given.extend(data.map(<*mut Slot>::cast_const));
        let at = given.iter().position(|&slot| slot == old)?;
        Some(given.swap_remove(at))
    })?;
    // SAFETY: a listed slot is the `Rc<Slot>` `swoole_event_cycle` gave up, which
    // the reactor no longer names and the list no longer holds.
    Some(Retired(unsafe { Rc::from_raw(ours) }))
}

/// The reactor is being freed: the cycle tasks [`swoole_event_cycle`] gave it, taken
/// out of it for the caller to drop once it no longer borrows the reactor.
/// No pass of a reactor is under way while it is freed, so a run still counted
/// was left by a bailout (`exit()` in the callback) and borrows nothing now.
pub(crate) fn take_cycle_tasks(fields: &mut crate::ReactorFields) -> impl Sized + use<> {
    [&mut fields.idle_task, &mut fields.future_task].map(|task| replace_slot(task, None))
}

/// Whether the thread reactor's future (`before`) or idle task is set.
#[must_use]
pub fn swoole_event_cycle_isset(before: bool) -> bool {
    field(|fields| {
        if before {
            fields.future_task.callback.is_some()
        } else {
            fields.idle_task.callback.is_some()
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
    field_mut(|fields| crate::deferred::defer_value(fields, value));
    Ok(())
}

/// `int swoole_event_wait()`: run the loop unless it may already exit, then free it.
#[cfg(all(feature = "upstream-symbols", not(test)))]
#[unsafe(export_name = "_Z17swoole_event_waitv")]
#[must_use]
pub extern "C-unwind" fn swoole_event_wait() -> c_int {
    // `if_exit` runs only under wait_exit, as upstream's `or`.
    let wait_exit = field(|fields| fields.wait_exit).unwrap_or(false);
    let retval = if wait_exit && swoole_event_if_exit() {
        0
    } else {
        swoole_event_run()
    };
    // The thread's reactor, which nothing here borrows any more.
    crate::swoole_event_free();
    retval
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::reactor_state::tests::running;

    #[test]
    fn add_or_update_names_the_read_or_write_interest() {
        // SW_EVENT_READ is 512 and SW_EVENT_WRITE 1024; nothing else is accepted.
        for event in -2048..=2048 {
            let expected = match event {
                512 => Some(crate::Interest::AddRead),
                1024 => Some(crate::Interest::AddWrite),
                _ => None,
            };
            assert_eq!(direction(event), expected, "{event}");
        }
    }

    thread_local! {
        static TRACE: std::cell::RefCell<Vec<String>> = const { std::cell::RefCell::new(Vec::new()) };
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
        fn run(&self) {
            swoole_event_cycle(false, None);
            let owner = swoole_rs_corebase::thread_reactor().cast::<crate::ReactorState>();
            crate::deferred::run_defer(running(owner));
            push(&self.0);
        }

        fn retire(self: Box<Self>) {
            push("retired");
        }
    }

    #[test]
    fn a_task_cleared_in_its_run_outlives_a_nested_pass() {
        let state = crate::reactor_state::tests::state(3, 1);
        swoole_rs_corebase::set_thread_reactor(state.owner.cast());
        swoole_event_cycle(false, Some(Box::new(Reentrant("ran".to_owned()))));
        let task = unsafe { (*state.fields).idle_task };
        unsafe { task.callback.unwrap()(task.data) };
        assert_eq!(take(), ["ran"]);
        assert!(unsafe { (*state.fields).idle_task }.callback.is_none());
        crate::deferred::run_defer(running(state.owner));
        assert_eq!(take(), ["retired"]);
        swoole_rs_corebase::set_thread_reactor(std::ptr::null_mut());
        unsafe { crate::reactor_state::tests::free(state.owner) };
    }

    /// A cycle task that records being dropped.
    struct Dropped(&'static str);

    impl CycleTask for Dropped {
        fn run(&self) {}

        fn retire(self: Box<Self>) {}
    }

    impl Drop for Dropped {
        fn drop(&mut self) {
            push(self.0);
        }
    }

    #[test]
    fn freeing_the_reactor_drops_its_cycle_tasks() {
        let state = crate::reactor_state::tests::state(3, 1);
        swoole_rs_corebase::set_thread_reactor(state.owner.cast());
        swoole_event_cycle(false, Some(Box::new(Dropped("idle"))));
        swoole_event_cycle(true, Some(Box::new(Dropped("future"))));
        assert_eq!(take(), Vec::<String>::new());
        swoole_rs_corebase::set_thread_reactor(std::ptr::null_mut());
        unsafe { crate::reactor_state::tests::free(state.owner) };
        assert_eq!(take(), ["idle", "future"]);
    }

    #[test]
    fn freeing_the_reactor_drops_a_cycle_task_its_bailout_left_running() {
        let state = crate::reactor_state::tests::state(3, 1);
        swoole_rs_corebase::set_thread_reactor(state.owner.cast());
        swoole_event_cycle(false, Some(Box::new(Dropped("idle"))));
        // A run that bailed out: counted, never finished.
        let slot = unsafe { &*(*state.fields).idle_task.data.cast::<Slot>() };
        slot.running.set(1);
        swoole_rs_corebase::set_thread_reactor(std::ptr::null_mut());
        unsafe { crate::reactor_state::tests::free(state.owner) };
        assert_eq!(take(), ["idle"]);
    }

    extern "C-unwind" fn noop(_: *mut c_void) {}

    /// A second `&mut` to the fields while the first is live is refused before it
    /// is made; Miri reports the nested borrow without the check (#3248).
    #[test]
    fn field_mut_refuses_a_nested_borrow() {
        let state = crate::reactor_state::tests::state(3, 1);
        swoole_rs_corebase::set_thread_reactor(state.owner.cast());
        let nested = std::panic::catch_unwind(|| {
            field_mut(|fields| {
                on_destroy(noop);
                fields.running = true;
            })
        });
        assert!(nested.is_err());
        swoole_rs_corebase::set_thread_reactor(std::ptr::null_mut());
        unsafe { crate::reactor_state::tests::free(state.owner) };
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
