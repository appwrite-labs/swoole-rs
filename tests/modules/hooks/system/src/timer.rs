//! Upstream's `core-tests/src/os/timer.cpp`, one function per TEST with the same inputs
//! and expected values, over the Rust timer (crates/timer).
//! `SwooleTG.timer` is crates/corebase's `thread_timer`.

use std::cell::{Cell, RefCell};
use std::ffi::{c_int, c_long, c_void};
use std::rc::{Rc, Weak};
use std::time::{SystemTime, UNIX_EPOCH};

use super::{SW_ERR, last_error, sleep};
use swoole_rs_timer::{
    Held, del_id, delay_held, delete_held, realtime_add, swoole_timer_clear, swoole_timer_exists,
    swoole_timer_free, swoole_timer_get, swoole_timer_select,
};

const SW_EVENTLOOP_WAIT_EXIT: c_int = 1;
const SW_ERROR_INVALID_PARAMS: c_int = 505;
const SW_NUM_MILLION: i64 = 1_000_000;

unsafe extern "C" {
    #[link_name = "_Z17swoole_event_initi"]
    fn swoole_event_init(flags: c_int) -> c_int;
    #[link_name = "_Z17swoole_event_waitv"]
    fn swoole_event_wait() -> c_int;
}

unsafe extern "C-unwind" {
    fn swr_v1_core_set_last_error(code: c_int);
}

/// Owned callback captures; the timer's last callback hold runs the destructor.
struct Hooks {
    call: RefCell<Box<dyn FnMut(Held)>>,
    destructor: Cell<Option<Box<dyn FnOnce()>>>,
}

impl Drop for Hooks {
    fn drop(&mut self) {
        if let Some(destructor) = self.destructor.take() {
            destructor();
        }
    }
}

/// The test may configure a live timer without extending its captures' lifetime.
struct Node {
    held: Held,
    hooks: Weak<Hooks>,
}

fn add(ms: c_long, persistent: bool, checked: bool, f: impl FnMut(Held) + 'static) -> Option<Node> {
    let hooks = Rc::new(Hooks {
        call: RefCell::new(Box::new(f)),
        destructor: Cell::new(None),
    });
    let weak = Rc::downgrade(&hooks);
    let issued = Rc::new(Cell::new(None));
    let slot = Rc::clone(&issued);
    let call = move || {
        (hooks.call.borrow_mut())(slot.get().expect("timer ID installed before selection"));
    };
    let held = if checked {
        swoole_rs_timer::schedule_owned(ms, persistent, call)
    } else {
        swoole_rs_timer::add_owned(ms, persistent, call)
    }?;
    issued.set(Some(held));
    Some(Node { held, hooks: weak })
}

pub(super) fn after(ms: c_long, f: impl FnMut(Held) + 'static) -> c_long {
    add(ms, false, true, f).map_or(c_long::from(SW_ERR), |node| node.held.id())
}

pub(super) fn tick(ms: c_long, f: impl FnMut(Held) + 'static) -> c_long {
    add(ms, true, true, f).map_or(c_long::from(SW_ERR), |node| node.held.id())
}

/// The original `tnode->destructor = f`, configured while the node is live.
fn set_destructor(node: &Node, f: impl FnOnce() + 'static) {
    node.hooks
        .upgrade()
        .unwrap()
        .destructor
        .set(Some(Box::new(f)));
}

/// `swoole::time<std::chrono::milliseconds>()`: the system clock.
/// Sets `SwooleTG.timer` to `timer` and returns the one it held.
fn timer_exchange(timer: *mut c_void) -> *mut c_void {
    let previous = swoole_rs_corebase::thread_timer();
    swoole_rs_corebase::set_thread_timer(timer);
    previous
}

fn now_ms() -> u64 {
    u64::try_from(
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_millis(),
    )
    .unwrap()
}

fn incr(cell: &Cell<i32>) -> i32 {
    cell.set(cell.get() + 1);
    cell.get()
}

fn event_init() {
    // SAFETY: this thread's reactor.
    unsafe { swoole_event_init(SW_EVENTLOOP_WAIT_EXIT) };
}

fn event_wait() {
    // SAFETY: after `event_init`.
    unsafe { swoole_event_wait() };
}

