// SPDX-License-Identifier: Apache-2.0
// Compatibility derived from Swoole v6.2.4 src/wrapper/event.cc (Swoole contributors).
//! event.cc's `swoole_event_*` C API over the thread's `Reactor` (`SwooleTG.reactor`).
//! With `upstream-symbols` those C++ or a test crate calls are defined under their
//! upstream (Itanium) symbols.
//! `swoole_event_init` and `swoole_event_free` are `reactor_state.rs`'s, with the
//! `Reactor`'s constructor and destructor; `swoole_event_defer` is callbacks.rs's.
use crate::handlers::Handler;
use crate::reactor_upstream::{prepared, reactor_change, reactor_interest};
use crate::{ReactorFields, SocketRecord};
use std::cell::Cell;
use std::ffi::{c_int, c_void};
use std::rc::Rc;

/// `SW_EVENT_READ` and `SW_EVENT_WRITE`.
const EVENT_READ: c_int = 512;
const EVENT_WRITE: c_int = 1024;

/// `SwooleTG.reactor`.
fn current() -> *mut c_void {
    swoole_rs_corebase::thread_reactor()
}

/// A field of the thread's `Reactor`, or `None` without one.
pub(crate) fn field<T>(read: impl FnOnce(&ReactorFields) -> T) -> Option<T> {
    field_mut(|fields| read(fields))
}

/// The `interest` kind `swoole_event_add_or_update` adds for `event`, if it names one.
fn direction(event: c_int) -> Option<u32> {
    match event {
        EVENT_READ => Some(2),
        EVENT_WRITE => Some(3),
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

/// `int swoole_event_set(Socket *socket, int events)`.
///
/// # Safety
/// As `Reactor::set`, with a live `SwooleTG.reactor`.
pub unsafe fn swoole_event_set(socket: *mut SocketRecord, events: c_int) -> c_int {
    // SAFETY: as the caller promises.
    unsafe { reactor_change(current(), socket, 1, events) }
}

/// `int swoole_event_del(Socket *socket)`.
///
/// # Safety
/// As `Reactor::del`, with a live `SwooleTG.reactor`.
#[cfg_attr(
    feature = "upstream-symbols",
    unsafe(export_name = "_Z16swoole_event_delPN6swoole7network6SocketE")
)]
pub unsafe extern "C" fn swoole_event_del(socket: *mut SocketRecord) -> c_int {
    // SAFETY: as the caller promises.
    unsafe { reactor_change(current(), socket, 2, 0) }
}

/// `int swoole_event_add_or_update(Socket *socket, int event)`: `add_read_event` or
/// `add_write_event`; upstream asserts the event is one of them.
///
/// # Safety
/// As `Reactor::add_read_event`, with a live `SwooleTG.reactor`.
#[cfg_attr(
    feature = "upstream-symbols",
    unsafe(export_name = "_Z26swoole_event_add_or_updatePN6swoole7network6SocketEi")
)]
pub unsafe extern "C" fn swoole_event_add_or_update(
    socket: *mut SocketRecord,
    event: c_int,
) -> c_int {
    let Some(kind) = direction(event) else {
        debug_assert!(false, "swoole_event_add_or_update: event {event}");
        return -1;
    };
    // SAFETY: as the caller promises.
    unsafe { reactor_interest(current(), socket, kind, 0) }
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
        std::process::abort()
    };
    // SAFETY: as the caller promises.
    unsafe { write(reactor, socket, data, len) }
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
        std::process::abort()
    };
    // SAFETY: as the caller promises.
    unsafe { writev(reactor, socket, iov, iovcnt) }
}

