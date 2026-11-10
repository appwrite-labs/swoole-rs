//! What `tests/modules/timer/kernel.cc` checked over the queue's C ABI, over the `Timer`
//! methods that ABI wrapped: one test per topic, same inputs and expected values. The
//! native side's custody of each token is the `Queue` harness: a token is handed back
//! exactly once, its destructor runs only after the queue call that released it has
//! returned, and callbacks run between `select_next` and `complete` with no borrow of
//! the queue held.
use super::*;
use std::cell::{Cell, RefCell};
use std::collections::BTreeMap;

/// A node's native destructor, which may call the queue again.
type Destroy<'a> = RefCell<Option<Box<dyn Fn(&Queue) + 'a>>>;

/// `struct Node`: a native timer node the queue holds only as a token.
struct Node<'a> {
    name: &'static str,
    drops: Cell<i32>,
    destroy: Destroy<'a>,
}

impl<'a> Node<'a> {
    fn new(name: &'static str) -> Self {
        Self {
            name,
            drops: Cell::new(0),
            destroy: RefCell::new(None),
        }
    }
    fn on_destroy(&self, destroy: impl Fn(&Queue) + 'a) {
        *self.destroy.borrow_mut() = Some(Box::new(destroy));
    }
    fn token(&self) -> *mut c_void {
        ptr::from_ref(self).cast_mut().cast()
    }
}

fn node<'n>(token: *mut c_void) -> &'n Node<'n> {
    // SAFETY: every token in these tests is a live `Node` the test owns.
    unsafe { &*token.cast::<Node<'n>>() }
}

/// `struct Queue`: the queue, the trace and the tokens it holds.
struct Queue {
    q: RefCell<Timer>,
    trace: RefCell<Vec<String>>,
    owned: RefCell<BTreeMap<i64, *mut c_void>>,
}

impl Queue {
    fn new() -> Self {
        Self {
            q: RefCell::new(Timer::default()),
            trace: RefCell::new(Vec::new()),
            owned: RefCell::new(BTreeMap::new()),
        }
    }
    fn add(&self, n: &Node<'_>, now: i64, delay: i64, repeat: bool) -> TimerView {
        let (status, v) = self.q.borrow_mut().add(now, delay, repeat, n.token());
        assert_eq!(status, OK);
        assert!(v.id > 0 && v.token == n.token() && v.deadline == now + delay);
        assert!(v.removed == 0 && v.reserved == 0);
        assert!(v.interval == if repeat { delay } else { 0 } && v.exec_count == 0);
        assert!(self.owned.borrow_mut().insert(v.id, v.token).is_none());
        v
    }
    fn stats(&self) -> TimerStats {
        let s = self.q.borrow().stats();
        assert_eq!(s.count, self.owned.borrow().len());
        s
    }
    fn drop_view(&self, v: TimerView) {
        let token = self.owned.borrow_mut().remove(&v.id);
        assert_eq!(token, Some(v.token));
        let n = node(v.token);
        n.drops.set(n.drops.get() + 1);
        assert_eq!(n.drops.get(), 1);
        self.trace.borrow_mut().push(format!("drop:{}", n.name));
        assert_eq!(self.q.borrow().get(v.id).0, NOT_FOUND);
        let active = self.stats().active_id;
        assert!(active == 0 || active != v.id);
        // Every queue call has returned before the native destructor runs.
        if let Some(destroy) = n.destroy.borrow().as_ref() {
            destroy(self);
        }
    }
    fn remove(&self, id: i64) {
        let (status, v) = self.q.borrow_mut().remove(id);
        assert_eq!(status, RELEASE);
        self.drop_view(v);
    }
    fn prime(&self, now: i64) {
        assert_eq!(self.q.borrow_mut().begin(now), OK);
        assert_eq!(self.q.borrow_mut().select_next().0, EMPTY);
        assert_eq!(self.q.borrow_mut().end().0, OK);
        assert_eq!(self.stats().round, 1);
    }
    fn select(&self, now: i64, callback: impl Fn(TimerView)) {
        assert_eq!(self.q.borrow_mut().begin(now), OK);
        loop {
            let (status, v) = self.q.borrow_mut().select_next();
            if status == EMPTY {
                break;
            }
            if status == CLOSING {
                assert!(self.stats().closing != 0 && v.id == 0 && v.token.is_null());
                break;
            }
            assert!(status == OK && self.stats().active_id == v.id);
            self.trace
                .borrow_mut()
                .push(format!("call:{}", node(v.token).name));
            callback(v); // A native callback; no borrow of the queue is held.
            let (status, v) = self.q.borrow_mut().complete(v.id);
            if status == RELEASE_OVERFLOW {
                self.trace
                    .borrow_mut()
                    .push(format!("overflow:{}", node(v.token).name));
            }
            if status == RELEASE || status == RELEASE_OVERFLOW {
                self.drop_view(v);
            } else {
                assert!(status == OK && self.stats().active_id == 0);
            }
        }
        assert_eq!(self.q.borrow_mut().end().0, OK);
    }
    /// `close()`: the export answered `OK` twice, as `Timer::close` is idempotent.
    fn close(&self) {
        self.q.borrow_mut().close();
        self.q.borrow_mut().close();
        let mut last = self.q.borrow_mut().drain();
        while last.0 == RELEASE {
            self.drop_view(last.1);
            last = self.q.borrow_mut().drain();
        }
        assert!(last.0 == EMPTY || last.0 == DEFERRED);
        assert!(last.1.id == 0 && last.1.token.is_null());
    }
    fn golden(&self, expected: &[&str]) {
        assert_eq!(*self.trace.borrow(), expected);
    }
}

