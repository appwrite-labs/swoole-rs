// SPDX-License-Identifier: Apache-2.0
// Compatibility derived from Swoole v6.2.4 src/reactor/base.cc (Swoole contributors).
//! The reactor's deferred queues (`Reactor::defer_tasks`, `Reactor::destroy_callbacks`):
//! FIFO Rust values ([`crate::event_api::Deferred`]), each run once or, when the queue
//! is freed unrun, disposed of.
use crate::Running;
use crate::event_api::Deferred;
use crate::reactor_state::ReactorFields;
use std::collections::VecDeque;

enum Task {
    /// A destroy callback that takes nothing ([`crate::event_api::on_destroy`]).
    Call(fn()),
    /// base.cc's `socket_free_defer` of a record the task owns: closed and freed when
    /// it runs, or when the queue is freed unrun.
    Free(crate::OwnedSocket),
    Value(Box<dyn Erased>),
}

/// A [`Deferred`] value of any type.
trait Erased {
    fn run(self: Box<Self>);
    fn dispose(self: Box<Self>);
}

/// The value out of its box, which is freed before the caller goes on: `run` may
/// bail out of the loop and never return.
#[expect(
    clippy::boxed_local,
    reason = "takes the box to free it on return, before `run` starts"
)]
fn unbox<T>(value: Box<T>) -> T {
    *value
}

impl<T: Deferred> Erased for T {
    fn run(self: Box<Self>) {
        T::run(unbox(self));
    }

    fn dispose(self: Box<Self>) {
        T::dispose(unbox(self));
    }
}

impl Task {
    /// Runs it; the queue no longer holds it.
    fn run(self) {
        match self {
            Self::Call(call) => call(),
            Self::Free(socket) => socket.into_box().close_and_free(),
            Self::Value(value) => value.run(),
        }
    }

    /// Releases it unrun.
    fn dispose(self) {
        match self {
            Self::Call(_) => {}
            Self::Free(socket) => socket.into_box().close_and_free(),
            Self::Value(value) => value.dispose(),
        }
    }
}

#[derive(Default)]
pub struct ReactorDeferred(Queue);

#[derive(Default)]
enum Queue {
    #[default]
    Empty,
    // Most passes defer one socket release: keep it inside the reactor.
    One(Task),
    Many(VecDeque<Task>),
}

impl ReactorDeferred {
    /// Pop before calling: a destroy callback may append to this same queue.
    fn pop(&mut self) -> Option<Task> {
        match &mut self.0 {
            Queue::Many(tasks) => tasks.pop_front(),
            _ => match std::mem::take(&mut self.0) {
                Queue::One(task) => Some(task),
                _ => None,
            },
        }
    }
}

/// Appends `task` to the queue in `slot` (a reactor's `defer_tasks` or
/// `destroy_callbacks`), creating it on first use.
fn push(slot: &mut Option<ReactorDeferred>, task: Task) {
    let queue = &mut slot.get_or_insert_default().0;
    *queue = match std::mem::take(queue) {
        Queue::Empty => Queue::One(task),
        Queue::One(first) => Queue::Many([first, task].into()),
        Queue::Many(mut tasks) => {
            tasks.push_back(task);
            Queue::Many(tasks)
        }
    };
}

/// Appends `value` to the reactor's `defer_tasks`.
pub(crate) fn defer_value(fields: &mut ReactorFields, value: impl Deferred) {
    push(&mut fields.defer_tasks, Task::Value(Box::new(value)));
}

/// Frees the queue in `slot`, disposing of the tasks that never ran, and empties the
/// slot. The slot owns its queue, so nothing runs it meanwhile.
pub(crate) fn deferred_free(slot: &mut Option<ReactorDeferred>) {
    if let Some(owner) = slot.take() {
        match owner.0 {
            Queue::Empty => {}
            Queue::One(task) => task.dispose(),
            Queue::Many(tasks) => tasks.into_iter().for_each(Task::dispose),
        }
    }
}