/// `void swoole_event_set_handler(int fd_type, int event, ReactorHandler handler)`;
/// nothing without a reactor. Safe callers use [`swoole_event_set_fd_handler`].
///
/// # Safety
/// `handler` is sound to call with the live reactor and any live event of
/// `fd_type`, whenever the reactor dispatches one.
#[cfg_attr(
    feature = "upstream-symbols",
    unsafe(export_name = "_Z24swoole_event_set_handleriiPFiPN6swoole7ReactorEPNS_5EventEE")
)]
pub unsafe extern "C" fn swoole_event_set_handler(
    fd_type: c_int,
    event: c_int,
    handler: Option<Handler>,
) {
    let reactor = current();
    if !reactor.is_null() {
        // SAFETY: the thread's live reactor, and a handler as the caller promises.
        unsafe { crate::reactor_upstream::reactor_set_handler(reactor, fd_type, event, handler) };
    }
}

/// `bool swoole_event_isset_handler(int fd_type, int event)`: false without a reactor.
#[cfg_attr(
    feature = "upstream-symbols",
    unsafe(export_name = "_Z26swoole_event_isset_handlerii")
)]
#[must_use]
pub extern "C" fn swoole_event_isset_handler(fd_type: c_int, event: c_int) -> bool {
    let reactor = current();
    // SAFETY: the thread's live reactor.
    !reactor.is_null()
        && unsafe { crate::reactor_upstream::reactor_isset_handler(reactor, fd_type, event) }
}

/// `Socket *swoole_event_get_socket(int fd)`: null without a reactor.
#[cfg_attr(
    feature = "upstream-symbols",
    unsafe(export_name = "_Z23swoole_event_get_socketi")
)]
#[must_use]
pub extern "C" fn swoole_event_get_socket(fd: c_int) -> *mut c_void {
    let Some(registry) = current_prepared().map(|prepared| prepared.registry) else {
        return std::ptr::null_mut();
    };
    // SAFETY: the live reactor's registry.
    crate::swr_v1_reactor_registry_get(unsafe { registry.as_ref() }, fd)
}

/// The thread's `Reactor`'s prepared views, or `None` without one.
fn current_prepared() -> Option<crate::ReactorPrepared> {
    let reactor = current();
    // SAFETY: `SwooleTG.reactor` is live until `swoole_event_free` clears it.
    (!reactor.is_null()).then(|| unsafe { prepared(reactor) })
}

/// A field of the thread's `Reactor`, written; `None` without one.
fn field_mut<T>(write: impl FnOnce(&mut ReactorFields) -> T) -> Option<T> {
    let fields = current_prepared()?.fields;
    // SAFETY: a live reactor's fields, borrowed for this call alone; the loop
    // reads them through its raw pointer, never across it.
    Some(write(unsafe { &mut *fields }))
}

/// Runs `call` (with null data) when the thread's `Reactor` is destroyed;
/// nothing without one.
#[cfg(all(target_os = "linux", feature = "upstream-symbols", not(test)))]
pub(crate) fn on_destroy(call: extern "C-unwind" fn(*mut c_void)) {
    field_mut(|fields| {
        let queue = &raw mut fields.destroy_callbacks;
        // SAFETY: the live reactor's destroy queue; `call` is safe with any data.
        let pushed =
            unsafe { crate::swr_v1_reactor_deferred_push(queue, call, std::ptr::null_mut(), None) };
        if pushed != 0 {
            std::process::abort();
        }
    });
}

/// `sw_reactor()->running = running`, when there is a reactor.
pub fn swoole_event_set_running(running: bool) {
    field_mut(|fields| fields.running = running);
}

/// `sw_reactor()->once = once`, when there is a reactor.
pub fn swoole_event_set_once(once: bool) {
    field_mut(|fields| fields.once = once);
}

/// `sw_reactor()->bailout`: false without a reactor.
#[must_use]
pub fn swoole_event_bailed_out() -> bool {
    field(|fields| fields.bailout).unwrap_or(false)
}