impl Drop for Queue {
    fn drop(&mut self) {
        if !std::thread::panicking() {
            assert!(self.owned.borrow().is_empty());
            assert!(self.q.borrow().idle(), "swr_v1_timer_release is OK");
        }
    }
}

/// `main`'s checks before the topics, less the null handles and outputs of the C ABI.
#[test]
fn invalid_calls() {
    let q = Queue::new();
    let mut timer = q.q.borrow_mut();
    assert_eq!(timer.drain(), (INVALID, TimerView::default()));
    assert_eq!(timer.select_next(), (INVALID, TimerView::default()));
    assert_eq!(timer.complete(1), (INVALID, TimerView::default()));
    assert_eq!(timer.get(0), (INVALID, TimerView::default()));
    assert_eq!(timer.begin(0), OK);
    assert!(!timer.idle(), "release is PENDING");
    assert_eq!(timer.end(), (OK, -1));
}

#[test]
fn active_cancel() {
    let a = Node::new("A");
    let q = Queue::new();
    let n = q.add(&a, 0, 1, true);
    q.prime(0);
    q.select(1, |v| {
        let (status, out) = q.q.borrow_mut().remove(v.id);
        assert!(status == DEFERRED && out.removed != 0);
        assert_eq!(q.q.borrow_mut().remove(v.id).0, NOT_FOUND);
        let (status, out) = q.q.borrow().get(v.id);
        assert!(status == OK && out.removed != 0 && out.token == a.token());
        assert!(q.stats().count == 1 && a.drops.get() == 0 && n.id == v.id);
    });
    q.golden(&["call:A", "drop:A"]);
}

#[test]
fn active_delay() {
    let (a, b) = (Node::new("A"), Node::new("B"));
    let q = Queue::new();
    let av = q.add(&a, 0, 1, false);
    q.add(&b, 0, 2, false);
    q.prime(0);
    q.select(2, |v| {
        if v.id == av.id {
            let (status, out) = q.q.borrow_mut().update(v.id, 20);
            assert!(status == OK && out.deadline == 20);
            assert_eq!(q.q.borrow_mut().complete(v.id + 100).0, INVALID);
        }
    });
    q.golden(&["call:A", "drop:A", "call:B", "drop:B"]);

    let c = Node::new("C");
    let r = Queue::new();
    let cv = r.add(&c, 0, 5, true);
    r.prime(0);
    r.select(5, |v| assert_eq!(r.q.borrow_mut().update(v.id, 20).0, OK));
    let (status, out) = r.q.borrow().get(cv.id);
    assert!(status == OK && out.deadline == 20 && out.exec_count == 1);
    r.close();
}

/// `recursive`: the `nested_selection` topic.
#[test]
fn recursive() {
    let a = Node::new("A");
    let q = Queue::new();
    q.add(&a, 0, 1, false);
    q.prime(0);
    q.select(1, |_| {
        let before = q.stats();
        assert_eq!(q.q.borrow_mut().begin(1), BUSY);
        assert_eq!(q.q.borrow_mut().select_next().0, BUSY);
        assert_eq!(q.q.borrow_mut().end(), (BUSY, -1));
        assert_eq!(q.stats().round, before.round);
    });
    q.golden(&["call:A", "drop:A"]);
}

