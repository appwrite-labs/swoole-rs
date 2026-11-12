// SPDX-License-Identifier: Apache-2.0
// Compatibility derived from Swoole v6.2.4 src/reactor/base.cc (Swoole contributors).
//! The reactor's end callbacks and exit conditions (upstream's `end_callbacks` and
//! `exit_conditions` maps), its built-in ones, `if_exit`, `execute_end_callbacks` and
//! `wait`. Each slot is keyed by upstream's `EndCallback` or `ExitCondition` and holds
//! a plain function or a built-in. A walk re-reads each slot as it reaches it, so a
//! callback that sets a later key is seen, one it erases is not, and an earlier key it
//! sets waits for the next walk.
use crate::Running;
use crate::event_api::running;
use crate::reactor_state::ReactorState;
use std::ffi::{c_int, c_void};

/// An end callback a caller sets (`Reactor::set_end_callback`).
pub type EndFn = extern "C-unwind" fn();
/// An exit condition a caller sets: whether the loop may exit, given the event count it
/// may change for the conditions after it.
pub type ExitFn = extern "C-unwind" fn(&mut usize) -> bool;

/// `Reactor::PRIORITY_*`: `PRIORITY_IOURING_SUBMIT` (7) is the last.
const PRIORITY_DEFER_TASK: c_int = 1;
const PRIORITY_IDLE_TASK: c_int = 2;
pub(crate) const PRIORITY_SIGNAL_CALLBACK: c_int = 3;
const PRIORITY_TRY_EXIT: c_int = 4;
#[cfg(all(target_os = "linux", target_env = "gnu"))]
const PRIORITY_MALLOC_TRIM: c_int = 5;
const END_SLOTS: usize = 8;
/// `Reactor::EXIT_CONDITION_*`: 0 to 9, then `DEFAULT` (999) and `USER_AFTER_DEFAULT`.
const EXIT_CONDITION_DEFER_TASK: c_int = 1;
const EXIT_CONDITION_DEFAULT: c_int = 999;
const EXIT_SLOTS: usize = 12;

#[derive(Clone, Copy)]
enum End {
    Call(EndFn),
    /// Runs `defer_tasks`.
    Defer,
    /// Runs `idle_task`.
    Idle,
    /// `swoole_signal_dispatch`.
    Signal,
    /// Stops the loop once `wait_exit` is set and `if_exit` agrees.
    TryExit,
    /// `malloc_trim` at most once a `SW_MALLOC_TRIM_INTERVAL`.
    #[cfg(all(target_os = "linux", target_env = "gnu"))]
    MallocTrim,
}

#[derive(Clone, Copy)]
enum Exit {
    Call(ExitFn),
    /// No task is deferred.
    Defer,
    /// No socket is registered.
    Default,
}

#[derive(Default)]
pub(crate) struct Callbacks {
    end: [Option<End>; END_SLOTS],
    exit: [Option<Exit>; EXIT_SLOTS],
}

fn end_slot(key: c_int) -> Option<usize> {
    usize::try_from(key).ok().filter(|&slot| slot < END_SLOTS)
}

fn exit_slot(key: c_int) -> Option<usize> {
    match key {
        0..=9 => usize::try_from(key).ok(),
        999 => Some(10),
        1000 => Some(11),
        _ => None,
    }
}

impl Callbacks {
    fn set_end(&mut self, key: c_int, end: Option<End>) {
        let slot = end_slot(key).and_then(|slot| self.end.get_mut(slot));
        debug_assert!(slot.is_some(), "end callback {key}");
        if let Some(slot) = slot {
            *slot = end;
        }
    }

    fn set_exit(&mut self, key: c_int, exit: Option<Exit>) -> bool {
        let slot = exit_slot(key).and_then(|slot| self.exit.get_mut(slot));
        debug_assert!(slot.is_some(), "exit condition {key}");
        slot.and_then(|slot| std::mem::replace(slot, exit))
            .is_some()
    }

    fn has_exit(&self, key: c_int) -> bool {
        exit_slot(key)
            .and_then(|slot| self.exit.get(slot))
            .is_some_and(Option::is_some)
    }
}

/// Installs the built-ins upstream's constructor sets, after its create hook.
pub(crate) fn install_builtins(callbacks: &mut Callbacks) {
    callbacks.set_end(PRIORITY_DEFER_TASK, Some(End::Defer));
    callbacks.set_exit(EXIT_CONDITION_DEFER_TASK, Some(Exit::Defer));
    callbacks.set_end(PRIORITY_IDLE_TASK, Some(End::Idle));
    if swoole_rs_corebase::swoole_is_main_thread() {
        callbacks.set_end(PRIORITY_SIGNAL_CALLBACK, Some(End::Signal));
    }
    callbacks.set_end(PRIORITY_TRY_EXIT, Some(End::TryExit));
    #[cfg(all(target_os = "linux", target_env = "gnu"))]
    if swoole_rs_corebase::swoole_is_main_thread() {
        callbacks.set_end(PRIORITY_MALLOC_TRIM, Some(End::MallocTrim));
    }
    callbacks.set_exit(EXIT_CONDITION_DEFAULT, Some(Exit::Default));
}

