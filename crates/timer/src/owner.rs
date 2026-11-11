// SPDX-License-Identifier: Apache-2.0
// Compatibility derived from Swoole v6.2.4 src/core/timer.cc (Swoole contributors).
//! `swoole::Timer` (upstream `src/core/timer.cc`, which the timer patch deletes): a
//! Rust-owned record over this crate's queue, and its nodes.
//! `native/timer/swoole_timer.h` is its C++ view.
use crate::{
    BUSY, CLOSING, DEFERRED, EMPTY, OK, RELEASE, RELEASE_OVERFLOW, Timer as Queue, TimerView,
    absolute_msec, swr_v1_timer_add_delay, swr_v1_timer_backend, swr_v1_timer_delay_deadline,
    swr_v1_timer_rearm_after_add, swr_v1_timer_relative_msec, swr_v1_timer_system_set,
};
use std::cell::{Cell, RefCell};
use std::ffi::{CStr, c_int, c_long, c_void};
use swoole_rs_corebase::{log, swr_v1_core_set_signal_alarm, thread_reactor};
use swoole_rs_reactor::ReactorPrepared;
use swoole_rs_reactor::{
    event_api, swr_v1_event_set_end_callback, swr_v1_event_set_exit_condition,
};

/// `SW_LOG_WARNING` and `SW_ERROR_INVALID_PARAMS`.
const LOG_WARNING: c_int = 4;
const INVALID_PARAMS: c_int = 505;
/// `swr_v1_timer_backend`'s modes.
const MANUAL: u32 = 0;
const REACTOR: u32 = 1;
/// `Reactor::PRIORITY_TIMER` and `Reactor::EXIT_CONDITION_TIMER`.
const PRIORITY_TIMER: c_int = 0;
const EXIT_CONDITION_TIMER: c_int = 0;

/// `swoole::TimerCallback`: `void (*)(Timer *, TimerNode *)`.
pub(crate) type Callback = unsafe extern "C-unwind" fn(timer: *mut c_void, node: *mut TimerNode);
/// `swoole::TimerDestructor`: `void (*)(TimerNode *)`.
type Destructor = unsafe extern "C-unwind" fn(node: *mut TimerNode);
/// A callback of the node's data alone, for `swr_v1_timer_add_plain`'s callers.
pub type Plain = unsafe extern "C-unwind" fn(data: *mut c_void);
/// `abandon_selection`'s pending Zend bailout.
pub type Bailout = unsafe extern "C-unwind" fn();
/// `TimerNode::TYPE_PHP`.
const TYPE_PHP: i32 = 1;

/// A node's fields, for `Swoole\Timer::info()`.
#[derive(Debug, Clone, Copy)]
pub struct Info {
    pub exec_msec: i64,
    pub exec_count: u64,
    pub interval: i64,
    pub round: u64,
    pub removed: bool,
}

/// `swoole::TimerNode`; `native/timer/swoole_timer.h` asserts the offsets. Only
/// `TimerNode::new` makes one, so `plain` can trail what C++ sees.
#[repr(C)]
pub struct TimerNode {
    pub(crate) id: c_long,
    kind: i32,
    pub(crate) exec_msec: i64,
    interval: i64,
    exec_count: u64,
    round: u64,
    pub(crate) removed: bool,
    callback: Option<Callback>,
    data: *mut c_void,
    destructor: Option<Destructor>,
    plain: Option<Plain>,
    /// A `TimerNode::php` node's destructor of its data.
    plain_drop: Option<Plain>,
}

impl TimerNode {
    pub(crate) fn new(callback: Option<Callback>, data: *mut c_void) -> Self {
        Self {
            id: 0,
            kind: 0,
            exec_msec: 0,
            interval: 0,
            exec_count: 0,
            round: 0,
            removed: false,
            callback,
            data,
            destructor: None,
            plain: None,
            plain_drop: None,
        }
    }

    /// A node whose callback is `plain(data)`.
    pub(crate) fn plain(plain: Plain, data: *mut c_void) -> Self {
        let mut node = Self::new(Some(call_plain), data);
        node.plain = Some(plain);
        node
    }

    /// A `TYPE_PHP` node: `callback(data)` on each run, `destructor(data)` once
    /// it is removed.
    pub(crate) fn php(callback: Plain, destructor: Plain, data: *mut c_void) -> Self {
        let mut node = Self::plain(callback, data);
        node.kind = TYPE_PHP;
        node.plain_drop = Some(destructor);
        node
    }

