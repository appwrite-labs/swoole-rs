// SPDX-License-Identifier: Apache-2.0
// Compatibility derived from Swoole v6.2.4 src/reactor/base.cc (Swoole contributors).
//! The reactor's deferred queues (`Reactor::defer_tasks`, `Reactor::destroy_callbacks`):
//! FIFO native `(call, data)` pairs, with a `dispose` that releases a task's data when
//! the queue is freed unrun, and Rust values ([`crate::event_api::Deferred`]).
use crate::Running;
use crate::event_api::Deferred;
use crate::reactor_state::ReactorFields;
use std::collections::VecDeque;
use std::ffi::c_void;
use std::ptr;

/// A deferred call. `dispose`, when set, releases `data` if the call never runs.
pub type DeferCall = unsafe extern "C-unwind" fn(*mut c_void);
pub type DeferDispose = DeferCall;

enum Task {
    Native {
        call: DeferCall,
        data: *mut c_void,
        dispose: Option<DeferDispose>,
    },
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
            // SAFETY: the task's own contract, from whoever queued it.
            Self::Native { call, data, .. } => unsafe { call(data) },
            Self::Value(value) => value.run(),
        }
    }

    /// Releases it unrun.
    fn dispose(self) {
        match self {
            Self::Native {
                data,
                dispose: Some(dispose),
                ..
            } => {
                // SAFETY: the task never ran, so its data is still the queue's.
                unsafe { dispose(data) };
            }
            Self::Native { .. } => {}
            Self::Value(value) => value.dispose(),
        }
    }
}

#[derive(Default)]
pub struct ReactorDeferred {
    tasks: VecDeque<Task>,
}

/// Appends `call(data)` to the queue in `slot`, creating it on first use: 0, or -2 when
/// it cannot allocate (nothing is queued and `data` stays the caller's).
///
/// # Safety
/// `slot` holds null or a queue from this module, and the loop may run `call(data)`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn swr_v1_reactor_deferred_push(
    slot: &mut *mut ReactorDeferred,
    call: DeferCall,
    data: *mut c_void,
    dispose: Option<DeferDispose>,
) -> i32 {
    push(
        slot,
        Task::Native {
            call,
            data,
            dispose,
        },
    )
}

/// Appends `task` to the queue in `slot`, creating it on first use: 0, or -2 when it
/// cannot allocate (nothing is queued). `slot` is a reactor's `defer_tasks` or
/// `destroy_callbacks`, which hold null or a queue from this module.
fn push(slot: &mut *mut ReactorDeferred, task: Task) -> i32 {
    if slot.is_null() {
        let Some(owner) = swoole_rs_buffer::try_box(ReactorDeferred::default) else {
            return -2;
        };
        *slot = Box::into_raw(owner);
    }
    // SAFETY: a live queue from this module, as above.
    let tasks = unsafe { &mut (**slot).tasks };
    if tasks.try_reserve(1).is_err() {
        return -2;
    }
    tasks.push_back(task);
    0
}

/// Appends `value` to the reactor's `defer_tasks`.
pub(crate) fn defer_value(fields: &mut ReactorFields, value: impl Deferred) {
    if push(&mut fields.defer_tasks, Task::Value(Box::new(value))) != 0 {
        std::alloc::handle_alloc_error(std::alloc::Layout::new::<ReactorDeferred>());
    }
}

/// Runs the queue's tasks in order, each popped before it is called, until it is empty;
/// tasks a call appends run in this pass too. No borrow of the queue spans a call.
///
/// # Safety
/// `owner` is null or a live queue from this module that no call frees.
pub(crate) unsafe fn deferred_run(owner: *mut ReactorDeferred) {
    if owner.is_null() {
        return;
    }
    loop {
        // SAFETY: as the caller promises; the borrow ends before the call.
        let Some(task) = (unsafe { (*owner).tasks.pop_front() }) else {
            return;
        };
        task.run();
    }
}

/// Frees the queue in `slot`, disposing of the tasks that never ran, and nulls the slot.
///
/// # Safety
/// `slot` holds null or a queue from this module, which nothing runs.
pub(crate) unsafe fn deferred_free(slot: &mut *mut ReactorDeferred) {
    let owner = std::mem::replace(slot, ptr::null_mut());
    if owner.is_null() {
        return;
    }
    // SAFETY: the queue was this module's; the slot no longer names it.
    let owner = unsafe { Box::from_raw(owner) };
    owner.tasks.into_iter().for_each(Task::dispose);
}

/// `Reactor::defer(fn, data)` on the reactor whose fields these are; `dispose(data)`
/// instead if the reactor is freed first.
///
/// # Safety
/// `fields` is a live reactor's.
pub unsafe fn defer(
    fields: *mut ReactorFields,
    call: DeferCall,
    data: *mut c_void,
    dispose: Option<DeferDispose>,
) {
    let slot = fields
        .wrapping_byte_add(std::mem::offset_of!(ReactorFields, defer_tasks))
        .cast::<*mut ReactorDeferred>();
    let task = Task::Native {
        call,
        data,
        dispose,
    };
    // SAFETY: the live reactor's queue slot.
    if push(unsafe { &mut *slot }, task) != 0 {
        std::alloc::handle_alloc_error(std::alloc::Layout::new::<ReactorDeferred>());
    }
}

