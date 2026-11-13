//! Upstream's `core-tests/src/os/timer.cpp`, one function per TEST with the same inputs
//! and expected values, over the Rust timer (crates/timer) under its upstream symbols.
//! `SwooleTG.timer` goes through ../fixture.cc.

use std::cell::Cell;
use std::ffi::{c_int, c_long, c_void};
use std::time::{SystemTime, UNIX_EPOCH};

use super::{SW_ERR, last_error, sleep};

const SW_EVENTLOOP_WAIT_EXIT: c_int = 1;
const SW_ERROR_INVALID_PARAMS: c_int = 505;
const SW_NUM_MILLION: i64 = 1_000_000;

/// `swoole::TimerNode` (native/timer/swoole_timer.h): `id`, the members up to
/// `callback`, `data` and `destructor`.
#[repr(C)]
pub(super) struct TimerNode {
    id: c_long,
    _members: [u64; 7],
    data: *mut c_void,
    destructor: Option<unsafe extern "C" fn(*mut TimerNode)>,
}

/// `swoole::TimerCallback`.
type Callback = unsafe extern "C" fn(*mut c_void, *mut TimerNode);

unsafe extern "C" {
    #[link_name = "_Z16swoole_timer_addlbPFvPN6swoole5TimerEPNS_9TimerNodeEEPv"]
    fn swoole_timer_add(
        ms: c_long,
        persistent: bool,
        callback: Callback,
        data: *mut c_void,
    ) -> *mut TimerNode;
    #[link_name = "_Z18swoole_timer_afterlPFvPN6swoole5TimerEPNS_9TimerNodeEEPv"]
    fn swoole_timer_after(ms: c_long, callback: Callback, data: *mut c_void) -> c_long;
    #[link_name = "_Z17swoole_timer_ticklPFvPN6swoole5TimerEPNS_9TimerNodeEEPv"]
    fn swoole_timer_tick(ms: c_long, callback: Callback, data: *mut c_void) -> c_long;
    fn swr_case_timer_exchange(timer: *mut c_void) -> *mut c_void;
    #[link_name = "_Z16swoole_timer_delPN6swoole9TimerNodeE"]
    pub(super) fn swoole_timer_del(tnode: *mut TimerNode) -> bool;
    #[link_name = "_Z18swoole_timer_delayPN6swoole9TimerNodeEl"]
    fn swoole_timer_delay(tnode: *mut TimerNode, delay_ms: c_long);
    #[link_name = "_Z19swoole_timer_existsl"]
    fn swoole_timer_exists(timer_id: c_long) -> bool;
    #[link_name = "_Z18swoole_timer_clearl"]
    fn swoole_timer_clear(timer_id: c_long) -> bool;
    #[link_name = "_Z16swoole_timer_getl"]
    fn swoole_timer_get(timer_id: c_long) -> *mut TimerNode;
    #[link_name = "_Z17swoole_timer_freev"]
    pub(super) fn swoole_timer_free();
    #[link_name = "_Z19swoole_timer_selectv"]
    fn swoole_timer_select();
    #[link_name = "_Z22swoole_signal_dispatchv"]
    fn swoole_signal_dispatch();
    #[link_name = "_Z17swoole_event_initi"]
    fn swoole_event_init(flags: c_int) -> c_int;
    #[link_name = "_Z17swoole_event_waitv"]
    fn swoole_event_wait() -> c_int;
    #[link_name = "_ZN6swoole12realtime_addEP8timespecl"]
    fn realtime_add(time: *mut [i64; 2], add_msec: i64);
    fn swr_v1_core_signal_alarm() -> bool;
}

unsafe extern "C-unwind" {
    fn swr_v1_core_set_last_error(code: c_int);
}

/// A node's closures, its data: the C++ lambdas' captures.
struct Hooks<'a> {
    call: Box<dyn FnMut(*mut TimerNode) + 'a>,
    destructor: Option<Box<dyn FnMut() + 'a>>,
}

