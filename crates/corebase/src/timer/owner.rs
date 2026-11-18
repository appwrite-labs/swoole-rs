// SPDX-License-Identifier: Apache-2.0
// Compatibility derived from Swoole v6.2.4 src/core/timer.cc (Swoole contributors).
//! `swoole::Timer` (upstream `src/core/timer.cc`, which the timer patch deletes): a
//! Rust-owned record over the queue and its nodes, accessed by typed Rust paths.
use crate::CCast;
use crate::reactor::{event_api, swr_v1_event_set_end_callback, swr_v1_event_set_exit_condition};
use crate::timer::{
    Status, Timer as Queue, TimerView, absolute_msec, swr_v1_timer_backend,
    swr_v1_timer_delay_deadline, swr_v1_timer_rearm_after_add, swr_v1_timer_relative_msec,
    swr_v1_timer_system_set,
};
use crate::{log, swr_v1_core_set_signal_alarm, thread_reactor};
use std::cell::Cell;
use std::ffi::{CStr, c_int, c_long, c_void};

/// `SW_LOG_WARNING` and `SW_ERROR_INVALID_PARAMS`.
const INVALID_PARAMS: c_int = swoole_rs_error::ErrorCode::SW_ERROR_INVALID_PARAMS as c_int;
/// `swr_v1_timer_backend`'s modes.
const MANUAL: u32 = 0;
const REACTOR: u32 = 1;

/// A node's fields, for `Swoole\Timer::info()`.
#[derive(Debug, Clone, Copy)]
pub struct Info {
    pub exec_msec: i64,
    pub exec_count: u64,
    pub interval: i64,
    pub round: u64,
    pub removed: bool,
}

/// A Rust-owned timer node; callers retain issued IDs instead of its layout. Its
/// metadata (deadline, counts, removal, kind) lives in the queue's view of it.
pub(crate) struct TimerNode {
    retained: SmallFn<()>,
}

use crate::log::LogLevel::SW_LOG_WARNING as LOG_WARNING;
use crate::small_fn::{Small, SmallFn};

/// A callback held in its node, dropped with it: once the node fires for the last
/// time, is deleted, or its timer is freed. A node removed while its callback runs
/// is released only after the callback returns.
struct Inline<F>(F);

impl<F: Fn() + 'static> Small<()> for Inline<F> {
    fn run(&self) {
        (self.0)();
    }
    fn consume(self) {}
}

/// A [`crate::timer::php::Callback`], released when its node goes.
struct Retain<C>(C);

impl<C: crate::timer::php::Callback> Small<()> for Retain<C> {
    fn run(&self) {
        self.0.run();
    }
    fn consume(self) {
        self.0.release();
    }
}

impl TimerNode {
    pub(crate) fn new() -> Self {
        Self {
            retained: SmallFn::empty(),
        }
    }

    /// A node whose callback is `f`, which may own what it captures, at most two words.
    pub(crate) fn closure<F: Fn() + 'static>(f: F) -> Self {
        let node = Self::new();
        node.retained.put(Inline(f));
        node
    }

    /// Install into an empty reservation; a full one aborts.
    pub(crate) fn install<C: crate::timer::php::Callback>(&self, callback: C) {
        self.retained.put(Retain(callback));
    }
}

/// Releases the node's callback: once the node is detached and freed, or refused.
impl Drop for TimerNode {
    fn drop(&mut self) {
        if let Some(callback) = self.retained.take() {
            callback.consume();
        }
    }
}

/// A queue status that only a broken invariant returns (upstream threw).
fn check(status: Status) {
    if status != Status::Ok {
        crate::fatal("a timer queue operation failed");
    }
}

fn warning(origin: &CStr, message: std::fmt::Arguments<'_>) {
    log::error_log(LOG_WARNING, INVALID_PARAMS, origin, message);
}

/// The system timer's `SIGALRM` handler.
extern "C" fn alarm(_signo: c_int) {
    swr_v1_core_set_signal_alarm(true);
}

/// `swoole::Timer`. Callbacks re-enter it, so every mutable field is a `Cell`, and the
/// queue a boxed one that [`Owner::lend`] takes out only for a queue operation, never
/// across a callback or destructor.
pub struct Owner {
    reactor: Cell<*mut c_void>,
    queue: Cell<Option<Box<Queue>>>,
    owner_pid: libc::pid_t,
    backend: Cell<u32>,
    abandoned: Cell<bool>,
    base_time: i64,
    next_msec: Cell<i64>,
    /// Which timer this is, for its [`crate::timer::Held`] ids: never 0.
    generation: u64,
}

/// The generation the last timer made took.
static GENERATIONS: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

impl Owner {
    /// `new Timer(manually_trigger)`.
    pub(crate) fn create(manually_trigger: bool) -> *mut Owner {
        let timer = Box::new(Owner {
            reactor: Cell::new(std::ptr::null_mut()),
            queue: Cell::new(Some(Box::default())),
            owner_pid: std::process::id().cast_signed(),
            backend: Cell::new(MANUAL),
            abandoned: Cell::new(false),
            base_time: absolute_msec(),
            next_msec: Cell::new(-1),
            generation: next_generation(),
        });
        timer.init(manually_trigger);
        Box::into_raw(timer)
    }

