//! Channel tests. `coroutine_channel` are upstream's core-tests/src/coroutine/channel.cpp
//! TESTs, one Rust test each with the same inputs and assertions; `lifetime_*` are the
//! two channel cases of the former tests/modules/coroutine/lifetime.cpp.
//!
//! [`rt`] stands in for Swoole's coroutines and event loop: each coroutine is a thread
//! and one baton decides which runs, so a resume switches to the coroutine until it
//! yields or ends, as `Coroutine::resume` does. Timers fire in deadline order once
//! every coroutine has yielded, and `run` returns when none is left, as
//! `swoole_event_wait` does with suspended coroutines still parked.
use super::*;
use core::ptr::null_mut;

mod rt {
    use std::cell::RefCell;
    use std::collections::{HashMap, HashSet};
    use std::sync::{Arc, Condvar, Mutex, MutexGuard};

    type Task = Box<dyn FnOnce() + Send>;

    #[derive(Default)]
    struct State {
        running: usize,
        origin: HashMap<usize, usize>,
        last_id: usize,
        alive: usize,
        now: f64,
        timers: Vec<(f64, usize, Task)>,
        last_timer: usize,
        cancel: HashMap<usize, Task>,
        canceled: HashSet<usize>,
        failure: Option<String>,
    }

    #[derive(Default)]
    pub(super) struct Runtime {
        state: Mutex<State>,
        baton: Condvar,
    }

    thread_local! { static CURRENT: RefCell<Option<(Arc<Runtime>, usize)>> = const { RefCell::new(None) }; }

    fn current() -> (Arc<Runtime>, usize) {
        CURRENT
            .with(|c| c.borrow().clone())
            .expect("inside rt::run")
    }

    fn lock(rt: &Runtime) -> MutexGuard<'_, State> {
        rt.state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    /// Hands the baton to `to` and waits for it to come back.
    fn switch(rt: &Runtime, me: usize, mut state: MutexGuard<'_, State>, to: usize) {
        state.running = to;
        rt.baton.notify_all();
        while state.running != me {
            state = rt
                .baton
                .wait(state)
                .unwrap_or_else(std::sync::PoisonError::into_inner);
        }
    }

    /// The running coroutine's id; 0 outside one.
    pub(super) fn me() -> usize {
        current().1
    }

    /// `Coroutine::resume`.
    pub(super) fn resume(co: usize) {
        let (rt, me) = current();
        let mut state = lock(&rt);
        state.origin.insert(co, me);
        switch(&rt, me, state, co);
    }

    /// `Coroutine::yield`, cancelable through `cancel` when given.
    pub(super) fn suspend(cancel: Option<Task>) {
        let (rt, me) = current();
        let mut state = lock(&rt);
        state.canceled.remove(&me);
        if let Some(cancel) = cancel {
            state.cancel.insert(me, cancel);
        }
        let origin = state.origin[&me];
        switch(&rt, me, state, origin);
        lock(&rt).cancel.remove(&me);
    }

    /// `Coroutine::cancel`.
    pub(super) fn cancel(co: usize) -> bool {
        let (rt, _) = current();
        let mut state = lock(&rt);
        let Some(cancel) = state.cancel.remove(&co) else {
            return false;
        };
        state.canceled.insert(co);
        drop(state);
        cancel();
        true
    }

    pub(super) fn is_canceled(co: usize) -> bool {
        lock(&current().0).canceled.contains(&co)
    }

