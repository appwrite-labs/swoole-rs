// SPDX-License-Identifier: Apache-2.0
// Compatibility derived from Swoole v6.2.4 src/core/timer.cc (Swoole contributors).
//! `swoole::Timer` (upstream `src/core/timer.cc`, which the timer patch deletes): a
//! Rust-owned record over this crate's queue. `native/timer/swoole_timer.h` is its
//! C++ view; `native/timer/timer.cc` runs what only C++ can: a `TimerNode`'s
//! `std::function` callback and destructor, C++ exceptions, and the reactor's
//! `std::function` hooks.
use crate::{
    BUSY, CLOSING, DEFERRED, EMPTY, OK, RELEASE, RELEASE_OVERFLOW, Timer as Queue, TimerStats,
    TimerView, absolute_msec, swr_v1_timer_backend, swr_v1_timer_begin, swr_v1_timer_close,
    swr_v1_timer_complete, swr_v1_timer_create, swr_v1_timer_delay_deadline, swr_v1_timer_drain,
    swr_v1_timer_end, swr_v1_timer_get, swr_v1_timer_next, swr_v1_timer_rearm_after_add,
    swr_v1_timer_relative_msec, swr_v1_timer_release, swr_v1_timer_remove, swr_v1_timer_stats_get,
    swr_v1_timer_system_set, swr_v1_timer_update,
};
use std::cell::Cell;
use std::ffi::{CStr, c_int, c_long, c_void};
use swoole_rs_corebase::{log, swr_v1_core_set_signal_alarm, thread_reactor};

/// `SW_LOG_WARNING` and `SW_ERROR_INVALID_PARAMS`.
const LOG_WARNING: c_int = 4;
const INVALID_PARAMS: c_int = 505;
/// `swr_v1_timer_backend`'s modes.
const MANUAL: u32 = 0;
const REACTOR: u32 = 1;

/// `swoole::TimerNode`'s leading members; the rest (`callback`, `data`,
/// `destructor`) are C++'s. `native/timer/swoole_timer.h` asserts the offsets.
#[repr(C)]
pub(crate) struct TimerNode {
    pub(crate) id: c_long,
    kind: i32,
    pub(crate) exec_msec: i64,
    interval: i64,
    exec_count: u64,
    round: u64,
    pub(crate) removed: bool,
}

/// A `std::exception_ptr` native/timer/timer.cc caught, or null.
type Failure = *mut c_void;
/// `abandon_selection`'s pending Zend bailout.
type Bailout = unsafe extern "C-unwind" fn();