    pub(crate) fn relative_msec(&self) -> i64 {
        swr_v1_timer_relative_msec(absolute_msec(), self.base_time)
    }

    pub(crate) fn next_msec(&self) -> i64 {
        self.next_msec.get()
    }

    pub(crate) fn generation(&self) -> u16 {
        self.generation.c_cast::<u16>()
    }

    pub(crate) fn identity(&self) -> u64 {
        self.generation
    }

    /// `f` over the queue, taken out for the call and put back after it. `f` is a
    /// queue operation alone: nothing it calls reaches this timer, so a second take,
    /// which finds no queue, is a broken invariant.
    #[inline]
    fn lend<R>(&self, f: impl FnOnce(&mut Queue) -> R) -> R {
        let Some(mut queue) = self.queue.take() else {
            lent();
        };
        let out = f(&mut queue);
        let nested = self.queue.replace(Some(queue));
        debug_assert!(nested.is_none(), "a nested lend put a queue back");
        // Only `lend` writes the cell after `create`, and a nested one stops at its
        // take, so this is the `None` the take left. Dropping it (`set`, or a check
        // here) joins every path of `f` at the put-back, which cost the hot paths
        // more than the borrow flag did.
        #[expect(clippy::mem_forget, reason = "the `None` the take left")]
        std::mem::forget(nested);
        out
    }

    pub(crate) fn count(&self) -> usize {
        self.lend(|queue| queue.heap.len())
    }

    /// `Timer::get_round()`.
    pub(crate) fn round(&self) -> u64 {
        self.lend(|queue| queue.round)
    }

    /// `Timer::get_ids()`, in heap order.
    pub(crate) fn ids(&self) -> Vec<i64> {
        self.lend(|queue| queue.heap.iter().map(|view| view.id).collect())
    }

    /// Node `id`'s fields, for `Swoole\Timer::info()`, and whether it is a PHP
    /// callable's; none for a node not queued.
    pub(crate) fn info(&self, id: c_long) -> Option<(Info, bool)> {
        let view = self.lend(|queue| queue.get(id)).ok()?;
        let info = Info {
            exec_msec: view.deadline,
            exec_count: view.exec_count,
            interval: view.interval,
            round: view.birth_round,
            removed: view.removed,
        };
        Some((info, view.php))
    }

    /// `Timer::get(id)`, borrowed.
    pub(crate) fn get(&self, id: c_long) -> Option<&TimerNode> {
        self.node(Some(id)).2
    }

    /// The queue's node `id`, or for `None` the next due node of the selection
    /// ([`Queue::select_next`], which makes it the active one): the queue's status
    /// and view, and the node when the status is `Ok`.
    #[inline]
    fn node(&self, id: Option<c_long>) -> (Status, TimerView, Option<&TimerNode>) {
        let (status, view) = self.lend(|queue| match id {
            Some(id) => queue.get(id).map_or_else(
                |status| (status, TimerView::default()),
                |view| (Status::Ok, view),
            ),
            None => queue.select_next(),
        });
        if status != Status::Ok {
            return (status, view, None);
        }
        // SAFETY: a token the queue holds, which it just returned: a live node `add`
        // boxed, which only `release` frees once the queue has let it go. Nothing
        // borrows a node mutably.
        let node = unsafe { view.token.cast::<TimerNode>().as_ref() };
        (status, view, node)
    }

    /// Picks the manual, reactor or system backend. Another backend keeps a
    /// previous reactor, as upstream's `reactor_` did.
    fn init(&self, manually_trigger: bool) {
        let reactor = thread_reactor();
        let backend =
            swr_v1_timer_backend(u32::from(manually_trigger), u32::from(!reactor.is_null()));
        self.backend.set(backend);
        if backend == REACTOR {
            self.reactor.set(reactor);
            attach();
        } else if backend != MANUAL {
            let alarm = crate::reactor::signal_upstream::Handler::new(alarm);
            _ = crate::reactor::signal_upstream::set(libc::SIGALRM, alarm, false);
        }
    }

    /// Arms the backend's wakeup `msec` from now; negative disarms it.
    fn set(&self, msec: i64) {
        match self.backend.get() {
            MANUAL => {}
            // The thread's reactor is the one the backend attached to: its destroy
            // callback frees this timer before the thread's reactor is cleared.
            REACTOR => event_api::swoole_event_set_timeout_msec(msec.c_cast::<i32>()),
            _ => {
                // Upstream's set() callers ignore its status too.
                _ = swr_v1_timer_system_set(msec);
            }
        }
    }

    fn reinit(&self, manually_trigger: bool) {
        self.set(-1);
        self.init(manually_trigger);
        self.set(self.next_msec.get());
    }