/// `sw_reactor()->if_exit()`: true without a reactor. Exit conditions may run PHP.
#[must_use]
pub fn swoole_event_if_exit() -> bool {
    let reactor = current();
    // SAFETY: the thread's live reactor.
    reactor.is_null() || unsafe { crate::callbacks::reactor_if_exit(reactor) }
}

/// `sw_reactor()->wait()`: -1 without a reactor. Handlers may run PHP.
#[must_use]
pub fn swoole_event_run() -> c_int {
    let reactor = current();
    if reactor.is_null() {
        return -1;
    }
    // SAFETY: the thread's live reactor, whose backend `swoole_event_init` opened.
    unsafe { crate::callbacks::reactor_wait(reactor) }
}

/// A reactor handler over the fd of the event it is called with.
pub trait FdHandler {
    fn handle(fd: c_int) -> c_int;
}

/// # Safety
/// The reactor calls it with a live `Event`.
unsafe extern "C" fn fd_handler<H: FdHandler>(_reactor: *mut c_void, event: *mut c_void) -> c_int {
    // SAFETY: as above.
    let fd = unsafe { (*event.cast::<crate::Event>()).fd };
    H::handle(fd)
}

/// `swoole_event_set_handler(fd_type, event, H)`, when there is a reactor.
pub fn swoole_event_set_fd_handler<H: FdHandler>(fd_type: c_int, event: c_int) {
    // SAFETY: `fd_handler` needs only the live event the reactor passes it.
    unsafe { swoole_event_set_handler(fd_type, event, Some(fd_handler::<H>)) };
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
    // Skipped when `run` bails out: the slot is then kept, never freed.
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
        let replaced = slot.data;
        *slot = crate::ReactorTask {
            callback: data.map(|_| cycle_task as crate::DeferCall),
            data: data.map_or(std::ptr::null_mut(), <*mut Slot>::cast),
        };
        replaced
    });
    let Some(replaced) = replaced.filter(|data| !data.is_null()) else {
        return;
    };
    // SAFETY: only this function sets these tasks' data, to an `Rc<Slot>` it gave
    // up, which the reactor no longer holds.
    let replaced = Retired(unsafe { Rc::from_raw(replaced.cast::<Slot>()) });
    // The reactor was just there.
    let _kept = swoole_event_defer_value(replaced);
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

/// # Safety
/// `data` is `swoole_event_defer_value`'s box, handed back once.
unsafe extern "C-unwind" fn deferred_run<T: Deferred>(data: *mut c_void) {
    // SAFETY: as above; the box is freed before `run`, which may bail out.
    let value = *unsafe { Box::from_raw(data.cast::<T>()) };
    T::run(value);
}

/// # Safety
/// As [`deferred_run`].
unsafe extern "C-unwind" fn deferred_dispose<T: Deferred>(data: *mut c_void) {
    // SAFETY: as above.
    let value = *unsafe { Box::from_raw(data.cast::<T>()) };
    T::dispose(value);
}

/// `sw_reactor()->defer(...)` of `value`, which comes back without a reactor.
/// # Errors
/// There is no reactor.
pub fn swoole_event_defer_value<T: Deferred>(value: T) -> Result<(), T> {
    let Some(fields) = current_prepared().map(|prepared| prepared.fields) else {
        return Err(value);
    };
    let data = Box::into_raw(Box::new(value)).cast();
    // SAFETY: the live reactor's fields; the queue hands `data` back once.
    unsafe { crate::defer(fields, deferred_run::<T>, data, Some(deferred_dispose::<T>)) };
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

    #[test]
    fn add_or_update_names_the_read_or_write_interest() {
        // SW_EVENT_READ is 512 and SW_EVENT_WRITE 1024; nothing else is accepted.
        for event in -2048..=2048 {
            let expected = match event {
                512 => Some(2),
                1024 => Some(3),
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
            unsafe { crate::deferred::run_defer(owner) };
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
        unsafe { crate::deferred::run_defer(state.owner) };
        assert_eq!(take(), ["retired"]);
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