// SAFETY: native/timer/timer.cc's, and Swoole's signal API.
#[expect(
    improper_ctypes,
    reason = "Owner is opaque to C++, which only passes its address back"
)]
unsafe extern "C-unwind" {
    fn swr_cpp_timer_node_new(data: *mut c_void, callback: *const c_void) -> *mut TimerNode;
    fn swr_cpp_timer_node_call(timer: *const Owner, node: *mut TimerNode) -> Failure;
    fn swr_cpp_timer_node_release(node: *mut TimerNode) -> Failure;
    fn swr_cpp_timer_rethrow(failure: Failure) -> !;
    fn swr_cpp_timer_forget(failure: Failure);
    fn swr_cpp_timer_reactor_timeout(reactor: *mut c_void, msec: i64);
    fn swr_cpp_timer_attach(reactor: *mut c_void, timer: *const Owner);
    #[link_name = "_Z17swoole_signal_setiPFviEb"]
    fn swoole_signal_set(signo: c_int, handler: extern "C" fn(c_int), safety: bool) -> usize;
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

/// Fails over to the C++ caller: rethrows `failure`, if there is one.
fn rethrow(failure: Failure) {
    if !failure.is_null() {
        // SAFETY: a caught exception, rethrown once.
        unsafe { swr_cpp_timer_rethrow(failure) };
    }
}

/// `swoole::Timer`. Callbacks re-enter it, so every mutable field is a `Cell`.
#[repr(C)]
pub(crate) struct Owner {
    reactor: Cell<*mut c_void>,
    queue: *mut Queue,
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
        let mut queue = std::ptr::null_mut();
        // SAFETY: a writable handle.
        check(unsafe { swr_v1_timer_create(&raw mut queue) });
        let timer = Box::into_raw(Box::new(Owner {
            reactor: Cell::new(std::ptr::null_mut()),
            queue,
            // SAFETY: getpid has no preconditions.
            owner_pid: unsafe { libc::getpid() },
            backend: Cell::new(MANUAL),
            abandoned: Cell::new(false),
            pending_bailout: Cell::new(None),
            base_time: absolute_msec(),
            next_msec: Cell::new(-1),
        }));
        // SAFETY: just allocated; the reactor hooks keep its address, which never moves.
        unsafe { &*timer }.init(manually_trigger);
        timer
    }

    pub(crate) fn relative_msec(&self) -> i64 {
        swr_v1_timer_relative_msec(absolute_msec(), self.base_time)
    }

    pub(crate) fn next_msec(&self) -> i64 {
        self.next_msec.get()
    }

    pub(crate) fn queue(&self) -> *mut Queue {
        self.queue
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
            // SAFETY: the thread's live reactor; the timer outlives its hooks (the
            // reactor's destroy callback frees it).
            unsafe { swr_cpp_timer_attach(reactor, self) };
        } else if backend != MANUAL {
            // SAFETY: a plain signal handler.
            unsafe { swoole_signal_set(libc::SIGALRM, alarm, false) };
        }
    }

    /// Arms the backend's wakeup `msec` from now; negative disarms it.
    fn set(&self, msec: i64) {
        match self.backend.get() {
            MANUAL => {}
            // SAFETY: the reactor the backend attached to.
            REACTOR => unsafe { swr_cpp_timer_reactor_timeout(self.reactor.get(), msec) },
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
    /// bailout. Returns the destructor's exception instead.
    fn release(&self, node: *mut TimerNode) -> Failure {
        // SAFETY: a node whose custody Rust just returned.
        let failure = unsafe { swr_cpp_timer_node_release(node) };
        if failure.is_null()
            && let Some(bailout) = self.pending_bailout.take()
        {
            // SAFETY: abandon_selection's bailout; it longjmps over plain frames.
            unsafe { bailout() };
        }
        failure
    }

    pub(crate) fn add(
        &self,
        msec: c_long,
        persistent: bool,
        data: *mut c_void,
        callback: *const c_void,
    ) -> *mut TimerNode {
        if msec <= 0 {
            warning(c"add", format_args!("msec value[{msec}] is invalid"));
            return std::ptr::null_mut();
        }
        // SAFETY: `callback` is a live TimerCallback, as the caller promises.
        let node = unsafe { swr_cpp_timer_node_new(data, callback) };
        let mut view = TimerView::default();
        // SAFETY: the live queue, a fresh token and a writable view.
        let status = unsafe {
            crate::swr_v1_timer_add(
                self.queue,
                self.relative_msec(),
                msec,
                u32::from(persistent),
                node.cast(),
                &raw mut view,
            )
        };
        if status != OK {
            warning(
                c"add",
                format_args!("Rust timer add failed: status={status}"),
            );
            // SAFETY: the node was never handed to the queue; it has no destructor.
            let failure = unsafe { swr_cpp_timer_node_release(node) };
            rethrow(failure);
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

    pub(crate) fn remove(&self, id: c_long) -> bool {
        let mut view = TimerView::default();
        // SAFETY: the live queue and a writable view.
        match unsafe { swr_v1_timer_remove(self.queue, id, &raw mut view) } {
            DEFERRED => {
                apply(&view);
                true
            }
            RELEASE => {
                rethrow(self.release(apply(&view)));
                true
            }
            _ => false,
        }
    }

    /// Refreshes a node's deadline from its caller-mutated `exec_msec`.
    fn update(&self, node: &TimerNode) {
        let mut view = TimerView::default();
        // SAFETY: the live queue and a writable view.
        if unsafe { swr_v1_timer_update(self.queue, node.id, node.exec_msec, &raw mut view) } == OK
            // SAFETY: as above.
            || unsafe { swr_v1_timer_get(self.queue, node.id, &raw mut view) } == OK
        {
            apply(&view);
        }
    }

    /// Moves a node's deadline to `delay_ms` from now.
    pub(crate) fn delay(&self, node: &TimerNode, delay_ms: c_long) {
        let mut deadline = 0;
        // SAFETY: a writable deadline.
        if unsafe {
            swr_v1_timer_delay_deadline(
                self.relative_msec(),
                node.exec_msec,
                delay_ms,
                &raw mut deadline,
            )
        } == 0
        {
            return;
        }
        let mut view = TimerView::default();
        // SAFETY: the live queue and a writable view.
        if unsafe { swr_v1_timer_update(self.queue, node.id, deadline, &raw mut view) } == OK {
            apply(&view);
        }
    }

    /// Completes the active node; a released one's destructor exception, if any.
    fn complete(&self, id: i64) -> Failure {
        let mut view = TimerView::default();
        // SAFETY: the live queue and a writable view.
        let status = unsafe { swr_v1_timer_complete(self.queue, id, &raw mut view) };
        match status {
            OK => {
                apply(&view);
            }
            RELEASE | RELEASE_OVERFLOW => {
                let failure = self.release(apply(&view));
                if failure.is_null() && status == RELEASE_OVERFLOW {
                    warning(c"select", format_args!("Rust timer repeat limit exceeded"));
                }
                return failure;
            }
            _ => check(status),
        }
        std::ptr::null_mut()
    }

    /// Ends the selection and arms the next wakeup; false if Rust refused.
    fn end(&self) -> bool {
        let mut delay = -1;
        // SAFETY: the live queue and a writable delay.
        if unsafe { swr_v1_timer_end(self.queue, &raw mut delay) } != OK {
            return false;
        }
        self.next_msec.set(delay);
        self.set(delay);
        true
    }

    /// A callback or destructor threw: detach the active node, end the
    /// selection, and rethrow the first exception.
    fn fail(&self, primary: Failure) {
        let mut stats = TimerStats::default();
        // SAFETY: the live queue and writable stats.
        if unsafe { swr_v1_timer_stats_get(self.queue, &raw mut stats) } == OK
            && stats.active_id != 0
        {
            let secondary = self.complete(stats.active_id);
            if !secondary.is_null() {
                // SAFETY: a caught exception, dropped once.
                unsafe { swr_cpp_timer_forget(secondary) };
            }
        }
        self.end();
        rethrow(primary);
    }

    /// Runs every due node. A Zend bailout longjmps out of a callback over these
    /// frames, which hold nothing to drop.
    pub(crate) fn select(&self) {
        // SAFETY: the live queue.
        let begin = unsafe { swr_v1_timer_begin(self.queue, self.relative_msec()) };
        if begin == BUSY || begin == CLOSING {
            return;
        }
        check(begin);
        loop {
            let mut view = TimerView::default();
            // SAFETY: the live queue and a writable view.
            let status = unsafe { swr_v1_timer_next(self.queue, &raw mut view) };
            if status == EMPTY || status == CLOSING {
                break;
            }
            check(status);
            let node = apply(&view);
            // SAFETY: the node the queue just selected.
            if !unsafe { (*node).removed } {
                // SAFETY: as above.
                let failure = unsafe { swr_cpp_timer_node_call(self, node) };
                if !failure.is_null() {
                    self.fail(failure);
                }
            }
            let failure = self.complete(view.id);
            if !failure.is_null() {
                self.fail(failure);
            }
        }
        if !self.end() {
            check(CLOSING);
        }
    }

    /// `~Timer`: releases every node, then the queue. A forked child or terminal
    /// Zend bailout will never resume an active selection, so it is closed here;
    /// same-process deletion during a live callback aborts.
    pub(crate) fn destroy(&self) {
        check(
            // SAFETY: the live queue.
            unsafe { swr_v1_timer_close(self.queue) },
        );
        self.set(-1);
        let mut view = TimerView::default();
        // SAFETY: getpid has no preconditions.
        if self.abandoned.get() || unsafe { libc::getpid() } != self.owner_pid {
            let mut stats = TimerStats::default();
            // SAFETY: the live queue and writable stats.
            check(unsafe { swr_v1_timer_stats_get(self.queue, &raw mut stats) });
            if stats.active_id != 0
                // SAFETY: the live queue and a writable view.
                && unsafe { swr_v1_timer_complete(self.queue, stats.active_id, &raw mut view) }
                    != RELEASE
            {
                std::process::abort();
            }
            if stats.selecting != 0 {
                let mut delay = 0;
                // SAFETY: the live queue and a writable delay.
                check(unsafe { swr_v1_timer_end(self.queue, &raw mut delay) });
            }
            if stats.active_id != 0 && !self.release(apply(&view)).is_null() {
                std::process::abort();
            }
        }
        let status = loop {
            // SAFETY: the live queue and a writable view.
            let status = unsafe { swr_v1_timer_drain(self.queue, &raw mut view) };
            if status != RELEASE {
                break status;
            }
            if !self.release(apply(&view)).is_null() {
                std::process::abort();
            }
        };
        // SAFETY: the drained queue, released once.
        if status != EMPTY || unsafe { swr_v1_timer_release(self.queue) } != OK {
            std::process::abort();
        }
    }
}

/// The record behind `timer`.
///
/// # Safety
/// `timer` is a live timer from `swr_v1_timer_owner_new`.
unsafe fn owner<'a>(timer: *const Owner) -> &'a Owner {
    // SAFETY: as the caller promises.
    unsafe { &*timer }
}

/// `new Timer(manually_trigger)`.
#[unsafe(no_mangle)]
extern "C-unwind" fn swr_v1_timer_owner_new(manually_trigger: bool) -> *mut Owner {
    Owner::create(manually_trigger)
}

/// `delete timer`.
///
/// # Safety
/// `timer` is a live timer, freed once.
#[unsafe(no_mangle)]
pub(crate) unsafe extern "C-unwind" fn swr_v1_timer_owner_free(timer: *mut Owner) {
    // SAFETY: as the caller promises.
    unsafe { owner(timer) }.destroy();
    // SAFETY: Owner::create's allocation, no longer referenced.
    drop(unsafe { Box::from_raw(timer) });
}

/// The timer's queue, for the C++ view's reads.
///
/// # Safety
/// `timer` is live.
#[unsafe(no_mangle)]
unsafe extern "C" fn swr_v1_timer_owner_queue(timer: *const Owner) -> *mut Queue {
    // SAFETY: as the caller promises.
    unsafe { owner(timer) }.queue
}

/// `Timer::get_reactor()`.
///
/// # Safety
/// `timer` is live.
#[unsafe(no_mangle)]
unsafe extern "C" fn swr_v1_timer_owner_reactor(timer: *const Owner) -> *mut c_void {
    // SAFETY: as the caller promises.
    unsafe { owner(timer) }.reactor.get()
}

/// `Timer::reinit(manually_trigger)`.
///
/// # Safety
/// `timer` is live.
#[unsafe(no_mangle)]
unsafe extern "C-unwind" fn swr_v1_timer_owner_reinit(timer: *mut Owner, manually_trigger: bool) {
    // SAFETY: as the caller promises.
    unsafe { owner(timer) }.reinit(manually_trigger);
}

/// `Timer::add`: the new node, or null with a warning.
///
/// # Safety
/// `timer` is live and `callback` is a live `TimerCallback`.
#[unsafe(no_mangle)]
unsafe extern "C-unwind" fn swr_v1_timer_owner_add(
    timer: *mut Owner,
    msec: c_long,
    persistent: bool,
    data: *mut c_void,
    callback: *const c_void,
) -> *mut TimerNode {
    // SAFETY: as the caller promises.
    unsafe { owner(timer) }.add(msec, persistent, data, callback)
}

/// `Timer::remove(id)`.
///
/// # Safety
/// `timer` is live.
#[unsafe(no_mangle)]
unsafe extern "C-unwind" fn swr_v1_timer_owner_remove(timer: *mut Owner, id: c_long) -> bool {
    // SAFETY: as the caller promises.
    unsafe { owner(timer) }.remove(id)
}

/// `Timer::update(node)`.
///
/// # Safety
/// `timer` is live; `node` is null or one of its nodes.
#[unsafe(no_mangle)]
unsafe extern "C" fn swr_v1_timer_owner_update(timer: *mut Owner, node: *const TimerNode) {
    // SAFETY: null or a live node, as the caller promises.
    if let Some(node) = unsafe { node.as_ref() } {
        // SAFETY: as the caller promises.
        unsafe { owner(timer) }.update(node);
    }
}

/// `Timer::delay(node, delay_ms)`.
///
/// # Safety
/// `timer` is live; `node` is null or one of its nodes.
#[unsafe(no_mangle)]
unsafe extern "C" fn swr_v1_timer_owner_delay(
    timer: *mut Owner,
    node: *const TimerNode,
    delay_ms: c_long,
) {
    // SAFETY: null or a live node, as the caller promises.
    if let Some(node) = unsafe { node.as_ref() } {
        // SAFETY: as the caller promises.
        unsafe { owner(timer) }.delay(node, delay_ms);
    }
}

/// `Timer::select()`.
///
/// # Safety
/// `timer` is live.
#[unsafe(no_mangle)]
unsafe extern "C-unwind" fn swr_v1_timer_owner_select(timer: *mut Owner) {
    // SAFETY: as the caller promises.
    unsafe { owner(timer) }.select();
}

/// `Timer::abandon_selection(bailout)`: only after a caught Zend bailout whose
/// selection will never resume.
///
/// # Safety
/// `timer` is live.
#[unsafe(no_mangle)]
unsafe extern "C" fn swr_v1_timer_owner_abandon(timer: *mut Owner, bailout: Option<Bailout>) {
    // SAFETY: as the caller promises.
    let timer = unsafe { owner(timer) };
    timer.abandoned.set(true);
    timer.pending_bailout.set(bailout);
}