    /// Deletes the detached node whose custody Rust just returned in `view`,
    /// releasing its callback before returning any deferred escape to its caller.
    fn release(view: &TimerView) {
        let node = view.token.cast::<TimerNode>();
        // SAFETY: `add` boxed it; the queue no longer holds it.
        drop(unsafe { Box::from_raw(node) });
    }

    /// The added node's id, or none; `php` marks a PHP callable's node.
    pub(crate) fn add(
        &self,
        msec: c_long,
        persistent: bool,
        php: bool,
        node: TimerNode,
    ) -> Option<c_long> {
        if msec <= 0 {
            warning(c"add", format_args!("msec value[{msec}] is invalid"));
            return None;
        }
        let now = self.relative_msec();
        // The queue boxes the node only once it has room for it, so a refused node
        // drops here with the closure.
        let added = self.lend(|queue| {
            queue.add_marked(now, msec, persistent, php, || {
                Box::into_raw(Box::new(node)).cast()
            })
        });
        let view = match added {
            Ok(view) => view,
            Err(status) => {
                warning(
                    c"add",
                    format_args!("Rust timer add failed: status={status:?}"),
                );
                return None;
            }
        };
        let next = swr_v1_timer_rearm_after_add(self.next_msec.get(), msec);
        if next != self.next_msec.get() {
            self.set(next);
            self.next_msec.set(next);
        }
        Some(view.id)
    }

    /// Reserve a PHP node before its caller retains a callable.
    pub(crate) fn reserve_php(&self, ms: i64, persistent: bool) -> Option<i64> {
        self.add(ms, persistent, true, TimerNode::new())
    }

    /// Whether `id` was removed.
    pub(crate) fn remove(&self, id: c_long) -> bool {
        self.remove_if(id, |_| true)
    }

    pub(crate) fn remove_php(&self, id: c_long) -> bool {
        self.remove_if(id, |view| view.php)
    }

    fn remove_if(&self, id: c_long, predicate: impl FnOnce(&TimerView) -> bool) -> bool {
        let (status, view) = self.lend(|queue| queue.remove_if(id, predicate));
        match status {
            Status::Deferred => true,
            Status::Release => {
                Self::release(&view);
                true
            }
            _ => false,
        }
    }

    /// Refreshes node `id`'s deadline from its caller-mutated `exec_msec`. Both by
    /// value, as for [`Self::delay`].
    #[cfg(test)]
    fn update(&self, id: i64, exec_msec: i64) {
        _ = self.lend(|queue| queue.update(id, exec_msec));
    }

    /// Moves node `id`'s deadline, now `exec_msec` (its queued deadline for `None`), to
    /// `delay_ms` from now.
    pub(crate) fn delay(&self, id: i64, exec_msec: Option<i64>, delay_ms: c_long) {
        let now = self.relative_msec();
        _ = self.lend(|queue| {
            queue.reschedule(id, |deadline| {
                swr_v1_timer_delay_deadline(now, exec_msec.unwrap_or(deadline), delay_ms)
            })
        });
    }

    /// Completes the active node.
    fn complete(&self, id: i64) {
        let (status, view) = self.lend(|queue| queue.complete(id));
        match status {
            Status::Ok => {}
            Status::Release | Status::ReleaseOverflow => {
                Self::release(&view);
                if status == Status::ReleaseOverflow {
                    warning(c"select", format_args!("Rust timer repeat limit exceeded"));
                }
            }
            _ => check(status),
        }
    }

    /// Ends the selection and arms the next wakeup; false if Rust refused.
    fn end(&self) -> bool {
        let Ok(delay) = self.lend(Queue::end) else {
            return false;
        };
        self.next_msec.set(delay);
        self.set(delay);
        true
    }

    /// Runs every due node. A Zend bailout longjmps out of a callback over these
    /// frames, which hold nothing to drop; a C++ exception aborts at the first
    /// Rust frame (`panic = "abort"`), as an uncaught one would.
    pub(crate) fn select(&self) {
        if self.abandoned.get() {
            return;
        }
        let now = self.relative_msec();
        // Most loop turns find no timer due.
        let idle = self.lend(|queue| queue.select_idle(now));
        if let Some(delay) = idle {
            self.next_msec.set(delay);
            self.set(delay);
            return;
        }
        let begin = self.lend(|queue| queue.begin(now));
        match begin {
            Ok(()) => {}
            Err(Status::Busy | Status::Closing) => return,
            Err(status) => check(status),
        }
        loop {
            // The queue is back before the node's callback runs.
            let (status, view, node) = self.node(None);
            if status == Status::Empty || status == Status::Closing {
                break;
            }
            check(status);
            // The node the queue just selected, held until complete.
            if !view.removed
                && let Some(node) = node
            {
                node.retained.run();
            }
            if self.abandoned.get() {
                return;
            }
            self.complete(view.id);
            if self.abandoned.get() {
                return;
            }
        }
        if !self.end() {
            check(Status::Closing);
        }
    }