fn sys() {
    let counter = Rc::new([const { Cell::new(0) }; 4]);
    let ms1 = now_ms();

    let first = Rc::clone(&counter);
    assert!(add(20, false, false, move |_| _ = incr(&first[0])).is_some());

    // SAFETY: this thread's last error.
    unsafe { swr_v1_core_set_last_error(0) };
    assert!(add(-1, false, false, |_| {}).is_none());
    assert_eq!(last_error(), SW_ERROR_INVALID_PARAMS);

    // SAFETY: as above.
    unsafe { swr_v1_core_set_last_error(0) };
    assert!(add(0, false, false, |_| {}).is_none());
    assert_eq!(last_error(), SW_ERROR_INVALID_PARAMS);

    // dtor test
    let fired = Rc::clone(&counter);
    let timer = add(20, false, false, move |_| _ = incr(&fired[2]));
    assert!(timer.is_some());
    let destroyed = Rc::clone(&counter);
    set_destructor(timer.as_ref().unwrap(), move || destroyed[3].set(9999));

    let repeated = Rc::clone(&counter);
    _ = add(100, true, false, move |tnode| {
        if incr(&repeated[1]) == 5 {
            del_id(tnode);
        }
    });

    while swoole_rs_timer::php::stats().is_some_and(|(count, _)| count > 0) {
        // The signal-driven timer's select loop, as upstream's.
        // SAFETY: sleep(3).
        unsafe { sleep(10) };
        swoole_rs_process::signal::swoole_signal_dispatch();
        if swoole_rs_corebase::swr_v1_core_signal_alarm() {
            swoole_timer_select();
        }
    }

    let ms2 = now_ms();

    swoole_timer_free();

    assert!(ms2.wrapping_sub(ms1) <= 510);
    assert_eq!(counter[0].get(), 1);
    assert_eq!(counter[1].get(), 5);
    assert_eq!(counter[2].get(), 1);
    assert_eq!(counter[3].get(), 9999);
}

fn async_() {
    let timer1_count = Rc::new(Cell::new(0));
    let timer2_count = Rc::new(Cell::new(0));

    event_init();

    let ms1 = now_ms();
    let first = Rc::clone(&timer1_count);
    after(20, move |_| _ = incr(&first));

    let second = Rc::clone(&timer2_count);
    tick(100, move |tnode| {
        if incr(&second) == 5 {
            del_id(tnode);
        }
    });

    event_wait();
    let ms2 = now_ms();
    assert!(ms2.wrapping_sub(ms1) <= 510);
    assert_eq!(timer1_count.get(), 1);
    assert_eq!(timer2_count.get(), 5);
}

fn exists() {
    let timer_id = tick(100, |_| {});

    assert!(swoole_timer_exists(timer_id));
}

fn clear() {
    let timer_id = tick(100, |_| {});

    _ = swoole_timer_clear(timer_id);
    assert!(!swoole_timer_exists(timer_id));
}

fn get() {
    let timer_id = tick(100, |_| {});

    let timer_node = swoole_timer_get(timer_id).unwrap();
    assert_eq!(timer_node.id(), timer_id);
    swoole_timer_free();
}

fn delay() {
    event_init();
    let ms1 = now_ms();
    let ms2 = Rc::new(Cell::new(0));
    let fired = Rc::clone(&ms2);
    let timer_id = after(100, move |_| fired.set(now_ms()));

    let timer_node = swoole_timer_get(timer_id).unwrap();
    delay_held(Some(timer_node), 100);
    event_wait();
    assert!(ms2.get().wrapping_sub(ms1) >= 100);
    del_id(timer_node);
    swoole_timer_free();
}

fn error() {
    let tmp = timer_exchange(std::ptr::null_mut());

    swoole_timer_free();
    swoole_timer_select(); // no effect
    assert!(swoole_timer_get(1).is_none());
    assert!(!swoole_timer_clear(1));
    assert!(!swoole_timer_exists(1));

    let timer_id = tick(0, |_| {});
    assert_eq!(timer_id, c_long::from(SW_ERR));

    let timer_id = after(0, |_| {});
    assert_eq!(timer_id, c_long::from(SW_ERR));

    delay_held(None, 100);
    assert!(!delete_held(None));
    timer_exchange(tmp);

    swoole_timer_free();
}

fn reinit() {
    let timer1_count = Rc::new(Cell::new(0));
    let timer2_count = Rc::new(Cell::new(0));

    let first = Rc::clone(&timer1_count);
    after(20, move |_| _ = incr(&first));

    event_init();

    // `after` created this thread's timer, which the exchange puts back.
    let timer = timer_exchange(std::ptr::null_mut());
    timer_exchange(timer);
    // SAFETY: this thread's live timer.
    swoole_rs_timer::owner_reinit(unsafe { &*timer.cast() }, false);

    let ms1 = now_ms();

    let second = Rc::clone(&timer2_count);
    tick(100, move |tnode| {
        if incr(&second) == 5 {
            del_id(tnode);
        }
    });

    event_wait();
    let ms2 = now_ms();
    assert!(ms2.wrapping_sub(ms1) <= 510);
    assert_eq!(timer1_count.get(), 1);
    assert_eq!(timer2_count.get(), 5);
}

fn realtime_add_() {
    let mut ts = libc::timespec {
        tv_sec: 1,
        tv_nsec: 900 * SW_NUM_MILLION,
    };

    realtime_add(&mut ts, 1905);
    assert_eq!(ts.tv_sec, 3);
    assert_eq!(ts.tv_nsec, 805 * SW_NUM_MILLION);
}

pub(super) const CASES: super::Cases = &[
    ("sys", sys),
    ("async", async_),
    ("exists", exists),
    ("clear", clear),
    ("get", get),
    ("delay", delay),
    ("error", error),
    ("reinit", reinit),
    ("realtime_add", realtime_add_),
];