/// Runs the reactor's `destroy_callbacks` in order, each popped before it is called,
/// until there are none; callbacks a call adds run in this pass too. No borrow of the
/// reactor spans a call.
pub(crate) fn run_destroy(reactor: Running<'_>) {
    let next = |state: &mut crate::ReactorState| {
        let queue = state.fields.destroy_callbacks.as_mut()?;
        queue.pop()
    };
    while let Some(task) = reactor.with(next) {
        task.run();
    }
}

/// `swoole_event_defer(socket_free_defer, socket)`: the thread's reactor closes and
/// frees `socket`'s record after the current pass, or as the reactor is freed first.
/// The loop may still meet the record through an event of this pass, so it stays
/// until then.
/// # Errors
/// There is no reactor: `socket` comes back.
pub fn defer_free(socket: crate::OwnedSocket) -> Result<(), crate::OwnedSocket> {
    let Some(reactor) = crate::event_api::running() else {
        return Err(socket);
    };
    reactor.with(|state| push(&mut state.fields.defer_tasks, Task::Free(socket)));
    Ok(())
}

/// `reactor->add_destroy_callback(...)` of `value`: [`Deferred::run`] as the reactor
/// is destroyed, or [`Deferred::dispose`] if it is queued after that pass.
pub fn defer_destroy_value(reactor: crate::Running<'_>, value: impl Deferred) {
    // The borrow only queues the boxed value.
    let task = Task::Value(Box::new(value));
    reactor.with(|state| push(&mut state.fields.destroy_callbacks, task));
}

/// `Reactor::add_destroy_callback(fn, nullptr)` of a `call` that takes nothing.
pub(crate) fn defer_destroy_call(fields: &mut ReactorFields, call: fn()) {
    push(&mut fields.destroy_callbacks, Task::Call(call));
}

/// The `PRIORITY_DEFER_TASK` end callback over `Reactor::defer_tasks`: the queue is
/// detached first, so tasks it defers run on the next pass, then run and freed. While it
/// runs, the reactor's `running_defer` holds it: a task that bails out of the loop jumps
/// over this frame, and the reactor's drop frees the queue instead.
pub(crate) fn run_defer(reactor: Running<'_>) {
    if reactor.with(|state| state.fields.defer_tasks.is_some()) {
        run_queue(reactor);
    }
}