    pub(crate) fn is_php(&self) -> bool {
        self.kind == TYPE_PHP
    }

    /// The timer's id, which its timer never reuses.
    #[must_use]
    pub fn id(&self) -> c_long {
        self.id
    }

    pub(crate) fn info(&self) -> Info {
        Info {
            exec_msec: self.exec_msec,
            exec_count: self.exec_count,
            interval: self.interval,
            round: self.round,
            removed: self.removed,
        }
    }
}

/// A `TimerNode::plain` node's callback.
unsafe extern "C-unwind" fn call_plain(_timer: *mut c_void, node: *mut TimerNode) {
    // SAFETY: the running node.
    let node = unsafe { &*node };
    if let Some(plain) = node.plain {
        // SAFETY: the caller's function of its data.
        unsafe { plain(node.data) };
    }
}

/// A queue status that only a broken invariant returns (upstream threw).
fn check(status: i32) {
    if status != OK {
        std::process::abort();
    }
}

fn warning(origin: &CStr, message: std::fmt::Arguments<'_>) {
    log::error_log(LOG_WARNING, INVALID_PARAMS, origin, message);
}

/// The system timer's `SIGALRM` handler.
extern "C" fn alarm(_signo: c_int) {
    swr_v1_core_set_signal_alarm(true);
}

/// Copies a view Rust hands back into its node, so the node is current.
fn apply(view: &TimerView) -> *mut TimerNode {
    let node = view.token.cast::<TimerNode>();
    // SAFETY: every token is a live TimerNode that `add` made.
    let node_ref = unsafe { &mut *node };
    node_ref.id = view.id;
    node_ref.exec_msec = view.deadline;
    node_ref.interval = view.interval;
    node_ref.exec_count = view.exec_count;
    node_ref.round = view.birth_round;
    node_ref.removed = view.removed != 0;
    node
}

/// `swoole::Timer`. Callbacks re-enter it, so every mutable field is a `Cell`, and the
/// queue a `RefCell` that no borrow holds across a callback or destructor.
pub struct Owner {
    reactor: Cell<*mut c_void>,
    queue: RefCell<Queue>,
    owner_pid: libc::pid_t,
    backend: Cell<u32>,
    abandoned: Cell<bool>,
    pending_bailout: Cell<Option<Bailout>>,
    base_time: i64,
    next_msec: Cell<i64>,
}

impl Owner {
    /// `new Timer(manually_trigger)`.
    pub(crate) fn create(manually_trigger: bool) -> *mut Owner {
        let timer = Box::into_raw(Box::new(Owner {
            reactor: Cell::new(std::ptr::null_mut()),
            queue: RefCell::default(),
            owner_pid: std::process::id().cast_signed(),
            backend: Cell::new(MANUAL),
            abandoned: Cell::new(false),
            pending_bailout: Cell::new(None),
            base_time: absolute_msec(),
            next_msec: Cell::new(-1),
        }));
        // SAFETY: just allocated.
        unsafe { &*timer }.init(manually_trigger);
        timer
    }

    pub(crate) fn relative_msec(&self) -> i64 {
        swr_v1_timer_relative_msec(absolute_msec(), self.base_time)
    }

    pub(crate) fn next_msec(&self) -> i64 {
        self.next_msec.get()
    }

    pub(crate) fn count(&self) -> usize {
        self.queue.borrow().heap.len()
    }

    /// `Timer::get_round()`.
    pub(crate) fn round(&self) -> u64 {
        self.queue.borrow().round
    }

    /// `Timer::get_ids()`, in heap order.
    pub(crate) fn ids(&self) -> Vec<i64> {
        self.queue
            .borrow()
            .heap
            .iter()
            .map(|view| view.id)
            .collect()
    }

    /// `Timer::get(id)`: the node, or null.
    pub(crate) fn token(&self, id: c_long) -> *mut TimerNode {
        self.queue.borrow().token(id).cast()
    }