/// `mutation`: the `callback_destructor_mutation` topic.
#[test]
fn mutation() {
    let (a, b, c) = (Node::new("A"), Node::new("B"), Node::new("C"));
    let q = Queue::new();
    q.add(&a, 0, 1, false);
    let bv = q.add(&b, 0, 2, false);
    let cv = q.add(&c, 0, 3, false);
    b.on_destroy(move |q| q.remove(cv.id));
    q.prime(0);
    q.select(3, |_| q.remove(bv.id));
    q.golden(&["call:A", "drop:B", "drop:C", "drop:A"]);
}

/// `ties`: the `equal_deadlines` topic.
#[test]
fn ties() {
    let (a, b, c) = (Node::new("A"), Node::new("B"), Node::new("C"));
    let q = Queue::new();
    q.add(&a, 0, 1, false);
    q.add(&b, 0, 1, false);
    q.add(&c, 0, 1, false);
    q.prime(0);
    q.select(1, |_| {});
    q.golden(&["call:A", "drop:A", "call:C", "drop:C", "call:B", "drop:B"]);
}

/// `missed`: the `missed_intervals` topic.
#[test]
fn missed() {
    let a = Node::new("A");
    let q = Queue::new();
    let av = q.add(&a, 0, 5, true);
    q.prime(0);
    q.select(23, |_| {});
    let (status, v) = q.q.borrow().get(av.id);
    assert!(status == OK && v.deadline == 25 && v.exec_count == 1);
    q.select(24, |_| {});
    let (status, v) = q.q.borrow().get(av.id);
    assert!(status == OK && v.exec_count == 1);
    q.select(25, |_| {});
    let (status, v) = q.q.borrow().get(av.id);
    assert!(status == OK && v.deadline == 30 && v.exec_count == 2);
    q.close();
    q.golden(&["call:A", "call:A", "drop:A"]);
}

/// `captured`: the `captured_time` topic.
#[test]
fn captured() {
    let (a, b, c) = (Node::new("A"), Node::new("B"), Node::new("C"));
    let q = Queue::new();
    q.add(&a, 0, 1, false);
    q.add(&b, 0, 50, false);
    q.prime(0);
    q.select(1, |_| {
        q.add(&c, 100, 1, false);
    });
    q.golden(&["call:A", "drop:A"]);
    q.select(100, |_| {});
    q.golden(&["call:A", "drop:A", "call:B", "drop:B"]);
    q.close();
}

/// `birth`: the `birth_round_head_blocking` topic.
#[test]
fn birth() {
    let (a, b, c) = (Node::new("A"), Node::new("B"), Node::new("C"));
    let q = Queue::new();
    let av = q.add(&a, 0, 1, false);
    let bv = q.add(&b, 0, 2, false);
    q.prime(2);
    q.select(2, |v| {
        if v.id == av.id {
            let cv = q.add(&c, 2, 1, false);
            assert_eq!(q.q.borrow_mut().update(cv.id, 0).0, OK);
        }
    });
    // Newly born C is heap head and blocks already-due B in this round.
    q.golden(&["call:A", "drop:A"]);
    assert_eq!(q.stats().count, 2);
    assert_eq!(q.q.borrow().get(bv.id).0, OK);
    q.select(3, |_| {});
    q.golden(&["call:A", "drop:A", "call:C", "drop:C", "call:B", "drop:B"]);
}

/// `snapshot`: the `snapshot_mutation` topic, over `swr_v1_timer_ids`, which
/// `swoole_timer.h` still reads.
#[test]
fn snapshot() {
    let c = Node::new("C");
    let (a, b) = (Node::new("A"), Node::new("B"));
    let q = Queue::new();
    let av = q.add(&a, 0, 1, false);
    let bv = q.add(&b, 0, 1, false);
    a.on_destroy(|q| {
        q.add(&c, 0, 1, false);
    });
    let queue = q.q.as_ptr().cast_const();
    let mut count = 0;
    // SAFETY: the live queue, no slots and a count.
    let sized = unsafe { swr_v1_timer_ids(queue, ptr::null_mut(), 0, &raw mut count) };
    assert!(sized == CAPACITY && count == 2);
    let mut guard = [-77_i64, -88];
    // SAFETY: as above, and one writable slot.
    let short = unsafe { swr_v1_timer_ids(queue, guard.as_mut_ptr(), 1, &raw mut count) };
    assert!(short == CAPACITY && guard == [-77, -88]);
    // SAFETY: as above, and two writable slots.
    assert_eq!(
        unsafe { swr_v1_timer_ids(queue, guard.as_mut_ptr(), 2, &raw mut count) },
        OK
    );
    guard.sort_unstable();
    assert!(guard[0] == av.id && guard[1] == bv.id);
    q.remove(guard[0]);
    q.remove(guard[1]);
    assert_eq!(q.stats().count, 1);
    q.close();
    q.golden(&["drop:A", "drop:B", "drop:C"]);
}