// Keep the queue's stack frame off empty passes through the end callback.
#[inline(never)]
fn run_queue(reactor: Running<'_>) {
    let index = reactor.with(|state| {
        let queue = state.fields.defer_tasks.take()?;
        let index = state.running_defer.len();
        state.running_defer.push(queue);
        Some(index)
    });
    let Some(index) = index else {
        return;
    };
    // Reborrow after each call: a nested pass can move the vector's storage.
    while let Some(task) = reactor.with(|state| state.running_defer.get_mut(index)?.pop()) {
        task.run();
    }
    // Keep any queues abandoned by a nested bailout for the reactor's drop.
    let mut queue = Some(reactor.with(|state| state.running_defer.remove(index)));
    deferred_free(&mut queue);
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use crate::reactor_state::tests::running;
    use std::cell::{Cell, RefCell};
    use std::ptr;

    thread_local! {
        static TRACE: RefCell<Vec<usize>> = const { RefCell::new(Vec::new()) };
        static FIELDS: Cell<*mut ReactorFields> = const { Cell::new(ptr::null_mut()) };
    }

    fn trace(n: usize) {
        TRACE.with_borrow_mut(|trace| trace.push(n));
    }

    fn take() -> Vec<usize> {
        TRACE.with_borrow_mut(std::mem::take)
    }

    /// A task traced as its number when run, then `then`; when disposed of unrun,
    /// `disposed` (if any) gets the number.
    #[derive(Clone, Copy)]
    pub(crate) struct Traced {
        n: usize,
        then: fn(),
        disposed: Option<fn(usize)>,
    }

    impl Deferred for Traced {
        fn run(self) {
            trace(self.n);
            (self.then)();
        }

        fn dispose(self) {
            if let Some(disposed) = self.disposed {
                disposed(self.n);
            }
        }
    }

    /// Traces `n` when run, nothing when disposed of.
    pub(crate) fn task(n: usize) -> Traced {
        Traced {
            n,
            then: || {},
            disposed: None,
        }
    }

    impl Traced {
        pub(crate) fn then(self, then: fn()) -> Self {
            Self { then, ..self }
        }

        /// Also traced, as 1000 + its number, when disposed of.
        fn disposed(self) -> Self {
            Self {
                disposed: Some(|n| trace(1000 + n)),
                ..self
            }
        }
    }

    /// `Reactor::defer` of `task` onto `fields`' queue rather than the thread's reactor.
    pub(crate) unsafe fn defer(fields: *mut ReactorFields, task: Traced) {
        defer_value(unsafe { &mut *fields }, task);
    }

    fn push_task(slot: &mut Option<ReactorDeferred>, task: Traced) {
        push(slot, Task::Value(Box::new(task)));
    }

    /// Defers task 4 from inside a deferred task.
    fn defer_again() {
        // SAFETY: the test's live fields; the running queue is detached.
        unsafe {
            assert!((*FIELDS.get()).defer_tasks.is_none());
            defer(FIELDS.get(), task(4));
        }
    }

    /// base.cc's `Reactor::defer` and the defer end callback: FIFO, a task's own defer
    /// lands on the next pass, and the queue is gone once run.
    #[test]
    fn deferred_tasks_run_in_order_and_reentrant_defers_wait_a_pass() {
        let state = crate::reactor_state::tests::state(3, 1);
        let fields = state.fields;
        FIELDS.set(fields);
        unsafe {
            assert!((*fields).defer_tasks.is_none());
            defer(fields, task(1));
            defer(fields, task(2));
            assert!((*fields).defer_tasks.is_some());
            run_defer(running(state.owner));
            assert_eq!(take(), [1, 2]);
            assert!((*fields).defer_tasks.is_none());
            defer(fields, task(3).then(defer_again));
            run_defer(running(state.owner));
            assert_eq!(take(), [3]);
            assert!((*fields).defer_tasks.is_some());
            run_defer(running(state.owner));
            assert_eq!(take(), [4]);
            assert!((*fields).defer_tasks.is_none());
            run_defer(running(state.owner));
            assert_eq!(take(), []);
            crate::reactor_state::tests::free(state.owner);
        }
    }

    /// Defers task 3 and runs a pass of its own (a nested loop) from inside a task.
    fn nest() {
        // SAFETY: the test's live reactor, which no reference holds.
        unsafe {
            defer(FIELDS.get(), task(3));
            run_defer(running((*FIELDS.get()).ptr.cast()));
        }
    }

    /// A pass a task starts runs the queue deferred since and pops only its own; the
    /// outer pass then goes on with its queue.
    #[test]
    fn a_nested_pass_leaves_the_outer_queue_running() {
        let state = crate::reactor_state::tests::state(3, 1);
        let fields = state.fields;
        FIELDS.set(fields);
        unsafe {
            (*fields).ptr = state.owner.cast();
            defer(fields, task(1).then(nest));
            defer(fields, task(2));
            run_defer(running(state.owner));
            assert_eq!(take(), [1, 3, 2]);
            assert!((*fields).defer_tasks.is_none());
            assert!((*state.owner).running_defer.is_empty());
            crate::reactor_state::tests::free(state.owner);
        }
    }

    /// Stands for a task that bails out of the loop (a `longjmp` in PHP): it never returns.
    fn escape() {
        panic!("escape");
    }

    /// A task that leaves the loop leaves its queue to the reactor, whose drop frees it
    /// and disposes of the tasks that never ran.
    #[test]
    fn a_queue_left_running_is_freed_with_the_reactor() {
        let state = crate::reactor_state::tests::state(3, 1);
        let fields = state.fields;
        unsafe {
            defer(fields, task(1));
            defer(fields, task(2).then(escape));
            defer(fields, task(3).disposed());
            let owner = std::panic::AssertUnwindSafe(state.owner);
            assert!(std::panic::catch_unwind(move || run_defer(running(*owner))).is_err());
            assert_eq!(take(), [1, 2]);
            assert!((*fields).defer_tasks.is_none());
            assert_eq!((*state.owner).running_defer.len(), 1);
            crate::reactor_state::tests::free(state.owner);
            assert_eq!(take(), [1003]);
        }
    }

    fn catch_nested_escape() {
        unsafe {
            defer(FIELDS.get(), task(3).then(escape));
            defer(FIELDS.get(), task(4).disposed());
            let owner = std::panic::AssertUnwindSafe((*FIELDS.get()).ptr.cast());
            assert!(std::panic::catch_unwind(move || run_defer(running(*owner))).is_err());
        }
    }

    #[test]
    fn a_caught_nested_escape_preserves_both_queues() {
        let state = crate::reactor_state::tests::state(3, 1);
        FIELDS.set(state.fields);
        unsafe {
            (*state.fields).ptr = state.owner.cast();
            defer(state.fields, task(1).then(catch_nested_escape));
            defer(state.fields, task(2));
            run_defer(running(state.owner));
            assert_eq!(take(), [1, 3, 2]);
            crate::reactor_state::tests::free(state.owner);
            assert_eq!(take(), [1004]);
        }
    }

    /// The former `callback_queues` gate (`add_destroy_callback` over the destroy queue):
    /// for 0, 1, 17 and 65 callbacks nothing runs until the queue does, then all run in
    /// order; a second run is empty, and one added afterwards runs on the next.
    #[test]
    fn destroy_queue_runs_in_order_and_again_after_an_append() {
        for count in [0usize, 1, 17, 65] {
            let state = crate::reactor_state::tests::state(3, 1);
            let reactor = running(state.owner);
            unsafe {
                let queue = &raw mut (*state.fields).destroy_callbacks;
                for i in 0..count {
                    push_task(&mut *queue, task(i));
                }
                assert_eq!(take(), []);
                run_destroy(reactor);
                assert_eq!(take(), (0..count).collect::<Vec<_>>());
                run_destroy(reactor);
                assert_eq!(take(), []);
                push_task(&mut *queue, task(99));
                run_destroy(reactor);
                assert_eq!(take(), [99]);
                deferred_free(&mut *queue);
                assert!((*queue).is_none());
                crate::reactor_state::tests::free(state.owner);
            }
        }
    }

    fn append_destroy() {
        unsafe { push_task(&mut (*FIELDS.get()).destroy_callbacks, task(3)) };
    }

    #[test]
    fn a_destroy_callback_appends_after_pending_callbacks() {
        let state = crate::reactor_state::tests::state(3, 1);
        FIELDS.set(state.fields);
        unsafe {
            let queue = &mut (*state.fields).destroy_callbacks;
            push_task(queue, task(1).then(append_destroy));
            push_task(queue, task(2));
            run_destroy(running(state.owner));
            assert_eq!(take(), [1, 2, 3]);
            crate::reactor_state::tests::free(state.owner);
        }
    }

    /// A task deferred while the reactor's drop disposes of the queue is disposed of too,
    /// not left in a queue nothing frees: task 1's disposal (traced as 1001) defers task
    /// 5 (disposed as 1005), the way a PHP destructor run by a disposer can call
    /// `Event::defer`.
    #[test]
    fn a_task_deferred_while_the_reactor_drops_is_disposed() {
        let state = crate::reactor_state::tests::state(3, 1);
        let fields = state.fields;
        FIELDS.set(fields);
        let dispose_and_defer = |n| {
            trace(1000 + n);
            // SAFETY: the test's reactor, still being dropped.
            unsafe { defer(FIELDS.get(), task(5).disposed()) };
        };
        let first = Traced {
            disposed: Some(dispose_and_defer),
            ..task(1)
        };
        unsafe {
            defer(fields, first);
            crate::reactor_state::tests::free(state.owner);
        }
        assert_eq!(take(), [1001, 1005]);
    }

    /// Freeing a queue disposes of each unrun task instead of running it.
    #[test]
    fn free_disposes_unrun_tasks() {
        let mut queue = None;
        push_task(&mut queue, task(99).disposed());
        push_task(&mut queue, task(7));
        deferred_free(&mut queue);
        assert_eq!(take(), [1099]);
        assert!(queue.is_none());
        deferred_free(&mut queue);
    }
}