/// `Reactor::execute_end_callbacks(timed_out)`. A callback may change the reactor, which
/// no reference to its state is held across.
pub(crate) fn execute_end(reactor: Running<'_>, timed_out: bool) {
    reactor.with(|state| state.fields.timed_out = timed_out);
    for slot in 0..END_SLOTS {
        // Each borrow ends, its copy read, before a callback runs.
        let Some(end) = reactor.with(|state| state.callbacks.end.get(slot).copied().flatten())
        else {
            continue;
        };
        match end {
            End::Call(call) => call(),
            End::Defer => crate::deferred::run_defer(reactor),
            End::Idle => run_task(reactor.with(|state| state.fields.idle_task)),
            End::Signal => swoole_rs_process::signal::swoole_signal_dispatch(),
            End::TryExit => {
                if reactor.with(|state| state.fields.wait_exit) && if_exit(reactor) {
                    reactor.with(|state| state.fields.running = false);
                }
            }
            #[cfg(all(target_os = "linux", target_env = "gnu"))]
            End::MallocTrim => {
                reactor.with(|state| malloc_trim(&mut state.fields.last_malloc_trim_time));
            }
        }
    }
}

/// `Swoole\Event::cycle`'s idle or future task, when set.
fn run_task(task: crate::ReactorTask) {
    if let Some(callback) = task.callback {
        // SAFETY: the task's callback and the data it was set with.
        unsafe { callback(task.data) };
    }
}

/// `malloc_trim` at most once a `SW_MALLOC_TRIM_INTERVAL`.
#[cfg(all(target_os = "linux", target_env = "gnu"))]
fn malloc_trim(last: &mut libc::time_t) {
    use swoole_rs_corebase::constants::{SW_MALLOC_TRIM_INTERVAL, SW_MALLOC_TRIM_PAD};
    // `time(nullptr)`, as upstream: a third of `SystemTime::now` and
    // `duration_since`, at the end of every wait.
    let now = swoole_rs_corebase::time_now();
    if *last < now - libc::time_t::from(SW_MALLOC_TRIM_INTERVAL) {
        // SAFETY: glibc's heap trim, with no preconditions.
        unsafe { libc::malloc_trim(usize::try_from(SW_MALLOC_TRIM_PAD).unwrap_or(0)) };
        *last = now;
    }
}

/// `Reactor::if_exit()`: each exit condition in key order over the registered socket
/// count, until one refuses. A condition may change the reactor, as [`execute_end`]'s.
pub(crate) fn if_exit(reactor: Running<'_>) -> bool {
    let mut event_num = reactor.registry().count();
    for slot in 0..EXIT_SLOTS {
        // As above: the copy is read before the condition runs.
        let Some(exit) = reactor.with(|state| state.callbacks.exit.get(slot).copied().flatten())
        else {
            continue;
        };
        let accepted = match exit {
            Exit::Call(call) => call(&mut event_num),
            Exit::Defer => !reactor.has_deferred(),
            Exit::Default => event_num == 0,
        };
        if !accepted {
            return false;
        }
    }
    true
}

/// `Reactor::set_end_callback(id, callback)`, or `erase_end_callback(id)` without one.
#[unsafe(no_mangle)]
pub extern "C" fn swr_v1_reactor_set_end(
    owner: &mut ReactorState,
    id: c_int,
    callback: Option<EndFn>,
) {
    owner.callbacks.set_end(id, callback.map(End::Call));
}

/// `Reactor::set_exit_condition(id, condition)`, or `remove_exit_condition(id)` without
/// one: whether one was set before.
#[unsafe(no_mangle)]
pub extern "C" fn swr_v1_reactor_set_exit(
    owner: &mut ReactorState,
    id: c_int,
    condition: Option<ExitFn>,
) -> bool {
    owner.callbacks.set_exit(id, condition.map(Exit::Call))
}

/// `Reactor::isset_exit_condition(id)`.
#[unsafe(no_mangle)]
pub extern "C" fn swr_v1_reactor_isset_exit(owner: &ReactorState, id: c_int) -> bool {
    owner.callbacks.has_exit(id)
}

/// `sw_reactor()->set_end_callback(id, callback)`.
pub fn swr_v1_event_set_end_callback(id: c_int, callback: EndFn) {
    if let Some(reactor) = running() {
        reactor.with(|owner| owner.callbacks.set_end(id, Some(End::Call(callback))));
    }
}