/// `limits`: the `checked_limits` topic, less `persistent == 2` and the null token,
/// which only the C ABI could pass.
#[test]
fn limits() {
    let a = Node::new("A");
    let q = Queue::new();
    let none = TimerView::default();
    assert_eq!(
        q.q.borrow_mut().add(i64::MAX - 2, 3, false, a.token()),
        (OVERFLOW, none)
    );
    assert_eq!(q.q.borrow_mut().add(0, 0, false, a.token()).0, INVALID);
    assert_eq!(
        q.q.borrow_mut().add(0, 1, false, ptr::null_mut()).0,
        INVALID
    );
    let av = q.add(&a, 10, 1, false);
    assert_eq!(av.id, 1);
    assert_eq!(q.q.borrow_mut().add(9, 1, false, a.token()).0, INVALID);
    assert_eq!(q.stats().count, 1);
    assert_eq!(q.q.borrow_mut().begin(9), INVALID);
    assert_eq!(q.stats().selecting, 0);
    assert_eq!(q.q.borrow_mut().update(av.id, -1).0, INVALID);
    let (status, v) = q.q.borrow().get(av.id);
    assert!(status == OK && v.deadline == 11);
    assert!(!q.q.borrow().idle(), "release is PENDING");
    q.close();
    q.golden(&["drop:A"]);
}

/// `overflow`: the `repeat_deadline_overflow` topic.
#[test]
fn overflow() {
    let a = Node::new("A");
    let q = Queue::new();
    q.add(&a, i64::MAX - 2, 1, true);
    q.prime(i64::MAX - 2);
    q.select(i64::MAX, |_| {});
    q.golden(&["call:A", "overflow:A", "drop:A"]);
}

/// `teardown`: the `callback_close_drain` topic.
#[test]
fn teardown() {
    let (a, b, c) = (Node::new("A"), Node::new("B"), Node::new("C"));
    let q = Queue::new();
    q.add(&a, 0, 1, false);
    q.add(&b, 0, 2, false);
    q.prime(0);
    q.select(2, |v| {
        q.close();
        assert_eq!(q.q.borrow_mut().drain(), (DEFERRED, TimerView::default()));
        assert_eq!(q.q.borrow_mut().add(2, 1, false, c.token()).0, CLOSING);
        assert_eq!(q.q.borrow_mut().update(v.id, 3).0, CLOSING);
        assert!(q.stats().count == 1 && a.drops.get() == 0 && q.stats().closing != 0);
    });
    q.golden(&["call:A", "drop:B", "drop:A"]);
}

/// `fork_copy`: the `unix_fork_copy` topic. The child disposes of its copy; the parent's
/// queue and token are untouched.
#[test]
#[cfg_attr(miri, ignore = "Miri has no fork")]
fn fork_copy() {
    let a = Node::new("A");
    let q = Queue::new();
    let av = q.add(&a, 0, 1, false);
    // SAFETY: fork(2); the child only touches the copied queue and exits.
    let child = unsafe { libc::fork() };
    assert!(child >= 0);
    if child == 0 {
        let held = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            q.close();
            assert!(a.drops.get() == 1 && q.stats().count == 0);
            assert!(q.q.borrow().idle(), "swr_v1_timer_release is OK");
        }));
        // SAFETY: ends the child without running the parent's test harness.
        unsafe { libc::_exit(i32::from(held.is_err())) };
    }
    let mut status = 0;
    // SAFETY: waitpid(2) on our child.
    assert_eq!(unsafe { libc::waitpid(child, &raw mut status, 0) }, child);
    assert!(libc::WIFEXITED(status) && libc::WEXITSTATUS(status) == 0);
    let (found, v) = q.q.borrow().get(av.id);
    assert!(a.drops.get() == 0 && found == OK && v.token == a.token());
    q.prime(0);
    q.select(1, |_| {});
    q.golden(&["call:A", "drop:A"]);
}
