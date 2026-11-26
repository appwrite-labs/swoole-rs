//! `swoole::coroutine::Channel` of owned values on PHP's coroutines: the
//! coroutine crate's queue, whose waits are `Coroutine::yield_ex`'s (a timeout
//! timer, cancelable) without its last error.
use crate::host::{self, Co};
use std::{
    cell::{Cell, UnsafeCell},
    ffi::c_int,
    mem,
    rc::Rc,
};
pub use swoole_rs_corebase::coroutine::chan::Stats;
use swoole_rs_corebase::coroutine::{
    Resume,
    chan::{self, CANCELED, Effects, TIMEDOUT},
};
use swoole_rs_corebase::timer::Deadline;
use swoole_rs_zend::bailout;

/// `SW_ERROR_CO_OUT_OF_COROUTINE`.
const OUT_OF_COROUTINE: c_int = swoole_rs_error::ErrorCode::SW_ERROR_CO_OUT_OF_COROUTINE as c_int;

/// A coroutine channel slot (upstream's `Channel *` of a channel object): none
/// until [`Channel::open`].
pub struct Channel<T>(Cell<Option<Queue<T>>>);

impl<T> Default for Channel<T> {
    fn default() -> Self {
        Self::new()
    }
}

impl<T> Channel<T> {
    #[must_use]
    pub const fn new() -> Self {
        Self(Cell::new(None))
    }

    /// A new queue of `capacity` values in place of any other, whose waiters
    /// keep it.
    pub fn open(&self, capacity: usize) {
        let queue = chan::Channel::new(capacity);
        self.0.set(Some(Queue(Rc::new(UnsafeCell::new(queue)))));
    }

    /// The queue. An operation keeps it alive until it returns, so a queue
    /// replaced or released meanwhile outlives its waiters.
    #[must_use]
    pub fn queue(&self) -> Option<Queue<T>> {
        let queue = self.0.take();
        let copy = queue.clone();
        #[expect(
            clippy::mem_forget,
            reason = "Rc cloning cannot re-enter, so the hot path can omit CellExt::with_taken's displaced-value drop"
        )]
        mem::forget(self.0.replace(queue));
        copy
    }

    /// Give the queue up.
    pub fn release(&self) -> Option<Queue<T>> {
        self.0.take()
    }
}

/// A channel's queue. Each operation keeps it alive while its coroutine waits.
pub struct Queue<T>(Rc<UnsafeCell<chan::Channel<T, Co>>>);

impl<T> Clone for Queue<T> {
    fn clone(&self) -> Self {
        Self(Rc::clone(&self.0))
    }
}

impl<T> Queue<T> {
    /// `Channel::push`, waiting until `deadline` for room.
    /// # Errors
    /// `value`, not queued, with the queue's error: it is closed, or the wait
    /// timed out or was canceled. Outside a coroutine, also Swoole's fatal
    /// error (deferred).
    pub fn push(self, value: T, deadline: Deadline) -> Result<(), (T, i32)> {
        let Some(mut php) = Php::current() else {
            let error = self.stats().error;
            return Err((value, error));
        };
        // A coroutine that never resumes retains this frame and its queue.
        chan::push(&self.0, &mut php, value, deadline).map_err(|value| (value, self.stats().error))
    }

    /// `Channel::pop`: the oldest value, waiting until `deadline` for one.
    /// # Errors
    /// The queue's error: it is closed and empty, or the wait timed out or was
    /// canceled. Outside a coroutine, also Swoole's fatal error (deferred).
    pub fn pop(self, deadline: Deadline) -> Result<T, i32> {
        let Some(mut php) = Php::current() else {
            return Err(self.stats().error);
        };
        chan::pop(&self.0, &mut php, deadline).ok_or_else(|| self.stats().error)
    }

    /// `Channel::close`: wakes every waiter; false if closed already.
    ///
    /// # Errors
    /// A resumed waiter bailed out. The trap protects the caller and this
    /// queue's owner; the close loop holds only copyable waiter handles.
    pub fn close(self) -> Result<bool, bailout::Bailout> {
        let mut php = Closing;
        let mut closed = false;
        swoole_rs_zend::contain_with(|| closed = chan::close(&self.0, &mut php))?;
        Ok(closed)
    }

    /// The queue's [`Stats`]: its last error, items and waiters.
    #[must_use]
    pub fn stats(&self) -> Stats {
        chan::stats(&self.0)
    }

    /// The oldest queued value, without waiting.
    #[must_use]
    pub fn take(&self) -> Option<T> {
        chan::take(&self.0)
    }
}

/// PHP's coroutines for one push or pop, from the running coroutine.
struct Php {
    co: Co,
}

/// PHP's coroutines for a close, which only resumes waiters.
struct Closing;

impl Php {
    /// The running coroutine, or Swoole's fatal error (deferred) outside one,
    /// where upstream's `Coroutine::get_current_safe()` raises it.
    #[inline]
    fn current() -> Option<Self> {
        let Some(co) = host::current() else {
            out_of_coroutine();
            return None;
        };
        Some(Self { co })
    }
}

#[cold]
fn out_of_coroutine() {
    bailout::defer(swoole_rs_phpswoole::swoole_fatal_error(
        OUT_OF_COROUTINE,
        c"API must be called in the coroutine",
    ));
}

impl Effects for Php {
    type Waiter = Co;

    fn current(&self) -> Co {
        self.co
    }

    /// Upstream's timer and cancel function remove the waiter, then resume
    /// it; the queue removes it here before anything else runs.
    fn wait(&mut self, deadline: Deadline) -> u32 {
        match host::wait(self.co, deadline) {
            Resume::Canceled => CANCELED,
            Resume::TimedOut => TIMEDOUT,
            Resume::Ok => 0,
        }
    }

    fn resume(&mut self, waiter: Co) {
        host::resume(waiter);
    }
}

impl Effects for Closing {
    type Waiter = Co;

    fn current(&self) -> Co {
        swoole_rs_corebase::fatal("a channel close never waits")
    }

    fn wait(&mut self, _: Deadline) -> u32 {
        swoole_rs_corebase::fatal("a channel close never waits")
    }

    fn resume(&mut self, waiter: Co) {
        host::resume(waiter);
    }
}
