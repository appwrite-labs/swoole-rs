// SPDX-License-Identifier: Apache-2.0
// Compatibility derived from Swoole v6.2.4 src/core/timer.cc (Swoole contributors).
//! `swoole::Timer` (upstream `src/core/timer.cc`, which the timer patch deletes): a
//! Rust-owned record over this crate's queue, and its nodes. C++ sees both only
//! through pointers.
use crate::{
    Status, Timer as Queue, TimerView, absolute_msec, swr_v1_timer_add_delay, swr_v1_timer_backend,
    swr_v1_timer_delay_deadline, swr_v1_timer_rearm_after_add, swr_v1_timer_relative_msec,
    swr_v1_timer_system_set,
};
use std::cell::{Cell, RefCell};
use std::ffi::{CStr, c_int, c_long, c_void};
use swoole_rs_corebase::CCast;
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

/// `swoole::TimerCallback`: `void (*)(Timer *, TimerNode *)`.
pub(crate) type Callback = unsafe extern "C-unwind" fn(timer: *mut c_void, node: *mut TimerNode);
/// `swoole::TimerDestructor`: `void (*)(TimerNode *)`.
type Destructor = unsafe extern "C-unwind" fn(node: *mut TimerNode);
/// A callback of the node's data alone, for `swr_v1_timer_add_plain`'s callers.
pub type Plain = swoole_rs_reactor::DeferCall;
/// `abandon_selection`'s pending Zend bailout.
pub type Bailout = unsafe extern "C-unwind" fn();

/// A node's fields, for `Swoole\Timer::info()`.
#[derive(Debug, Clone, Copy)]
pub struct Info {
    pub exec_msec: i64,
    pub exec_count: u64,
    pub interval: i64,
    pub round: u64,
    pub removed: bool,
}

/// `swoole::TimerNode`. Only `TimerNode::new` makes one, so `plain` can trail the head
/// the test fixtures read (tests/modules/hooks/system and http/server), which the
/// assertion below pins.
#[repr(C)]
pub struct TimerNode {
    pub(crate) id: c_long,
    /// Upstream's `type == TYPE_PHP`: a [`TimerNode::php`] node.
    php: bool,
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
    /// A [`TimerNode::closure`] node's callback.
    closure: Option<Closure>,
}
const _: () = assert!(
    std::mem::offset_of!(TimerNode, exec_msec) == 16
        && std::mem::offset_of!(TimerNode, removed) == 48
        && std::mem::offset_of!(TimerNode, destructor) == 72,
    "the TimerNode head the fixtures read"
);

pub(crate) use closure::Closure;

/// A `Copy` callback of at most two words, held in its node: no allocation, and
/// nothing to free when the node goes.
mod closure {
    use std::mem::{MaybeUninit, align_of, size_of};

    type Env = MaybeUninit<[usize; 2]>;

    #[derive(Clone, Copy)]
    pub(crate) struct Closure {
        call: fn(&Env),
        env: Env,
    }

    impl Closure {
        pub(crate) fn new<F: Fn() + Copy + 'static>(f: F) -> Self {
            const {
                assert!(
                    size_of::<F>() <= size_of::<Env>() && align_of::<F>() <= align_of::<Env>(),
                    "a timer closure holds at most two words"
                );
            };
            let mut env = Env::uninit();
            // SAFETY: `F` fits `Env`'s size and alignment, as asserted above.
            unsafe { env.as_mut_ptr().cast::<F>().write(f) };
            Self {
                call: call::<F>,
                env,
            }
        }

        pub(crate) fn run(&self) {
            (self.call)(&self.env);
        }
    }

    /// Only [`Closure::new`] pairs `call::<F>` with an env, one that holds an `F`.
    fn call<F: Fn()>(env: &Env) {
        // SAFETY: `env` holds the `F` `Closure::new` wrote, and `F` is `Copy`.
        let f = unsafe { &*env.as_ptr().cast::<F>() };
        f();
    }

    #[cfg(test)]
    mod tests {
        use super::Closure;
        use std::cell::Cell;

        thread_local! {
            static RAN: Cell<usize> = const { Cell::new(0) };
        }

        #[test]
        fn a_copy_runs_the_two_words_it_captured() {
            let (tens, ones) = (3usize, 4usize);
            let closure = Closure::new(move || RAN.set(tens * 10 + ones));
            let copy = closure;
            copy.run();
            assert_eq!(RAN.get(), 34);
        }
    }
}