/// `Reactor::add_destroy_callback(fn, data)` on the reactor whose fields these are.
///
/// # Safety
/// `call(data)` is sound when the reactor is destroyed.
pub unsafe fn defer_destroy(fields: &mut ReactorFields, call: DeferCall, data: *mut c_void) {
    push_destroy(fields, call, data);
}

/// `Reactor::add_destroy_callback(fn, nullptr)` of a `call` that is sound with any data.
pub(crate) fn defer_destroy_any(
    fields: &mut ReactorFields,
    call: extern "C-unwind" fn(*mut c_void),
) {
    push_destroy(fields, call, ptr::null_mut());
}

/// Appends `call(data)` to the destroy queue, which runs it as [`defer_destroy`] requires.
fn push_destroy(fields: &mut ReactorFields, call: DeferCall, data: *mut c_void) {
    let task = Task::Native {
        call,
        data,
        dispose: None,
    };
    if push(&mut fields.destroy_callbacks, task) != 0 {
        std::alloc::handle_alloc_error(std::alloc::Layout::new::<ReactorDeferred>());
    }
}

/// The `PRIORITY_DEFER_TASK` end callback over `Reactor::defer_tasks`: the queue is
/// detached first, so tasks it defers run on the next pass, then run and freed. While it
/// runs, the reactor's `running_defer` holds it: a task that bails out of the loop jumps
/// over this frame, and the reactor's drop frees the queue instead.
pub(crate) fn run_defer(reactor: Running<'_>) {
    // The borrow ends before a task runs.
    let queue = reactor.with(|state| {
        let queue = std::mem::replace(&mut state.fields.defer_tasks, ptr::null_mut());
        if !queue.is_null() {
            state.running_defer.push(queue);
        }
        queue
    });
    if queue.is_null() {
        return;
    }
    // SAFETY: the detached queue is ours; a task may defer onto the reactor.
    unsafe { deferred_run(queue) };
    // Every task ran, and a nested pass popped its own.
    reactor.with(|state| state.running_defer.pop());
    let mut queue = queue;
    // SAFETY: as above; the reactor no longer names it.
    unsafe { deferred_free(&mut queue) };
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::reactor_state::tests::running;
    use std::cell::RefCell;

    thread_local! {
        static TRACE: RefCell<Vec<usize>> = const { RefCell::new(Vec::new()) };
        static FIELDS: std::cell::Cell<*mut ReactorFields> = const { std::cell::Cell::new(ptr::null_mut()) };
    }

    unsafe extern "C-unwind" fn record(data: *mut c_void) {
        TRACE.with_borrow_mut(|trace| trace.push(data as usize));
    }

    unsafe extern "C-unwind" fn disposed(data: *mut c_void) {
        TRACE.with_borrow_mut(|trace| trace.push(1000 + data as usize));
    }

    /// Defers `record(4)` from inside a deferred task.
    unsafe extern "C-unwind" fn defer_again(data: *mut c_void) {
        TRACE.with_borrow_mut(|trace| trace.push(data as usize));
        // SAFETY: the test's live fields; the running queue is detached.
        unsafe {
            assert!((*FIELDS.get()).defer_tasks.is_null());
            defer(FIELDS.get(), record, token(4), None);
        }
    }

    fn take() -> Vec<usize> {
        TRACE.with_borrow_mut(std::mem::take)
    }

    fn token(value: usize) -> *mut c_void {
        ptr::without_provenance_mut(value)
    }

    /// base.cc's `Reactor::defer` and the defer end callback: FIFO, a task's own defer
    /// lands on the next pass, and the queue is gone once run.
    #[test]
    fn deferred_tasks_run_in_order_and_reentrant_defers_wait_a_pass() {
        let state = crate::reactor_state::tests::state(3, 1);
        let fields = state.fields;
        FIELDS.set(fields);
        unsafe {
            assert!((*fields).defer_tasks.is_null());
            defer(fields, record, token(1), None);
            defer(fields, record, token(2), None);
            assert!(!(*fields).defer_tasks.is_null());
            run_defer(running(state.owner));
            assert_eq!(take(), [1, 2]);
            assert!((*fields).defer_tasks.is_null());
            defer(fields, defer_again, token(3), None);
            run_defer(running(state.owner));
            assert_eq!(take(), [3]);
            assert!(!(*fields).defer_tasks.is_null());
            run_defer(running(state.owner));
            assert_eq!(take(), [4]);
            assert!((*fields).defer_tasks.is_null());
            run_defer(running(state.owner));
            assert_eq!(take(), []);
            crate::reactor_state::tests::free(state.owner);
        }
    }

    /// Stands for a task that bails out of the loop (a `longjmp` in PHP): it never returns.
    unsafe extern "C-unwind" fn escape(data: *mut c_void) {
        TRACE.with_borrow_mut(|trace| trace.push(data as usize));
        panic!("escape");
    }

    /// A task that leaves the loop leaves its queue to the reactor, whose drop frees it
    /// and disposes of the tasks that never ran.
    #[test]
    fn a_queue_left_running_is_freed_with_the_reactor() {
        let state = crate::reactor_state::tests::state(3, 1);
        let fields = state.fields;
        unsafe {
            defer(fields, record, token(1), None);
            defer(fields, escape, token(2), None);
            let slot = &raw mut (*fields).defer_tasks;
            assert_eq!(
                swr_v1_reactor_deferred_push(&mut *slot, record, token(3), Some(disposed)),
                0
            );
            let owner = std::panic::AssertUnwindSafe(state.owner);
            assert!(std::panic::catch_unwind(move || run_defer(running(*owner))).is_err());
            assert_eq!(take(), [1, 2]);
            assert!((*fields).defer_tasks.is_null());
            assert_eq!((*state.owner).running_defer.len(), 1);
            crate::reactor_state::tests::free(state.owner);
            assert_eq!(take(), [1003]);
        }
    }

    /// The former `callback_queues` gate (`add_destroy_callback` over the destroy queue):
    /// for 0, 1, 17 and 65 callbacks nothing runs until the queue does, then all run in
    /// order; a second run is empty, and one added afterwards runs on the next.
    #[test]
    fn destroy_queue_runs_in_order_and_again_after_an_append() {
        for count in [0usize, 1, 17, 65] {
            let mut queue = ptr::null_mut();
            unsafe {
                for i in 0..count {
                    assert_eq!(
                        swr_v1_reactor_deferred_push(&mut queue, record, token(i), None),
                        0
                    );
                }
                assert_eq!(take(), []);
                deferred_run(queue);
                assert_eq!(take(), (0..count).collect::<Vec<_>>());
                deferred_run(queue);
                assert_eq!(take(), []);
                swr_v1_reactor_deferred_push(&mut queue, record, token(99), None);
                deferred_run(queue);
                assert_eq!(take(), [99]);
                deferred_free(&mut queue);
                assert!(queue.is_null());
            }
        }
    }

    /// Disposes of its data, then defers `record(5)` (disposed as 1005), the way a PHP
    /// destructor run by a disposer can call `Event::defer`.
    unsafe extern "C-unwind" fn dispose_and_defer(data: *mut c_void) {
        TRACE.with_borrow_mut(|trace| trace.push(1000 + data as usize));
        // SAFETY: the test's reactor, still being dropped.
        unsafe { defer(FIELDS.get(), record, token(5), Some(disposed)) };
    }

    /// A task deferred while the reactor's drop disposes of the queue is disposed of too,
    /// not left in a queue nothing frees.
    #[test]
    fn a_task_deferred_while_the_reactor_drops_is_disposed() {
        let state = crate::reactor_state::tests::state(3, 1);
        let fields = state.fields;
        FIELDS.set(fields);
        unsafe {
            defer(fields, record, token(1), Some(dispose_and_defer));
            crate::reactor_state::tests::free(state.owner);
        }
        assert_eq!(take(), [1001, 1005]);
    }

    /// Freeing a queue disposes of each unrun task's data instead of calling it.
    #[test]
    fn free_disposes_unrun_tasks() {
        let mut queue = ptr::null_mut();
        unsafe {
            swr_v1_reactor_deferred_push(&mut queue, record, token(99), Some(disposed));
            swr_v1_reactor_deferred_push(&mut queue, record, token(7), None);
            deferred_free(&mut queue);
            assert_eq!(take(), [1099]);
            assert!(queue.is_null());
            deferred_free(&mut queue);
        }
    }

    /// A Rust value, traced as 100 + its number when run and 2000 + it when disposed.
    struct Value(usize);

    impl Deferred for Value {
        fn run(self) {
            TRACE.with_borrow_mut(|trace| trace.push(100 + self.0));
        }

        fn dispose(self) {
            TRACE.with_borrow_mut(|trace| trace.push(2000 + self.0));
        }
    }

    /// Rust values share the native tasks' FIFO, and one never run is disposed of
    /// with the reactor.
    #[test]
    fn values_run_in_order_with_native_tasks_and_are_disposed_unrun() {
        let state = crate::reactor_state::tests::state(3, 1);
        let fields = state.fields;
        unsafe {
            defer(fields, record, token(1), None);
            defer_value(&mut *fields, Value(2));
            defer(fields, record, token(3), None);
            run_defer(running(state.owner));
            assert_eq!(take(), [1, 102, 3]);
            defer_value(&mut *fields, Value(4));
            crate::reactor_state::tests::free(state.owner);
            assert_eq!(take(), [2004]);
        }
    }
}