unsafe extern "C" fn call(_timer: *mut c_void, tnode: *mut TimerNode) {
    // SAFETY: `hooks` leaked the node's closures into its data for the timer's lifetime.
    let hooks = unsafe { &mut *(*tnode).data.cast::<Hooks<'_>>() };
    (hooks.call)(tnode);
}

unsafe extern "C" fn destruct(tnode: *mut TimerNode) {
    // SAFETY: as in `call`.
    let hooks = unsafe { &mut *(*tnode).data.cast::<Hooks<'_>>() };
    if let Some(destructor) = hooks.destructor.as_mut() {
        destructor();
    }
}

/// A closure as a node's data. Leaked, as the C++ lambdas' captures outlive
/// nothing they borrow while the timer can still fire.
fn hooks<'a>(f: impl FnMut(*mut TimerNode) + 'a) -> *mut c_void {
    let hooks = Hooks {
        call: Box::new(f),
        destructor: None,
    };
    Box::into_raw(Box::new(hooks)).cast()
}

fn add<'a>(ms: c_long, persistent: bool, f: impl FnMut(*mut TimerNode) + 'a) -> *mut TimerNode {
    // SAFETY: `call` takes the closures `hooks` leaked.
    unsafe { swoole_timer_add(ms, persistent, call, hooks(f)) }
}

pub(super) fn after<'a>(ms: c_long, f: impl FnMut(*mut TimerNode) + 'a) -> c_long {
    // SAFETY: as in `add`.
    unsafe { swoole_timer_after(ms, call, hooks(f)) }
}

pub(super) fn tick<'a>(ms: c_long, f: impl FnMut(*mut TimerNode) + 'a) -> c_long {
    // SAFETY: as in `add`.
    unsafe { swoole_timer_tick(ms, call, hooks(f)) }
}

/// `tnode->destructor = f`.
///
/// # Safety
/// `tnode` is a live node `add` made.
unsafe fn set_destructor<'a>(tnode: *mut TimerNode, f: impl FnMut() + 'a) {
    // SAFETY: as the caller promises; its data is the closures `hooks` leaked.
    unsafe {
        (*(*tnode).data.cast::<Hooks<'a>>()).destructor = Some(Box::new(f));
        (*tnode).destructor = Some(destruct);
    }
}

/// `swoole::time<std::chrono::milliseconds>()`: the system clock.
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
    let counter = [const { Cell::new(0) }; 4];
    let ms1 = now_ms();

    assert!(!add(20, false, |_| _ = incr(&counter[0])).is_null());

    // SAFETY: this thread's last error.
    unsafe { swr_v1_core_set_last_error(0) };
    assert!(add(-1, false, |_| {}).is_null());
    assert_eq!(last_error(), SW_ERROR_INVALID_PARAMS);

    // SAFETY: as above.
    unsafe { swr_v1_core_set_last_error(0) };
    assert!(add(0, false, |_| {}).is_null());
    assert_eq!(last_error(), SW_ERROR_INVALID_PARAMS);

    // dtor test
    let timer = add(20, false, |_| _ = incr(&counter[2]));
    assert!(!timer.is_null());
    // SAFETY: a live node `add` made.
    unsafe { set_destructor(timer, || counter[3].set(9999)) };

    add(100, true, |tnode| {
        if incr(&counter[1]) == 5 {
            // SAFETY: the running node.
            unsafe { swoole_timer_del(tnode) };
        }
    });

    while swoole_rs_timer::php::stats().is_some_and(|(count, _)| count > 0) {
        // SAFETY: SwooleTG.timer is set; the signal-driven timer's select loop, as upstream's.
        unsafe {
            sleep(10);
            swoole_signal_dispatch();
            if swr_v1_core_signal_alarm() {
                swoole_timer_select();
            }
        }
    }

    let ms2 = now_ms();

    // SAFETY: frees this thread's timer.
    unsafe { swoole_timer_free() };

    assert!(ms2.wrapping_sub(ms1) <= 510);
    assert_eq!(counter[0].get(), 1);
    assert_eq!(counter[1].get(), 5);
    assert_eq!(counter[2].get(), 1);
    assert_eq!(counter[3].get(), 9999);
}