    /// `Timer::get(id)`, borrowed.
    pub(crate) fn get(&self, id: c_long) -> Option<&TimerNode> {
        // SAFETY: null or a live TimerNode that `add` made.
        unsafe { self.token(id).as_ref() }
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
            attach(reactor);
        } else if backend != MANUAL {
            let alarm = swoole_rs_reactor::signal_upstream::Handler::new(alarm);
            _ = swoole_rs_reactor::signal_upstream::set(libc::SIGALRM, alarm, false);
        }
    }

    /// Arms the backend's wakeup `msec` from now; negative disarms it.
    fn set(&self, msec: i64) {
        match self.backend.get() {
            MANUAL => {}
            REACTOR => {
                // The live reactor the backend attached to.
                let fields = ReactorPrepared::of(self.reactor.get()).fields;
                #[expect(
                    clippy::cast_possible_truncation,
                    reason = "Reactor::timeout_msec is an int"
                )]
                let msec = msec as i32;
                // SAFETY: its live fields.
                unsafe { (*fields).timeout_msec = msec };
            }
            _ => {
                swr_v1_timer_system_set(msec);
            }
        }
    }

    fn reinit(&self, manually_trigger: bool) {
        self.set(-1);
        self.init(manually_trigger);
        self.set(self.next_msec.get());
    }

    /// Runs a detached node's destructor and deletes it, then raises a pending
    /// bailout. The destructor may longjmp over these frames, which hold
    /// nothing to drop.
    fn release(&self, node: *mut TimerNode) {
        let (destructor, plain_drop, data) = {
            // SAFETY: a node whose custody Rust just returned.
            let node = unsafe { &*node };
            (node.destructor, node.plain_drop, node.data)
        };
        if let Some(destructor) = destructor {
            // SAFETY: the node's own destructor.
            unsafe { destructor(node) };
        } else if let Some(plain_drop) = plain_drop {
            // SAFETY: the node's destructor of its own data.
            unsafe { plain_drop(data) };
        }
        // SAFETY: `add` boxed it; the queue no longer holds it.
        drop(unsafe { Box::from_raw(node) });
        if let Some(bailout) = self.pending_bailout.take() {
            // SAFETY: abandon_selection's bailout; it longjmps over plain frames.
            unsafe { bailout() };
        }
    }

    pub(crate) fn add(&self, msec: c_long, persistent: bool, node: TimerNode) -> *mut TimerNode {
        if msec <= 0 {
            warning(c"add", format_args!("msec value[{msec}] is invalid"));
            return std::ptr::null_mut();
        }
        let msec = swr_v1_timer_add_delay(msec, persistent);
        let node = Box::into_raw(Box::new(node));
        let now = self.relative_msec();
        let (status, view) = self
            .queue
            .borrow_mut()
            .add(now, msec, persistent, node.cast());
        if status != OK {
            warning(
                c"add",
                format_args!("Rust timer add failed: status={status}"),
            );
            // SAFETY: the node was never handed to the queue.
            drop(unsafe { Box::from_raw(node) });
            return std::ptr::null_mut();
        }
        let node = apply(&view);
        let next = swr_v1_timer_rearm_after_add(self.next_msec.get(), msec);
        if next != self.next_msec.get() {
            self.set(next);
            self.next_msec.set(next);
        }
        node
    }

    /// Whether `id` was removed.
    pub(crate) fn remove(&self, id: c_long) -> bool {
        let (status, view) = self.queue.borrow_mut().remove(id);
        match status {
            DEFERRED => {
                apply(&view);
                true
            }
            RELEASE => {
                self.release(apply(&view));
                true
            }
            _ => false,
        }
    }

    /// Refreshes a node's deadline from its caller-mutated `exec_msec`.
    #[cfg(test)]
    fn update(&self, node: &TimerNode) {
        let (status, view) = {
            let mut queue = self.queue.borrow_mut();
            match queue.update(node.id, node.exec_msec) {
                (OK, view) => (OK, view),
                _ => queue.get(node.id),
            }
        };
        if status == OK {
            apply(&view);
        }
    }

    /// Moves a node's deadline to `delay_ms` from now.
    pub(crate) fn delay(&self, node: &TimerNode, delay_ms: c_long) {
        let Some(deadline) =
            swr_v1_timer_delay_deadline(self.relative_msec(), node.exec_msec, delay_ms)
        else {
            return;
        };
        let (status, view) = self.queue.borrow_mut().update(node.id, deadline);
        if status == OK {
            apply(&view);
        }
    }

    /// Completes the active node.
    fn complete(&self, id: i64) {
        let (status, view) = self.queue.borrow_mut().complete(id);
        match status {
            OK => {
                apply(&view);
            }
            RELEASE | RELEASE_OVERFLOW => {
                self.release(apply(&view));
                if status == RELEASE_OVERFLOW {
                    warning(c"select", format_args!("Rust timer repeat limit exceeded"));
                }
            }
            _ => check(status),
        }
    }

    /// Ends the selection and arms the next wakeup; false if Rust refused.
    fn end(&self) -> bool {
        let (status, delay) = self.queue.borrow_mut().end();
        if status != OK {
            return false;
        }
        self.next_msec.set(delay);
        self.set(delay);
        true
    }

    /// Runs every due node. A Zend bailout longjmps out of a callback over these
    /// frames, which hold nothing to drop; a C++ exception aborts at the first
    /// Rust frame (`panic = "abort"`), as an uncaught one would.
    pub(crate) fn select(&self) {
        let now = self.relative_msec();
        let begin = self.queue.borrow_mut().begin(now);
        if begin == BUSY || begin == CLOSING {
            return;
        }
        check(begin);
        loop {
            let (status, view) = self.queue.borrow_mut().select_next();
            if status == EMPTY || status == CLOSING {
                break;
            }
            check(status);
            let node = apply(&view);
            // SAFETY: the node the queue just selected.
            let node_ref = unsafe { &*node };
            if !node_ref.removed
                && let Some(callback) = node_ref.callback
            {
                // SAFETY: the node's own callback, of this timer and the node.
                unsafe { callback(std::ptr::from_ref(self).cast_mut().cast(), node) };
            }
            self.complete(view.id);
        }
        if !self.end() {
            check(CLOSING);
        }
    }

    /// Only after a caught Zend bailout whose selection will never resume.
    pub(crate) fn abandon(&self, bailout: Option<Bailout>) {
        self.abandoned.set(true);
        self.pending_bailout.set(bailout);
    }

    /// `~Timer`: releases every node, then the queue. A forked child or terminal
    /// Zend bailout will never resume an active selection, so it is closed here;
    /// same-process deletion during a live callback aborts.
    pub(crate) fn destroy(&self) {
        self.queue.borrow_mut().close();
        self.set(-1);
        if self.abandoned.get() || std::process::id().cast_signed() != self.owner_pid {
            let (active, view) = {
                let mut queue = self.queue.borrow_mut();
                let stats = queue.stats();
                let (status, view) = if stats.active_id == 0 {
                    (RELEASE, TimerView::default())
                } else {
                    queue.complete(stats.active_id)
                };
                if status != RELEASE {
                    std::process::abort();
                }
                if stats.selecting != 0 {
                    check(queue.end().0);
                }
                (stats.active_id != 0, view)
            };
            if active {
                self.release(apply(&view));
            }
        }
        let status = loop {
            let (status, view) = self.queue.borrow_mut().drain();
            if status != RELEASE {
                break status;
            }
            self.release(apply(&view));
        };
        // The record's drop frees the drained queue.
        if status != EMPTY || !self.queue.borrow().idle() {
            std::process::abort();
        }
    }
}