/// `sw_reactor()->erase_end_callback(id)`.
pub fn swr_v1_event_erase_end_callback(id: c_int) {
    if let Some(reactor) = running() {
        reactor.with(|owner| owner.callbacks.set_end(id, None));
    }
}

/// `sw_reactor()->set_exit_condition(id, condition)`.
pub fn swr_v1_event_set_exit_condition(id: c_int, condition: ExitFn) {
    if let Some(reactor) = running() {
        reactor.with(|owner| swr_v1_reactor_set_exit(owner, id, Some(condition)));
    }
}

/// `sw_reactor()->remove_exit_condition(id)`.
pub fn swr_v1_event_remove_exit_condition(id: c_int) {
    if let Some(reactor) = running() {
        reactor.with(|owner| swr_v1_reactor_set_exit(owner, id, None));
    }
}

/// `void swoole_event_defer(void (*)(void *), void *)`: `sw_reactor()->defer`.
///
/// # Safety
/// `call` accepts `data` when the loop runs it.
#[cfg_attr(
    feature = "upstream-symbols",
    unsafe(export_name = "_Z18swoole_event_deferPFvPvES_")
)]
pub unsafe extern "C" fn swoole_event_defer(call: crate::DeferCall, data: *mut c_void) {
    if let Some(reactor) = crate::event_api::current_prepared() {
        // SAFETY: the thread's live reactor; the loop runs `call(data)` as its caller set.
        unsafe { crate::deferred::defer(reactor.fields, call, data, None) };
    }
}

/// `bool Reactor::if_exit()`.
#[cfg_attr(
    feature = "upstream-symbols",
    unsafe(export_name = "_ZN6swoole7Reactor7if_exitEv")
)]
#[must_use]
pub extern "C-unwind" fn reactor_if_exit(reactor: Running<'_>) -> bool {
    if_exit(reactor)
}

/// `int Reactor::wait()`: the loop over the state's backend, the future task before
/// each kernel wait and the end callbacks after it.
#[cfg_attr(
    feature = "upstream-symbols",
    unsafe(export_name = "_ZN6swoole7Reactor4waitEv")
)]
#[must_use]
pub extern "C-unwind" fn reactor_wait(reactor: Running<'_>) -> c_int {
    wait(reactor)
}