    /// `Coroutine::create`: runs `f` until it first yields; returns its id.
    pub(super) fn create(f: impl FnOnce() + Send + 'static) -> usize {
        let (rt, _) = current();
        let id = {
            let mut state = lock(&rt);
            state.last_id += 1;
            state.alive += 1;
            state.last_id
        };
        let inner = Arc::clone(&rt);
        std::thread::spawn(move || {
            CURRENT.with(|c| *c.borrow_mut() = Some((Arc::clone(&inner), id)));
            let mut state = lock(&inner);
            while state.running != id {
                state = inner
                    .baton
                    .wait(state)
                    .unwrap_or_else(std::sync::PoisonError::into_inner);
            }
            drop(state);
            let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(f));
            let mut state = lock(&inner);
            if let Err(panic) = result {
                let text = panic.downcast_ref::<String>().cloned();
                let text = text.or_else(|| panic.downcast_ref::<&str>().map(|s| (*s).to_owned()));
                state.failure.get_or_insert(text.unwrap_or_default());
            }
            state.alive -= 1;
            state.running = state.origin[&id];
            inner.baton.notify_all();
        });
        resume(id);
        id
    }

    /// A one-shot timer; returns its id.
    pub(super) fn timer_add(seconds: f64, f: impl FnOnce() + Send + 'static) -> usize {
        let (rt, _) = current();
        let mut state = lock(&rt);
        state.last_timer += 1;
        let (deadline, id) = (state.now + seconds, state.last_timer);
        state.timers.push((deadline, id, Box::new(f)));
        id
    }

    pub(super) fn timer_del(id: usize) {
        lock(&current().0).timers.retain(|timer| timer.1 != id);
    }

    /// Live coroutines (`Coroutine::count`).
    pub(super) fn count() -> usize {
        lock(&current().0).alive
    }

    /// `swoole::test::coroutine::run`: create each coroutine, then run the timers
    /// until none is left. A failed assertion in a coroutine fails the caller.
    pub(super) fn run(fs: Vec<Box<dyn FnOnce() + Send>>) {
        CURRENT.with(|c| {
            c.borrow_mut()
                .get_or_insert_with(|| (Arc::new(Runtime::default()), 0));
        });
        for f in fs {
            create(f);
        }
        loop {
            let (rt, _) = current();
            let mut state = lock(&rt);
            let Some(next) = (0..state.timers.len()).min_by(|&a, &b| {
                let (a, b) = (&state.timers[a], &state.timers[b]);
                a.0.total_cmp(&b.0).then(a.1.cmp(&b.1))
            }) else {
                break;
            };
            let (deadline, _, f) = state.timers.remove(next);
            state.now = deadline;
            drop(state);
            f();
        }
        if let Some(failure) = lock(&current().0).failure.take() {
            panic!("{failure}");
        }
    }

    pub(super) fn run1(f: impl FnOnce() + Send + 'static) {
        run(vec![Box::new(f)]);
    }
}

/// A channel shared between test coroutines: only the baton holder touches it.
#[derive(Clone, Copy)]
struct Chan(usize);

/// The test runtime's effects, as Swoole's: a timer for a positive timeout and a
/// cancel function, each resuming the waiter.
struct Rt {
    timeout: f64,
}

impl Effects for Rt {
    type Waiter = *mut c_void;
    fn current(&self) -> *mut c_void {
        rt::me() as *mut c_void
    }

    fn wait(&mut self) -> u32 {
        use std::sync::{
            Arc,
            atomic::{AtomicBool, Ordering},
        };
        let co = rt::me();
        let timedout = Arc::new(AtomicBool::new(false));
        let fired = Arc::clone(&timedout);
        let timer = (self.timeout > 0.0).then(|| {
            rt::timer_add(self.timeout, move || {
                fired.store(true, Ordering::Relaxed);
                rt::resume(co);
            })
        });
        rt::suspend(Some(Box::new(move || rt::resume(co))));
        if let Some(timer) = timer {
            rt::timer_del(timer);
        }
        (if rt::is_canceled(co) { CANCELED } else { 0 })
            | (if timedout.load(Ordering::Relaxed) {
                TIMEDOUT
            } else {
                0
            })
    }

    fn resume(&mut self, co: *mut c_void) {
        rt::resume(co as usize);
    }
}