/// Hooks this thread's timer to `reactor`, the thread's: select after each loop
/// round, keep the loop while a timer is pending, and free the timer with it.
fn attach(reactor: *mut c_void) {
    swr_v1_event_set_end_callback(PRIORITY_TIMER, crate::wrapper::swoole_timer_select);
    swr_v1_event_set_exit_condition(EXIT_CONDITION_TIMER, idle);
    // SAFETY: `reactor` is the thread's, whose destroy queue this is; `destroyed`
    // takes it back.
    unsafe { event_api::on_destroy_with(destroyed, reactor) };
}

/// The exit condition: no timer is pending.
extern "C-unwind" fn idle(_event_num: &mut usize) -> bool {
    crate::wrapper::current().is_none_or(|timer| timer.count() == 0)
}

/// The reactor's destroy callback: frees this thread's timer, closing a
/// selection a bailout left.
unsafe extern "C-unwind" fn destroyed(reactor: *mut c_void) {
    let Some(timer) = crate::wrapper::current() else {
        return;
    };
    // The reactor being destroyed, which `attach` registered.
    let fields = ReactorPrepared::of(reactor).fields;
    // SAFETY: its live fields.
    if unsafe { (*fields).bailout } {
        timer.abandon(None);
    }
    crate::wrapper::swoole_timer_free();
}