fn async_() {
    let timer1_count = Cell::new(0);
    let timer2_count = Cell::new(0);

    event_init();

    let ms1 = now_ms();
    after(20, |_| _ = incr(&timer1_count));

    tick(100, |tnode| {
        if incr(&timer2_count) == 5 {
            // SAFETY: the running node.
            unsafe { swoole_timer_del(tnode) };
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

    // SAFETY: a timer id.
    assert!(unsafe { swoole_timer_exists(timer_id) });
}

fn clear() {
    let timer_id = tick(100, |_| {});

    // SAFETY: a timer id.
    unsafe { swoole_timer_clear(timer_id) };
    // SAFETY: as above.
    assert!(!unsafe { swoole_timer_exists(timer_id) });
}

fn get() {
    let timer_id = tick(100, |_| {});

    // SAFETY: a live timer's node.
    let timer_node = unsafe { swoole_timer_get(timer_id) };
    assert_eq!(unsafe { (*timer_node).id }, timer_id);
    // SAFETY: frees this thread's timer.
    unsafe { swoole_timer_free() };
}

fn delay() {
    event_init();
    let ms1 = now_ms();
    let ms2 = Cell::new(0);
    let timer_id = after(100, |_| ms2.set(now_ms()));

    // SAFETY: a live timer's node, delayed, then the loop runs it; deleting it after
    // it ran is upstream's sequence.
    unsafe {
        let timer_node = swoole_timer_get(timer_id);
        swoole_timer_delay(timer_node, 100);
        event_wait();
        assert!(ms2.get().wrapping_sub(ms1) >= 100);
        swoole_timer_del(timer_node);
        swoole_timer_free();
    }
}

fn error() {
    // SAFETY: SwooleTG.timer is unset for the calls below and restored after.
    unsafe {
        let tmp = swr_case_timer_exchange(std::ptr::null_mut());

        swoole_timer_free();
        swoole_timer_select(); // no effect
        assert!(swoole_timer_get(1).is_null());
        assert!(!swoole_timer_clear(1));
        assert!(!swoole_timer_exists(1));

        let timer_id = tick(0, |_| {});
        assert_eq!(timer_id, c_long::from(SW_ERR));

        let timer_id = after(0, |_| {});
        assert_eq!(timer_id, c_long::from(SW_ERR));

        swoole_timer_delay(std::ptr::null_mut(), 100);
        assert!(!swoole_timer_del(std::ptr::null_mut()));
        swr_case_timer_exchange(tmp);

        swoole_timer_free();
    }
}

fn reinit() {
    let timer1_count = Cell::new(0);
    let timer2_count = Cell::new(0);

    after(20, |_| _ = incr(&timer1_count));

    event_init();

    // SAFETY: `after` created this thread's timer, which the exchange puts back.
    let timer = unsafe { swr_case_timer_exchange(std::ptr::null_mut()) };
    // SAFETY: as above.
    unsafe { swr_case_timer_exchange(timer) };
    // SAFETY: this thread's live timer.
    swoole_rs_timer::owner_reinit(unsafe { &*timer.cast() }, false);

    let ms1 = now_ms();

    tick(100, |tnode| {
        if incr(&timer2_count) == 5 {
            // SAFETY: the running node.
            unsafe { swoole_timer_del(tnode) };
        }
    });

    event_wait();
    let ms2 = now_ms();
    assert!(ms2.wrapping_sub(ms1) <= 510);
    assert_eq!(timer1_count.get(), 1);
    assert_eq!(timer2_count.get(), 5);
}

fn realtime_add_() {
    let mut ts = [1, 900 * SW_NUM_MILLION];

    // SAFETY: a writable timespec (tv_sec, tv_nsec).
    unsafe { realtime_add(&raw mut ts, 1905) };
    assert_eq!(ts[0], 3);
    assert_eq!(ts[1], 805 * SW_NUM_MILLION);
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