impl Chan {
    fn new(capacity: usize) -> Self {
        Self(Box::into_raw(Box::new(Channel::<*mut c_void>::new(capacity))) as usize)
    }
    fn ptr(self) -> *mut Channel<*mut c_void> {
        self.0 as *mut Channel<*mut c_void>
    }
    fn cell(self) -> &'static std::cell::UnsafeCell<Channel<*mut c_void>> {
        // SAFETY: live until `delete`; `UnsafeCell` is a transparent `Channel`.
        unsafe { &*self.ptr().cast() }
    }
    fn push(self, data: *mut c_void) -> bool {
        self.push_for(data, -1.0)
    }
    fn push_for(self, data: *mut c_void, timeout: f64) -> bool {
        push(self.cell(), &mut Rt { timeout }, data).is_ok()
    }
    fn pop(self) -> *mut c_void {
        self.pop_for(-1.0)
    }
    fn pop_for(self, timeout: f64) -> *mut c_void {
        pop(self.cell(), &mut Rt { timeout }).unwrap_or(null_mut())
    }
    fn close(self) -> bool {
        close(self.cell(), &mut Rt { timeout: 0.0 })
    }
    fn get_error(self) -> i32 {
        // SAFETY: as above.
        unsafe { (*self.ptr()).error() }
    }
    fn delete(self) {
        // SAFETY: made by `new`, deleted once.
        drop(unsafe { Box::from_raw(self.ptr()) });
    }
}

fn int(value: i32) -> *mut c_void {
    Box::into_raw(Box::new(value)).cast()
}

fn read(data: *mut c_void) -> i32 {
    // SAFETY: an `int` the test pushed.
    unsafe { *data.cast::<i32>() }
}

/// Frees an `int`, so the sanitizer run sees no leak.
fn free(data: *mut c_void) {
    // SAFETY: made by `int`, freed once.
    drop(unsafe { Box::from_raw(data.cast::<i32>()) });
}

/// Reads and frees a popped `int`.
fn take(data: *mut c_void) -> i32 {
    let value = read(data);
    free(data);
    value
}

#[test]
fn coroutine_channel_push_pop() {
    rt::run1(|| {
        let chan = Chan::new(1);
        let i = int(1);
        assert!(chan.push(i));
        assert_eq!(take(chan.pop()), 1);
        chan.delete();
    });
}

#[test]
fn coroutine_channel_push_yield() {
    let chan = Chan::new(1);
    rt::run(vec![
        Box::new(move || {
            assert!(chan.push(int(1)));
            assert!(chan.push(int(1)));
        }),
        Box::new(move || {
            assert_eq!(take(chan.pop()), 1);
            assert_eq!(take(chan.pop()), 1);
        }),
    ]);
    chan.delete();
}

#[test]
fn coroutine_channel_pop_yield() {
    let chan = Chan::new(1);
    let i = int(1) as usize;
    rt::run(vec![
        Box::new(move || {
            assert_eq!(read(chan.pop()), 1);
            assert_eq!(read(chan.pop()), 1);
        }),
        Box::new(move || {
            assert!(chan.push(i as *mut c_void));
            assert!(chan.push(i as *mut c_void));
        }),
    ]);
    free(i as *mut c_void);
    chan.delete();
}

#[test]
fn coroutine_channel_push_timeout() {
    rt::run1(|| {
        let chan = Chan::new(1);
        assert!(chan.push_for(null_mut(), 0.001));
        assert!(!chan.push_for(null_mut(), 0.001));
        chan.delete();
    });
}

#[test]
fn coroutine_channel_pop_timeout() {
    rt::run1(|| {
        let chan = Chan::new(1);
        assert_eq!(chan.pop_for(0.001), null_mut());
        chan.delete();
    });
}

#[test]
fn coroutine_channel_close() {
    let chan = Chan::new(1);
    let value = int(1) as usize;
    rt::run1(move || {
        let value = value as *mut c_void;
        loop {
            if !chan.push(value) {
                assert_eq!(chan.get_error(), ERROR_CLOSED);
                assert!(!chan.push(null_mut()));
                break;
            }
        }
    });
    assert!(chan.close());
    assert!(!chan.close());
    free(value as *mut c_void);
    chan.delete();

    let chan2 = Chan::new(1);
    rt::run1(move || {
        loop {
            if chan2.pop_for(0.0).is_null() {
                assert_eq!(chan2.get_error(), ERROR_CLOSED);
                assert_eq!(chan2.pop(), null_mut());
                break;
            }
        }
    });
    assert!(chan2.close());
    chan2.delete();
}