/// `Reactor::wait()` of the reactor `reactor` names.
pub(crate) fn wait(reactor: Running<'_>) -> c_int {
    fn begin(reactor: Running<'_>) {
        run_task(reactor.with(|state| state.fields.future_task));
    }
    let ops = crate::WaitOps {
        reactor,
        begin,
        end: execute_end,
    };
    if crate::wait_loop::wait(&ops) < 0 {
        -1
    } else {
        0
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::reactor_state::tests::running;
    use std::cell::{Cell, RefCell};

    thread_local! {
        static TRACE: RefCell<Vec<i32>> = const { RefCell::new(Vec::new()) };
        static OWNER: Cell<*mut ReactorState> = const { Cell::new(std::ptr::null_mut()) };
    }
    fn push(value: i32) {
        TRACE.with_borrow_mut(|trace| trace.push(value));
    }
    fn take() -> Vec<i32> {
        TRACE.with_borrow_mut(std::mem::take)
    }
    fn callbacks() -> &'static mut Callbacks {
        // SAFETY: the test's live state, borrowed for one change.
        unsafe { &mut (*OWNER.get()).callbacks }
    }
    fn set_end(key: c_int, call: EndFn) {
        callbacks().set_end(key, Some(End::Call(call)));
    }
    fn set_exit(key: c_int, call: ExitFn) {
        callbacks().set_exit(key, Some(Exit::Call(call)));
    }
    /// A fresh state with no callbacks, as `OWNER`.
    fn with_state(test: impl FnOnce(*mut ReactorState)) {
        let state = crate::reactor_state::tests::state(3, 1);
        OWNER.set(state.owner);
        test(state.owner);
        // SAFETY: the state the test made.
        unsafe { crate::reactor_state::tests::free(state.owner) };
        take();
    }

    extern "C-unwind" fn end0() {
        push(0);
    }
    extern "C-unwind" fn end1() {
        push(1);
    }
    extern "C-unwind" fn end2() {
        push(2);
    }
    extern "C-unwind" fn end3() {
        push(3);
    }
    extern "C-unwind" fn end5() {
        // SAFETY: the test's live state.
        assert!(unsafe { (*OWNER.get()).fields.timed_out });
        push(5);
    }
    extern "C-unwind" fn end99() {
        push(99);
    }
    /// Sets a later and an earlier key, and erases a later one.
    extern "C-unwind" fn end_mutating() {
        push(1);
        set_end(2, end2);
        set_end(0, end0);
        // SAFETY: the test's live state.
        callbacks().set_end(3, None);
    }

    /// The former callbacks gate: an empty store, end callbacks in key order with
    /// replacement and erasure.
    #[test]
    fn end_callbacks_run_in_key_order() {
        with_state(|owner| unsafe {
            assert!(if_exit(running(owner)));
            execute_end(running(owner), false);
            assert!(!(*owner).fields.timed_out);
            assert!(!swr_v1_reactor_isset_exit(&*owner, 1));
            assert!(!callbacks().set_exit(1, None));
            swr_v1_reactor_set_end(&mut *owner, 1, None);
            assert_eq!(take(), []);

            set_end(5, end5);
            set_end(1, end1);
            set_end(3, end3);
            execute_end(running(owner), true);
            assert_eq!(take(), [1, 3, 5]);
            set_end(3, end99);
            execute_end(running(owner), true);
            assert_eq!(take(), [1, 99, 5]);
            swr_v1_reactor_set_end(&mut *owner, 3, None);
            swr_v1_reactor_set_end(&mut *owner, 7, None);
            execute_end(running(owner), true);
            assert_eq!(take(), [1, 5]);
        });
    }

    /// The former callbacks gate: an end callback that adds after and before itself
    /// and erases a later one.
    #[test]
    fn end_callbacks_see_changes_after_their_key() {
        with_state(|owner| unsafe {
            set_end(1, end_mutating);
            set_end(3, end3);
            set_end(5, end99);
            execute_end(running(owner), false);
            assert_eq!(take(), [1, 2, 99]);
            assert!(!(*owner).fields.timed_out);
        });
    }

    extern "C-unwind" fn exit1(n: &mut usize) -> bool {
        push(1);
        assert_eq!(*n, 0);
        *n += 7;
        true
    }
    extern "C-unwind" fn exit2(n: &mut usize) -> bool {
        push(2);
        assert_eq!(*n, 7);
        false
    }
    extern "C-unwind" fn exit3(n: &mut usize) -> bool {
        push(3);
        assert_eq!(*n, 7);
        true
    }
    extern "C-unwind" fn refuse(_: &mut usize) -> bool {
        push(3);
        false
    }
    extern "C-unwind" fn exit_mutating(n: &mut usize) -> bool {
        push(1);
        set_exit(2, exit3);
        callbacks().set_exit(3, None);
        *n += 7;
        true
    }

    /// The former callbacks gate: exit conditions share the event count, stop at the
    /// first false, and see additions and removals made during the walk.
    #[test]
    fn exit_conditions_share_the_count_and_stop_at_a_refusal() {
        with_state(|owner| unsafe {
            set_exit(3, exit3);
            set_exit(1, exit1);
            set_exit(2, exit2);
            assert!(swr_v1_reactor_isset_exit(&*owner, 2));
            assert!(!if_exit(running(owner)));
            assert_eq!(take(), [1, 2]);
            assert!(callbacks().set_exit(2, None));
            assert!(!callbacks().set_exit(2, None));
            assert!(!swr_v1_reactor_isset_exit(&*owner, 2));
            assert!(if_exit(running(owner)));
            assert_eq!(take(), [1, 3]);
        });
        with_state(|owner| {
            set_exit(1, exit_mutating);
            set_exit(3, refuse);
            assert!(if_exit(running(owner)));
            assert_eq!(take(), [1, 3]);
        });
    }

    /// The built-ins: the deferred queue runs at the end and holds the loop open; the
    /// default condition refuses while sockets are registered; try-exit stops the loop
    /// only under `wait_exit` once every condition agrees.
    #[test]
    fn builtins_defer_idle_and_exit() {
        unsafe extern "C-unwind" fn record(data: *mut c_void) {
            push(data as i32);
        }
        with_state(|owner| unsafe {
            install_builtins(callbacks());
            let fields = &raw mut (*owner).fields;
            assert!(if_exit(running(owner)));
            crate::deferred::defer(fields, record, std::ptr::without_provenance_mut(4), None);
            assert!(!if_exit(running(owner)));
            (*fields).idle_task.callback = Some(record);
            (*fields).idle_task.data = std::ptr::without_provenance_mut(6);
            (*fields).running = true;
            execute_end(running(owner), false);
            assert_eq!(take(), [4, 6]);
            assert!((*fields).running);
            (*fields).wait_exit = true;
            execute_end(running(owner), false);
            assert!(!(*fields).running);
            (*fields).running = true;
            let mut socket = crate::SocketRecord::with_fd(3, 0);
            assert_eq!(
                crate::reactor_state::tests::record(owner, &raw mut socket, 0, 512),
                0
            );
            assert!(!if_exit(running(owner)));
            execute_end(running(owner), false);
            assert!((*fields).running);
            assert_eq!(take(), [6, 6]);
        });
    }
}