    /// Only after a caught Zend bailout whose selection will never resume.
    pub(crate) fn abandon(&self) {
        self.abandoned.set(true);
    }

    /// `~Timer`: releases every node, then the queue. A forked child or terminal
    /// Zend bailout will never resume an active selection, so it is closed here;
    /// same-process deletion during a live callback aborts. Takes the timer by
    /// value: a published timer is first taken out of its slot (`wrapper::take`).
    #[expect(
        clippy::boxed_local,
        reason = "the timer stays at its address while released callbacks run"
    )]
    pub(crate) fn destroy(self: Box<Self>) {
        self.lend(Queue::close);
        self.set(-1);
        if self.abandoned.get() || std::process::id().cast_signed() != self.owner_pid {
            let (active, view) = self.lend(|queue| {
                let stats = queue.stats();
                let (status, view) = if stats.active_id == 0 {
                    (Status::Release, TimerView::default())
                } else {
                    queue.complete(stats.active_id)
                };
                if status != Status::Release {
                    crate::fatal("a timer completed without releasing");
                }
                if stats.selecting
                    && let Err(status) = queue.end()
                {
                    check(status);
                }
                (stats.active_id != 0, view)
            });
            if active {
                Self::release(&view);
            }
        }
        let status = loop {
            let (status, view) = self.lend(Queue::drain);
            if status != Status::Release {
                break status;
            }
            Self::release(&view);
        };
        // The record's drop frees the drained queue.
        if status != Status::Empty || !self.lend(|queue| queue.idle()) {
            crate::fatal("a timer queue still busy when dropped");
        }
    }
}

/// A second take of a timer's queue while an operation holds it.
#[cold]
#[inline(never)]
fn lent() -> ! {
    crate::fatal("a timer queue re-entered during a queue operation")
}

/// A unique timer identity; low bits retain Held's nonzero packed generation.
/// Reservations use the full identity, so a packed generation wrapping cannot
/// install a callback into an unrelated timer's node.
fn next_generation() -> u64 {
    use std::sync::atomic::Ordering::Relaxed;
    loop {
        let previous = GENERATIONS
            .try_update(Relaxed, Relaxed, |value| value.checked_add(1))
            .unwrap_or_else(|_| crate::fatal("timer identities exhausted"));
        let generation = previous + 1;
        if generation.c_cast::<u16>() != 0 {
            return generation;
        }
    }
}

/// Hooks this thread's timer to the thread's reactor: select after each loop
/// round, keep the loop while a timer is pending, and free the timer with it.
fn attach() {
    swr_v1_event_set_end_callback(
        crate::reactor::Priority::Timer,
        crate::timer::wrapper::swoole_timer_select,
    );
    swr_v1_event_set_exit_condition(crate::reactor::ExitCondition::Timer, idle);
    event_api::on_destroy(destroyed);
}

/// The exit condition: no timer is pending.
fn idle(_event_num: &mut usize) -> bool {
    crate::timer::wrapper::current().is_none_or(|timer| timer.count() == 0)
}

/// The reactor's destroy callback: frees this thread's timer, closing a
/// selection a bailout left. `swoole_event_free` destroys the thread's reactor
/// before it clears it, so the thread's reactor is the one being destroyed.
fn destroyed() {
    let Some(timer) = crate::timer::wrapper::current() else {
        return;
    };
    if event_api::swoole_event_bailed_out() {
        timer.abandon();
    }
    crate::timer::wrapper::swoole_timer_free();
}

/// `Timer::get_reactor()`.
pub(crate) fn owner_reactor(timer: &Owner) -> *mut c_void {
    timer.reactor.get()
}

/// `Timer::reinit(manually_trigger)`.
pub fn owner_reinit(timer: &Owner, manually_trigger: bool) {
    timer.reinit(manually_trigger);
}