#[test]
fn coroutine_channel_cancel() {
    let chan = Chan::new(1);
    rt::run1(move || {
        let cid = rt::create(move || {
            assert_eq!(chan.pop(), null_mut());
            assert_eq!(chan.get_error(), ERROR_CANCELED);
        });
        assert!(rt::cancel(cid));
        assert!(chan.close());
        assert_eq!(chan.pop(), null_mut());
        assert_eq!(chan.get_error(), ERROR_CLOSED);
    });
    chan.delete();
}

#[test]
fn lifetime_pop_waiter_deletes_channel() {
    use std::sync::atomic::{AtomicBool, Ordering};
    static DELETED: AtomicBool = AtomicBool::new(false);
    let chan = Chan::new(1);
    let value = int(17) as usize;
    rt::run1(move || {
        rt::create(move || {
            assert!(chan.push(value as *mut c_void));
            assert!(chan.push(value as *mut c_void));
            chan.delete();
            DELETED.store(true, Ordering::Relaxed);
        });
        assert_eq!(chan.pop() as usize, value);
        assert!(DELETED.load(Ordering::Relaxed));
    });
    assert_eq!(rt::count(), 0);
    free(value as *mut c_void);
}

#[test]
fn lifetime_push_waiter_deletes_channel() {
    use std::sync::atomic::{AtomicBool, Ordering};
    static DELETED: AtomicBool = AtomicBool::new(false);
    let chan = Chan::new(1);
    let value = int(17) as usize;
    rt::run1(move || {
        rt::create(move || {
            assert_eq!(chan.pop() as usize, value);
            chan.delete();
            DELETED.store(true, Ordering::Relaxed);
        });
        assert!(chan.push(value as *mut c_void));
        assert!(DELETED.load(Ordering::Relaxed));
    });
    assert_eq!(rt::count(), 0);
    free(value as *mut c_void);
}

/// Records resumes; `wait` returns fixed flags without suspending.
struct Log {
    flags: u32,
    resumed: Vec<usize>,
}

impl Effects for Log {
    type Waiter = *mut c_void;
    fn current(&self) -> *mut c_void {
        9 as *mut c_void
    }
    fn wait(&mut self) -> u32 {
        self.flags
    }
    fn resume(&mut self, co: *mut c_void) {
        self.resumed.push(co as usize);
    }
}

#[test]
fn cancel_wins_over_timeout() {
    let mut ch = std::cell::UnsafeCell::new(Channel::<*mut c_void>::new(1));
    let mut fx = Log {
        flags: CANCELED | TIMEDOUT,
        resumed: vec![],
    };
    assert_eq!(pop(&ch, &mut fx), None);
    assert_eq!(ch.get_mut().error(), ERROR_CANCELED);
    fx.flags = TIMEDOUT;
    assert_eq!(pop(&ch, &mut fx), None);
    assert_eq!(ch.get_mut().error(), ERROR_TIMEOUT);
    assert_eq!(ch.get_mut().consumer_num(), 0);
}

#[test]
fn close_wakes_producers_then_consumers() {
    let mut ch = std::cell::UnsafeCell::new(Channel::<*mut c_void>::new(0));
    ch.get_mut()
        .producers
        .extend([1, 2].map(|n| n as *mut c_void));
    ch.get_mut().consumers.push_back(3 as *mut c_void);
    let mut fx = Log {
        flags: 0,
        resumed: vec![],
    };
    assert!(close(&ch, &mut fx));
    assert_eq!(fx.resumed, [1, 2, 3]);
}

#[test]
fn remove_drops_every_entry_of_a_waiter() {
    let t = |n: usize| n as *mut c_void;
    let mut ch = Channel::<*mut c_void>::new(1);
    ch.consumers.extend([t(1), t(2), t(1)]);
    ch.producers.push_back(t(1));
    ch.remove(Role::Consumer, t(1));
    assert_eq!(ch.consumer_num(), 1);
    ch.remove(Role::Producer, t(1));
    assert_eq!(ch.producer_num(), 0);
    assert_eq!(ch.pop_data(), None);
}