impl TimerNode {
    pub(crate) fn new(callback: Option<Callback>, data: *mut c_void) -> Self {
        Self {
            id: 0,
            php: false,
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
            closure: None,
        }
    }

    /// A node whose callback is `f`, which `Owner::select` calls directly.
    pub(crate) fn closure(f: Closure) -> Self {
        let mut node = Self::new(None, std::ptr::null_mut());
        node.closure = Some(f);
        node
    }

    /// A node whose callback is `plain(data)`, which `Owner::select` calls directly.
    pub(crate) fn plain(plain: Plain, data: *mut c_void) -> Self {
        let mut node = Self::new(None, data);
        node.plain = Some(plain);
        node
    }

    /// A PHP node: `callback(data)` on each run, `destructor(data)` once
    /// it is removed.
    pub(crate) fn php(callback: Plain, destructor: Plain, data: *mut c_void) -> Self {
        let mut node = Self::plain(callback, data);
        node.php = true;
        node.plain_drop = Some(destructor);
        node
    }

    pub(crate) fn is_php(&self) -> bool {
        self.php
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

/// A queue status that only a broken invariant returns (upstream threw).
fn check(status: Status) {
    if status != Status::Ok {
        swoole_rs_corebase::fatal("a timer queue operation failed");
    }
}

fn warning(origin: &CStr, message: std::fmt::Arguments<'_>) {
    log::error_log(LOG_WARNING, INVALID_PARAMS, origin, message);
}

/// The system timer's `SIGALRM` handler.
extern "C" fn alarm(_signo: c_int) {
    swr_v1_core_set_signal_alarm(true);
}

/// What a node calls, copied out of it so that no borrow of it spans a call.
#[derive(Clone, Copy)]
struct Calls {
    closure: Option<Closure>,
    plain: Option<Plain>,
    callback: Option<Callback>,
    data: *mut c_void,
    destructor: Option<Destructor>,
    plain_drop: Option<Plain>,
}

/// Copies a view Rust hands back into its node, so the node is current: the node,
/// and what it calls as of now.
#[inline]
fn applied(view: &TimerView) -> (*mut TimerNode, Calls) {
    let node = view.token.cast::<TimerNode>();
    // SAFETY: every token is a live TimerNode that `add` made.
    let node_ref = unsafe { &mut *node };
    node_ref.id = view.id;
    node_ref.exec_msec = view.deadline;
    node_ref.interval = view.interval;
    node_ref.exec_count = view.exec_count;
    node_ref.round = view.birth_round;
    node_ref.removed = view.removed;
    let calls = Calls {
        closure: node_ref.closure,
        plain: node_ref.plain,
        callback: node_ref.callback,
        data: node_ref.data,
        destructor: node_ref.destructor,
        plain_drop: node_ref.plain_drop,
    };
    (node, calls)
}

/// [`applied`]'s node.
fn apply(view: &TimerView) -> *mut TimerNode {
    applied(view).0
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
        let timer = Box::new(Owner {
            reactor: Cell::new(std::ptr::null_mut()),
            queue: RefCell::default(),
            owner_pid: std::process::id().cast_signed(),
            backend: Cell::new(MANUAL),
            abandoned: Cell::new(false),
            pending_bailout: Cell::new(None),
            base_time: absolute_msec(),
            next_msec: Cell::new(-1),
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
            attach();
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
                let msec = msec.c_cast::<i32>();
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

    /// Runs the destructor of a detached node, whose custody Rust just returned
    /// ([`applied`]'s result), and deletes it, then raises a pending bailout. The
    /// destructor may longjmp over these frames, which hold nothing to drop.
    fn release(&self, (node, calls): (*mut TimerNode, Calls)) {
        if let Some(destructor) = calls.destructor {
            // SAFETY: the node's own destructor.
            unsafe { destructor(node) };
        } else if let Some(plain_drop) = calls.plain_drop {
            // SAFETY: the node's destructor of its own data.
            unsafe { plain_drop(calls.data) };
        }
        // SAFETY: `add` boxed it; the queue no longer holds it.
        drop(unsafe { Box::from_raw(node) });
        if let Some(bailout) = self.pending_bailout.take() {
            // SAFETY: abandon_selection's bailout; it longjmps over plain frames.
            unsafe { bailout() };
        }
    }

    /// The added node and its id, or none.
    pub(crate) fn add(
        &self,
        msec: c_long,
        persistent: bool,
        node: TimerNode,
    ) -> Option<(*mut TimerNode, c_long)> {
        if msec <= 0 {
            warning(c"add", format_args!("msec value[{msec}] is invalid"));
            return None;
        }
        let msec = swr_v1_timer_add_delay(msec, persistent);
        let now = self.relative_msec();
        // The queue boxes the node only once it has room for it, so a refused node
        // drops here with the closure.
        let (status, view) = self.queue.borrow_mut().add(now, msec, persistent, || {
            Box::into_raw(Box::new(node)).cast()
        });
        if status != Status::Ok {
            warning(
                c"add",
                format_args!("Rust timer add failed: status={status:?}"),
            );
            return None;
        }
        let node = apply(&view);
        let next = swr_v1_timer_rearm_after_add(self.next_msec.get(), msec);
        if next != self.next_msec.get() {
            self.set(next);
            self.next_msec.set(next);
        }
        Some((node, view.id))
    }

    /// Whether `id` was removed.
    pub(crate) fn remove(&self, id: c_long) -> bool {
        let (status, view) = self.queue.borrow_mut().remove(id);
        match status {
            Status::Deferred => {
                apply(&view);
                true
            }
            Status::Release => {
                self.release(applied(&view));
                true
            }
            _ => false,
        }
    }

    /// Refreshes node `id`'s deadline from its caller-mutated `exec_msec`. Both by
    /// value, as for [`Self::delay`].
    #[cfg(test)]
    fn update(&self, id: i64, exec_msec: i64) {
        let (status, view) = {
            let mut queue = self.queue.borrow_mut();
            match queue.update(id, exec_msec) {
                (Status::Ok, view) => (Status::Ok, view),
                _ => queue.get(id),
            }
        };
        if status == Status::Ok {
            apply(&view);
        }
    }

    /// Moves node `id`'s deadline, now `exec_msec`, to `delay_ms` from now. The node's
    /// fields come by value: `apply` writes the node, so no reference to it may be live.
    pub(crate) fn delay(&self, id: i64, exec_msec: i64, delay_ms: c_long) {
        let Some(deadline) = swr_v1_timer_delay_deadline(self.relative_msec(), exec_msec, delay_ms)
        else {
            return;
        };
        let (status, view) = self.queue.borrow_mut().update(id, deadline);
        if status == Status::Ok {
            apply(&view);
        }
    }

    /// Completes the active node.
    fn complete(&self, id: i64) {
        let (status, view) = self.queue.borrow_mut().complete(id);
        match status {
            Status::Ok => {
                apply(&view);
            }
            Status::Release | Status::ReleaseOverflow => {
                self.release(applied(&view));
                if status == Status::ReleaseOverflow {
                    warning(c"select", format_args!("Rust timer repeat limit exceeded"));
                }
            }
            _ => check(status),
        }
    }

    /// Ends the selection and arms the next wakeup; false if Rust refused.
    fn end(&self) -> bool {
        let (status, delay) = self.queue.borrow_mut().end();
        if status != Status::Ok {
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
        // Most loop turns find no timer due.
        let idle = self.queue.borrow_mut().select_idle(now);
        if let Some(delay) = idle {
            self.next_msec.set(delay);
            self.set(delay);
            return;
        }
        let begin = self.queue.borrow_mut().begin(now);
        if begin == Status::Busy || begin == Status::Closing {
            return;
        }
        check(begin);
        loop {
            let (status, view) = self.queue.borrow_mut().select_next();
            if status == Status::Empty || status == Status::Closing {
                break;
            }
            check(status);
            // The node the queue just selected; the copies are read before it runs.
            let (node, calls) = applied(&view);
            if !view.removed {
                if let Some(closure) = calls.closure {
                    closure.run();
                } else if let Some(plain) = calls.plain {
                    // SAFETY: the node's function of its own data.
                    unsafe { plain(calls.data) };
                } else if let Some(callback) = calls.callback {
                    // SAFETY: the node's own callback, of this timer and the node.
                    unsafe { callback(std::ptr::from_ref(self).cast_mut().cast(), node) };
                }
            }
            self.complete(view.id);
        }
        if !self.end() {
            check(Status::Closing);
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
                    (Status::Release, TimerView::default())
                } else {
                    queue.complete(stats.active_id)
                };
                if status != Status::Release {
                    swoole_rs_corebase::fatal("a timer completed without releasing");
                }
                if stats.selecting {
                    check(queue.end().0);
                }
                (stats.active_id != 0, view)
            };
            if active {
                self.release(applied(&view));
            }
        }
        let status = loop {
            let (status, view) = self.queue.borrow_mut().drain();
            if status != Status::Release {
                break status;
            }
            self.release(applied(&view));
        };
        // The record's drop frees the drained queue.
        if status != Status::Empty || !self.queue.borrow().idle() {
            swoole_rs_corebase::fatal("a timer queue still busy when dropped");
        }
    }
}

/// Hooks this thread's timer to the thread's reactor: select after each loop
/// round, keep the loop while a timer is pending, and free the timer with it.
fn attach() {
    swr_v1_event_set_end_callback(
        swoole_rs_reactor::Priority::Timer,
        crate::wrapper::swoole_timer_select,
    );
    swr_v1_event_set_exit_condition(swoole_rs_reactor::ExitCondition::Timer, idle);
    event_api::on_destroy(destroyed);
}

/// The exit condition: no timer is pending.
extern "C-unwind" fn idle(_event_num: &mut usize) -> bool {
    crate::wrapper::current().is_none_or(|timer| timer.count() == 0)
}

/// The reactor's destroy callback: frees this thread's timer, closing a
/// selection a bailout left. `swoole_event_free` destroys the thread's reactor
/// before it clears it, so the thread's reactor is the one being destroyed.
extern "C-unwind" fn destroyed(_data: *mut c_void) {
    let Some(timer) = crate::wrapper::current() else {
        return;
    };
    if event_api::swoole_event_bailed_out() {
        timer.abandon(None);
    }
    crate::wrapper::swoole_timer_free();
}

/// The timer's queue, for `Server::get_timer_stats`.
#[must_use]
pub fn owner_queue(timer: &Owner) -> *mut Queue {
    timer.queue.as_ptr()
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

    /// `delete timer`, as `swoole_timer_free` does of the thread's: these timers are
    /// never published.
    unsafe fn free(timer: *mut Owner) {
        unsafe { &*timer }.destroy();
        drop(unsafe { Box::from_raw(timer) });
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
            let (node, _) = self
                .timer()
                .add(1000, repeat, TimerNode::new(Some(call), hooks.cast()))
                .expect("add failed");
            unsafe { (*node).exec_msec = 0 };
            self.timer().update(unsafe { (*node).id }, 0);
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
    fn equal_deadlines() {
        let p = Probe::new();
        for label in ['A', 'B', 'C'] {
            p.add(label, false, p.record(label));
        }
        expect(&finish(&p, 0, 0, false), "AaCcBb", "AaCcBb", 0, 3, false);
    }

    #[test]
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
            Box::new(move |t, n| {
                log.borrow_mut().events.push('A');
                if repeat {
                    unsafe { (*n).exec_msec = 100_000 };
                    t.update(unsafe { (*n).id }, 100_000);
                } else {
                    let (id, exec_msec) = unsafe { ((*n).id, (*n).exec_msec) };
                    t.delay(id, exec_msec, 100_000);
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
        p.on_destroy(a, 'a', move || {
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