/// `tests/modules/timer/probe.cc`'s scenarios, with `test_adapter.py`'s expected values:
/// a manual timer whose nodes log their callback (the label) and destructor (the
/// label in lower case).
#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::RefCell;
    use std::rc::Rc;

    /// `delete timer`, as `swoole_timer_free` does of the thread's: these timers are
    /// never published.
    unsafe fn free(timer: *mut Owner) {
        unsafe { Box::from_raw(timer) }.destroy();
    }

    type Call = Box<dyn FnMut(&Owner, c_long)>;

    /// A node's closures, as probe.cc's `Hooks`.
    struct Hooks {
        id: Cell<c_long>,
        call: RefCell<Call>,
        destructor: RefCell<Box<dyn FnMut()>>,
    }

    /// A probe node's id and hooks.
    struct Added {
        id: c_long,
        hooks: Rc<Hooks>,
    }

    /// The hooks a probe node retains: its callback runs `call` with the node's id,
    /// and its release `destructor`.
    struct Hook {
        timer: &'static Owner,
        hooks: Rc<Hooks>,
    }

    impl crate::timer::php::Callback for Hook {
        fn run(&self) {
            (self.hooks.call.borrow_mut())(self.timer, self.hooks.id.get());
        }
        fn release(self) {
            (self.hooks.destructor.borrow_mut())();
        }
    }

    #[derive(Default)]
    struct Log {
        events: String,
        destructors: u32,
    }

    struct Probe {
        timer: *mut Owner,
        log: Rc<RefCell<Log>>,
    }

    impl Probe {
        fn new() -> Self {
            Self {
                timer: Owner::create(true),
                log: Rc::default(),
            }
        }
        fn timer(&self) -> &'static Owner {
            unsafe { &*self.timer }
        }
        fn events(&self) -> String {
            self.log.borrow().events.clone()
        }
        fn destructors(&self) -> u32 {
            self.log.borrow().destructors
        }
        /// probe.cc's `add`: due now, with a destructor that logs `label`.
        fn add(&self, label: char, repeat: bool, call_back: Call) -> Added {
            let log = Rc::clone(&self.log);
            let hooks = Rc::new(Hooks {
                id: Cell::new(0),
                call: RefCell::new(call_back),
                destructor: RefCell::new(Box::new(move || {
                    let mut log = log.borrow_mut();
                    log.destructors += 1;
                    log.events.push(label.to_ascii_lowercase());
                })),
            });
            let timer = self.timer();
            let id = timer
                .add(1000, repeat, false, TimerNode::new())
                .expect("add failed");
            timer.update(id, 0);
            hooks.id.set(id);
            timer.get(id).expect("added").install(Hook {
                timer,
                hooks: Rc::clone(&hooks),
            });
            Added { id, hooks }
        }
        fn record(&self, label: char) -> Call {
            let log = Rc::clone(&self.log);
            Box::new(move |_, _| log.borrow_mut().events.push(label))
        }
        /// Replaces a node's destructor, logging as `add`'s does first.
        fn on_destroy(&self, node: &Added, label: char, then: impl FnMut() + 'static) {
            let log = Rc::clone(&self.log);
            let mut then = then;
            *node.hooks.destructor.borrow_mut() = Box::new(move || {
                {
                    let mut log = log.borrow_mut();
                    log.destructors += 1;
                    log.events.push(label);
                }
                then();
            });
        }
        fn present(&self, id: c_long) -> bool {
            id != 0 && self.timer().info(id).is_some()
        }
        fn free(&self) {
            unsafe { free(self.timer) };
        }
    }

    #[test]
    fn escaped_callback_keeps_active_metadata_and_stops_later_timers() {
        let probe = Probe::new();
        let log = Rc::clone(&probe.log);
        let first = probe.add(
            'A',
            false,
            Box::new(move |timer, _| {
                log.borrow_mut().events.push('A');
                timer.abandon();
            }),
        );
        probe.add('B', false, probe.record('B'));
        let timer = probe.timer();
        timer.select();
        timer.select();
        assert_eq!(probe.events(), "A");
        let first = first.id;
        assert_eq!(timer.lend(|queue| queue.stats().active_id), first);
        assert_eq!(timer.count(), 2);
        timer.select();
        assert_eq!(probe.events(), "A");
        probe.free();
        assert_eq!(probe.events(), "Aab");
        assert_eq!(probe.destructors(), 2);
    }

    /// The common tail: two selections (the first is the creation-round barrier),
    /// then `(after_second, events, count_after_second, destructors, a_present,
    /// b_present)` once the timer is freed.
    fn finish(
        probe: &Probe,
        a: c_long,
        b: c_long,
        select_again: bool,
    ) -> (String, String, usize, u32, bool, bool) {
        let timer = probe.timer();
        timer.select();
        assert_eq!(probe.events(), "", "after_first");
        timer.select();
        let count = timer.count();
        let (a_present, b_present) = (probe.present(a), probe.present(b));
        let after_second = probe.events();
        if select_again {
            timer.select();
        }
        probe.free();
        (
            after_second,
            probe.events(),
            count,
            probe.destructors(),
            a_present,
            b_present,
        )
    }

    fn expect(
        got: &(String, String, usize, u32, bool, bool),
        second: &str,
        events: &str,
        count: usize,
        destructors: u32,
        a_present: bool,
    ) {
        assert_eq!(
            *got,
            (
                second.into(),
                events.into(),
                count,
                destructors,
                a_present,
                false
            )
        );
    }

    #[test]
    fn equal_deadlines() {
        let p = Probe::new();
        for label in ['A', 'B', 'C'] {
            p.add(label, false, p.record(label));
        }
        expect(&finish(&p, 0, 0, false), "AaCcBb", "AaCcBb", 0, 3, false);
    }

    #[test]
    fn callback_self_removal_preserves_deferred_release() {
        for persistent in [false, true] {
            let p = Probe::new();
            let log = Rc::clone(&p.log);
            p.add(
                'A',
                persistent,
                Box::new(move |t, id| {
                    let mut log = log.borrow_mut();
                    assert_eq!(log.destructors, 0);
                    log.events.push('A');
                    log.events.push(if t.remove(id) { 'Y' } else { 'N' });
                    log.events.push(if t.remove(id) { 'Y' } else { 'N' });
                    let viewed = t.info(id).is_some_and(|(info, _)| info.removed);
                    log.events.push(if viewed { 'V' } else { 'X' });
                }),
            );
            expect(&finish(&p, 0, 0, false), "AYNVa", "AYNVa", 0, 1, false);
        }
    }

    fn remove_other(in_destructor: bool) -> (String, String, usize, u32, bool, bool) {
        let p = Probe::new();
        let other = Rc::new(Cell::new(0));
        let (log, target) = (Rc::clone(&p.log), Rc::clone(&other));
        let a = p.add(
            'A',
            false,
            Box::new(move |t, _| {
                log.borrow_mut().events.push('A');
                if !in_destructor {
                    t.remove(target.get());
                }
            }),
        );
        other.set(p.add('B', false, p.record('B')).id);
        p.add('C', false, p.record('C'));
        if in_destructor {
            let (timer, target) = (p.timer(), Rc::clone(&other));
            p.on_destroy(&a, 'a', move || {
                timer.remove(target.get());
            });
        }
        finish(&p, 0, 0, false)
    }

    #[test]
    fn remove_other_in_a_callback() {
        expect(&remove_other(false), "AbaCc", "AbaCc", 0, 3, false);
    }

    #[test]
    fn destructor_remove_other() {
        expect(&remove_other(true), "AabCc", "AabCc", 0, 3, false);
    }

    /// `delay-active-once` and `update-active-repeat`: the active node moves its own
    /// deadline 100 s out from its callback.
    fn move_active(repeat: bool) -> (String, String, usize, u32, bool, bool) {
        let p = Probe::new();
        let updated = Rc::new(Cell::new(false));
        let (log, flag) = (Rc::clone(&p.log), Rc::clone(&updated));
        let a = p.add(
            'A',
            repeat,
            Box::new(move |t, id| {
                log.borrow_mut().events.push('A');
                if repeat {
                    t.update(id, 100_000);
                } else {
                    let exec_msec = t.info(id).expect("queued").0.exec_msec;
                    t.delay(id, Some(exec_msec), 100_000);
                }
                flag.set(true);
            }),
        );
        let b = p.add('B', false, p.record('B'));
        let (a, b) = (a.id, b.id);
        let got = finish(&p, a, b, false);
        assert!(updated.get(), "the callback updated its node");
        got
    }

    #[test]
    fn update_active_repeat() {
        expect(&move_active(true), "ABb", "ABba", 1, 2, true);
    }

    #[test]
    fn delay_active_once() {
        expect(&move_active(false), "AaBb", "AaBb", 0, 2, false);
    }

    #[test]
    fn destructor_add() {
        let p = Probe::new();
        let a = p.add('A', false, p.record('A'));
        let probe = Probe {
            timer: p.timer,
            log: Rc::clone(&p.log),
        };
        p.on_destroy(&a, 'a', move || {
            probe.add('C', false, probe.record('C'));
        });
        p.add('B', false, p.record('B'));
        expect(&finish(&p, 0, 0, true), "AaBb", "AaBbCc", 1, 3, false);
    }

    #[test]
    fn nested_select_once() {
        let p = Probe::new();
        let (log, nested) = (Rc::clone(&p.log), Rc::new(Cell::new(false)));
        p.add(
            'A',
            false,
            Box::new(move |t, _| {
                log.borrow_mut().events.push('A');
                if !nested.replace(true) {
                    t.select();
                }
            }),
        );
        expect(&finish(&p, 0, 0, false), "Aa", "Aa", 0, 1, false);
    }

    /// Waits for `child`; its exit status.
    fn reap(child: libc::pid_t) -> i32 {
        let mut status = 0;
        assert_eq!(unsafe { libc::waitpid(child, &raw mut status, 0) }, child);
        status
    }

    #[test]
    #[cfg_attr(miri, ignore = "Miri has no fork")]
    fn fork_teardown() {
        let p = Probe::new();
        let id = p.add('A', false, p.record('A')).id;
        let child = unsafe { libc::fork() };
        assert!(child >= 0, "fork failed");
        if child == 0 {
            p.free();
            unsafe { libc::_exit(if p.destructors() == 1 { 0 } else { 2 }) };
        }
        assert_eq!(reap(child), 0, "child_status");
        let intact = p.present(id) && p.timer().count() == 1 && p.destructors() == 0;
        p.free();
        assert!(intact, "parent_intact");
        assert_eq!(p.destructors(), 1, "parent_destructors");
    }

    #[test]
    #[cfg_attr(miri, ignore = "Miri has no fork")]
    fn fork_active_teardown() {
        let p = Probe::new();
        let (log, status, intact) = (
            Rc::clone(&p.log),
            Rc::new(Cell::new(-1)),
            Rc::new(Cell::new(false)),
        );
        let (child_status, parent_intact) = (Rc::clone(&status), Rc::clone(&intact));
        let timer = p.timer;
        p.add(
            'A',
            false,
            Box::new(move |t, id| {
                let child = unsafe { libc::fork() };
                assert!(child >= 0, "fork failed");
                if child == 0 {
                    let log = Rc::clone(&log);
                    unsafe { free(timer) };
                    let log = log.borrow();
                    let ok = log.destructors == 2 && log.events.len() == 2;
                    unsafe { libc::_exit(if ok { 0 } else { 2 }) };
                }
                child_status.set(reap(child));
                parent_intact
                    .set(t.info(id).is_some() && t.count() == 2 && log.borrow().destructors == 0);
                log.borrow_mut().events.push('A');
            }),
        );
        p.add('B', false, p.record('B'));
        p.timer().select();
        p.timer().select();
        let drained = p.timer().count() == 0 && p.events() == "AaBb" && p.destructors() == 2;
        p.free();
        assert_eq!(status.get(), 0, "child_status");
        assert!(intact.get(), "parent_intact");
        assert!(drained, "parent_drained");
        assert_eq!(p.destructors(), 2, "parent_destructors");
    }

    #[test]
    fn an_owning_node_drops_its_capture_with_the_node() {
        let p = Probe::new();
        let timer = p.timer();
        let capture = Rc::new(Cell::new(0));
        let ids = Rc::new(Cell::new(0));
        // A tick that removes itself when it fires, while its callback runs.
        let add = |persistent, remove: bool| {
            let (seen, me) = (Rc::clone(&capture), Rc::clone(&ids));
            let f = Rc::new(move || {
                seen.set(seen.get() + 1);
                if remove {
                    assert!(timer.remove(me.get()));
                }
            });
            let id = timer
                .add(1000, persistent, false, TimerNode::closure(move || f()))
                .expect("add failed");
            timer.update(id, 0);
            id
        };
        let (_once, deleted, _tick) = (add(false, false), add(false, false), add(true, false));
        ids.set(add(true, true));
        assert_eq!(Rc::strong_count(&capture), 5, "held by each node");
        assert!(timer.remove(deleted));
        assert_eq!(Rc::strong_count(&capture), 4, "dropped on delete");
        timer.select();
        timer.select();
        assert_eq!(capture.get(), 3, "the one-shot and both ticks ran");
        assert_eq!(
            Rc::strong_count(&capture),
            2,
            "dropped once the one-shot fired and the tick removed itself"
        );
        p.free();
        assert_eq!(
            Rc::strong_count(&capture),
            1,
            "dropped when the timer is freed"
        );
    }

    #[test]
    fn timer_is_unpublished_when_callbacks_are_released() {
        use crate::timer::wrapper::{
            current, publish, swoole_timer_clear, swoole_timer_free, timer,
        };
        let previous = timer();
        let p = Probe::new();
        publish(p.timer);
        let node = p.add('A', false, p.record('A'));
        let id = node.id;
        // A destructor that calls back into the API finds no timer, not the one
        // being destroyed.
        p.on_destroy(&node, 'a', move || {
            assert!(current().is_none());
            assert!(!swoole_timer_clear(id));
        });
        swoole_timer_free();
        publish(previous);
        assert_eq!(p.destructors(), 1);
    }

    #[test]
    fn wrapper_clear() {
        use crate::timer::wrapper::{
            publish, swoole_timer_clear, swoole_timer_get, timer as slot_value,
        };
        let p = Probe::new();
        let previous = slot_value();
        publish(p.timer);
        let inactive_id = p.add('C', false, p.record('C')).id;
        let inactive = swoole_timer_clear(inactive_id) && !swoole_timer_clear(inactive_id);
        let missing = !swoole_timer_clear(0)
            && !swoole_timer_clear(-1)
            && !swoole_timer_clear(inactive_id + 100);
        let other = Rc::new(Cell::new(0));
        let seen = Rc::new(Cell::new((false, false, false)));
        let (log, target, flags) = (Rc::clone(&p.log), Rc::clone(&other), Rc::clone(&seen));
        p.add(
            'A',
            true,
            Box::new(move |timer, id| {
                log.borrow_mut().events.push('A');
                let cancelled_other =
                    swoole_timer_clear(target.get()) && !swoole_timer_clear(target.get());
                let active = swoole_timer_clear(id) && !swoole_timer_clear(id);
                let active_view = swoole_timer_get(id).is_some_and(|held| held.id() == id)
                    && timer.info(id).is_some_and(|(info, _)| info.removed);
                flags.set((active, cancelled_other, active_view));
            }),
        );
        other.set(p.add('B', false, p.record('B')).id);
        p.timer().select();
        p.timer().select();
        let remaining = p.timer().count();
        p.free();
        publish(previous);
        assert!(inactive, "inactive");
        assert!(missing, "missing");
        assert_eq!(
            seen.get(),
            (true, true, true),
            "active, cancelled_other, active_view"
        );
        assert_eq!(remaining, 0, "remaining");
        assert_eq!(p.events(), "cAba", "events");
        assert_eq!(p.destructors(), 3, "destructors");
    }
    #[test]
    fn php_reservations_own_callbacks_through_cancel_fire_and_teardown() {
        use crate::timer::wrapper::{php, publish, timer as slot_value};
        struct Kept {
            id: i64,
            log: Rc<RefCell<Vec<&'static str>>>,
        }
        impl php::Callback for Kept {
            fn run(&self) {
                self.log.borrow_mut().push("run");
                assert!(php::clear(self.id));
                assert!(php::info(self.id).unwrap().0.removed);
            }
            fn release(self) {
                assert!(php::info(self.id).is_none());
                self.log.borrow_mut().push("release");
            }
        }
        let p = Probe::new();
        let previous = slot_value();
        publish(p.timer);
        let log = Rc::new(RefCell::new(Vec::new()));
        let native_id = p.add('N', false, p.record('N')).id;
        assert!(!php::clear(native_id));
        assert!(p.timer().remove(native_id));
        assert!(php::reserve(0, false).is_none());
        assert_eq!(p.timer().count(), 0);
        let abandoned = php::reserve(1, false).unwrap();
        assert_eq!(p.timer().count(), 1);
        drop(abandoned);
        assert_eq!(p.timer().count(), 0);

        let pending = php::reserve(1, true).unwrap();
        let id = pending.id().get();
        assert!(pending.commit(Kept {
            id,
            log: Rc::clone(&log)
        }));
        p.timer().update(id, 0);
        p.timer().select();
        p.timer().select();
        assert_eq!(*log.borrow(), ["run", "release"]);
        assert_eq!(p.timer().count(), 0);

        let pending = php::reserve(1, false).unwrap();
        let id = pending.id().get();
        assert!(php::clear(id));
        assert!(!pending.commit(Kept {
            id,
            log: Rc::clone(&log)
        }));
        assert_eq!(*log.borrow(), ["run", "release", "release"]);

        let pending = php::reserve(1, false).unwrap();
        let id = pending.id().get();
        assert!(pending.commit(Kept {
            id,
            log: Rc::clone(&log)
        }));
        p.free();
        publish(previous);
        assert_eq!(*log.borrow(), ["run", "release", "release", "release"]);
        assert_eq!(Rc::strong_count(&log), 1);
    }

    #[test]
    fn a_reservation_cannot_install_into_a_replacement_timer() {
        use crate::timer::wrapper::{php, publish, timer as slot_value};
        struct Kept(Rc<Cell<usize>>);
        impl php::Callback for Kept {
            fn run(&self) {
                panic!("the old reservation must not run");
            }
            fn release(self) {
                self.0.set(self.0.get() + 1);
            }
        }
        let first = Probe::new();
        let previous = slot_value();
        publish(first.timer);
        let pending = php::reserve(1, false).unwrap();
        let first_identity = first.timer().identity();
        first.free();
        let second = Probe::new();
        // Simulate the packed Held generation wrapping without changing the
        // global counter or creating 65,536 timer owners.
        unsafe { (*second.timer).generation = first_identity + (1 << 16) };
        assert_eq!(second.timer().generation(), first_identity.c_cast::<u16>());
        publish(second.timer);
        let replacement = php::reserve(1, false).unwrap();
        assert_eq!(pending.id(), replacement.id());
        let released = Rc::new(Cell::new(0));
        assert!(!pending.commit(Kept(Rc::clone(&released))));
        assert_eq!(released.get(), 1);
        assert_eq!(second.timer().count(), 1);
        drop(replacement);
        assert_eq!(second.timer().count(), 0);
        second.free();
        publish(previous);
    }

    #[test]
    fn a_retained_callback_keeps_its_own_state_between_ticks() {
        use crate::timer::wrapper::{php, publish, timer as slot_value};
        struct Counter(Cell<usize>, Rc<Cell<usize>>);
        impl php::Callback for Counter {
            fn run(&self) {
                self.0.set(self.0.get() + 1);
            }
            fn release(self) {
                self.1.set(self.0.get());
            }
        }
        let p = Probe::new();
        let previous = slot_value();
        publish(p.timer);
        let released = Rc::new(Cell::new(0));
        let pending = php::reserve(1, true).unwrap();
        let id = pending.id().get();
        assert!(pending.commit(Counter(Cell::new(0), Rc::clone(&released))));
        p.timer().select();
        for _ in 0..2 {
            p.timer().update(id, 0);
            p.timer().select();
        }
        assert_eq!(released.get(), 0);
        assert!(php::clear(id));
        assert_eq!(released.get(), 2);
        assert_eq!(Rc::strong_count(&released), 1);
        p.free();
        publish(previous);
    }
}