/// `delete timer`.
///
/// # Safety
/// `timer` is a live timer from [`Owner::create`], freed once.
pub(crate) unsafe fn free(timer: *mut Owner) {
    // SAFETY: as the caller promises.
    unsafe { &*timer }.destroy();
    // SAFETY: Owner::create's allocation, no longer referenced.
    drop(unsafe { Box::from_raw(timer) });
}

/// The timer's queue, for `Server::get_timer_stats`.
#[must_use]
pub fn owner_queue(timer: &Owner) -> *mut Queue {
    timer.queue.as_ptr()
}

/// `Timer::get_reactor()`.
#[must_use]
pub fn owner_reactor(timer: &Owner) -> *mut c_void {
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
    use std::rc::Rc;

    /// `Owner::init` declares the reactor's `swoole_signal_set` by symbol: naming it
    /// links its object into the test binary on every linker.
    #[used]
    static SIGNAL_SET: unsafe extern "C" fn(c_int, usize, bool) -> usize =
        swoole_rs_reactor::signal_upstream::swoole_signal_set;

    /// The reactor's signal code calls `Socket::free`, which crates/tls defines and
    /// a manual timer never reaches; crates/tls depends on this one, so the test
    /// binary links it from here.
    #[unsafe(export_name = "_ZN6swoole7network6Socket4freeEv")]
    extern "C" fn socket_free() {
        unreachable!("a manual timer frees no socket");
    }

    type Call = Box<dyn FnMut(&Owner, *mut TimerNode)>;

    /// A node's closures, its data, as probe.cc's `Hooks`.
    struct Hooks {
        call: Call,
        destructor: Box<dyn FnMut()>,
    }

    unsafe extern "C-unwind" fn call(timer: *mut c_void, node: *mut TimerNode) {
        let hooks = unsafe { (*node).data.cast::<Hooks>() };
        unsafe { ((*hooks).call)(&*timer.cast::<Owner>(), node) };
    }

    unsafe extern "C-unwind" fn destruct(node: *mut TimerNode) {
        let mut hooks = unsafe { Box::from_raw((*node).data.cast::<Hooks>()) };
        (hooks.destructor)();
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
        fn add(&self, label: char, repeat: bool, call_back: Call) -> *mut TimerNode {
            let log = Rc::clone(&self.log);
            let hooks = Box::into_raw(Box::new(Hooks {
                call: call_back,
                destructor: Box::new(move || {
                    let mut log = log.borrow_mut();
                    log.destructors += 1;
                    log.events.push(label.to_ascii_lowercase());
                }),
            }));
            let node = self
                .timer()
                .add(1000, repeat, TimerNode::new(Some(call), hooks.cast()));
            assert!(!node.is_null(), "add failed");
            unsafe { (*node).exec_msec = 0 };
            self.timer().update(unsafe { &*node });
            unsafe { (*node).destructor = Some(destruct) };
            node
        }
        fn record(&self, label: char) -> Call {
            let log = Rc::clone(&self.log);
            Box::new(move |_, _| log.borrow_mut().events.push(label))
        }
        /// Replaces a node's destructor, logging as `add`'s does first.
        fn on_destroy(&self, node: *mut TimerNode, label: char, then: impl FnMut() + 'static) {
            let log = Rc::clone(&self.log);
            let mut then = then;
            let hooks = unsafe { (*node).data.cast::<Hooks>() };
            unsafe {
                (*hooks).destructor = Box::new(move || {
                    {
                        let mut log = log.borrow_mut();
                        log.destructors += 1;
                        log.events.push(label);
                    }
                    then();
                });
            }
        }
        fn present(&self, id: c_long) -> bool {
            id != 0 && self.timer().get(id).is_some()
        }
        fn free(&self) {
            unsafe { free(self.timer) };
        }
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
    #[cfg_attr(miri, ignore = "the reactor's natives")]
    fn equal_deadlines() {
        let p = Probe::new();
        for label in ['A', 'B', 'C'] {
            p.add(label, false, p.record(label));
        }
        expect(&finish(&p, 0, 0, false), "AaCcBb", "AaCcBb", 0, 3, false);
    }

    #[test]
    #[cfg_attr(miri, ignore = "the reactor's natives")]
    fn self_remove() {
        let p = Probe::new();
        let log = Rc::clone(&p.log);
        p.add(
            'A',
            true,
            Box::new(move |t, n| {
                let id = unsafe { (*n).id };
                let mut log = log.borrow_mut();
                log.events.push('A');
                log.events.push(if t.remove(id) { 'Y' } else { 'N' });
                log.events.push(if t.remove(id) { 'Y' } else { 'N' });
                let viewed = t.token(id) == n && unsafe { (*n).removed };
                log.events.push(if viewed { 'V' } else { 'X' });
            }),
        );
        expect(&finish(&p, 0, 0, false), "AYNVa", "AYNVa", 0, 1, false);
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
        other.set(unsafe { (*p.add('B', false, p.record('B'))).id });
        p.add('C', false, p.record('C'));
        if in_destructor {
            let (timer, target) = (p.timer(), Rc::clone(&other));
            p.on_destroy(a, 'a', move || {
                timer.remove(target.get());
            });
        }
        finish(&p, 0, 0, false)
    }

    #[test]
    #[cfg_attr(miri, ignore = "the reactor's natives")]
    fn remove_other_in_a_callback() {
        expect(&remove_other(false), "AbaCc", "AbaCc", 0, 3, false);
    }

    #[test]
    #[cfg_attr(miri, ignore = "the reactor's natives")]
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
            Box::new(move |t, n| {
                log.borrow_mut().events.push('A');
                if repeat {
                    unsafe { (*n).exec_msec = 100_000 };
                    t.update(unsafe { &*n });
                } else {
                    t.delay(unsafe { &*n }, 100_000);
                }
                flag.set(true);
            }),
        );
        let b = p.add('B', false, p.record('B'));
        let (a, b) = unsafe { ((*a).id, (*b).id) };
        let got = finish(&p, a, b, false);
        assert!(updated.get(), "the callback updated its node");
        got
    }

    #[test]
    #[cfg_attr(miri, ignore = "the reactor's natives")]
    fn update_active_repeat() {
        expect(&move_active(true), "ABb", "ABba", 1, 2, true);
    }

    #[test]
    #[cfg_attr(miri, ignore = "the reactor's natives")]
    fn delay_active_once() {
        expect(&move_active(false), "AaBb", "AaBb", 0, 2, false);
    }

    #[test]
    #[cfg_attr(miri, ignore = "the reactor's natives")]
    fn destructor_add() {
        let p = Probe::new();
        let a = p.add('A', false, p.record('A'));
        let probe = Probe {
            timer: p.timer,
            log: Rc::clone(&p.log),
        };
        p.on_destroy(a, 'a', move || {
            probe.add('C', false, probe.record('C'));
        });
        p.add('B', false, p.record('B'));
        expect(&finish(&p, 0, 0, true), "AaBb", "AaBbCc", 1, 3, false);
    }

    #[test]
    #[cfg_attr(miri, ignore = "the reactor's natives")]
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
        let id = unsafe { (*p.add('A', false, p.record('A'))).id };
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
            Box::new(move |t, node| {
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
                let id = unsafe { (*node).id };
                parent_intact
                    .set(t.token(id) == node && t.count() == 2 && log.borrow().destructors == 0);
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
    #[cfg_attr(miri, ignore = "the reactor's natives")]
    fn wrapper_clear() {
        use crate::wrapper::{publish, swoole_timer_clear, swoole_timer_get, timer as slot_value};
        let p = Probe::new();
        let previous = slot_value();
        publish(p.timer);
        let inactive_id = unsafe { (*p.add('C', false, p.record('C'))).id };
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
            Box::new(move |_, node| {
                log.borrow_mut().events.push('A');
                let cancelled_other =
                    swoole_timer_clear(target.get()) && !swoole_timer_clear(target.get());
                let id = unsafe { (*node).id };
                let active = swoole_timer_clear(id) && !swoole_timer_clear(id);
                let active_view = swoole_timer_get(id) == node && unsafe { (*node).removed };
                flags.set((active, cancelled_other, active_view));
            }),
        );
        other.set(unsafe { (*p.add('B', false, p.record('B'))).id });
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
}
