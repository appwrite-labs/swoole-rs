//! `swoole::coroutine::Channel` of owned values on PHP's coroutines: the
//! coroutine crate's queue, whose waits are `Coroutine::yield_ex`'s (a timeout
//! timer, cancelable) without its last error.
use crate::host::{self, Co};
use std::{
    cell::{Cell, UnsafeCell},
    ffi::{c_int, c_void},
    mem::{self, ManuallyDrop},
    ptr,
    rc::Rc,
};
pub use swoole_rs_coroutine::chan::Stats;
use swoole_rs_coroutine::{
    RC_CANCELED, RC_TIMEDOUT,
    chan::{self, CANCELED, Effects, TIMEDOUT},
};
use swoole_rs_zend::bailout;

/// `SW_ERROR_CO_OUT_OF_COROUTINE`.
const OUT_OF_COROUTINE: c_int = 10001;

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
        // Still empty: cloning an `Rc` runs no code, so nothing filled the slot.
        mem::forget(self.0.replace(queue));
        copy
    }

    /// Give the queue up.
    pub fn release(&self) -> Option<Queue<T>> {
        self.0.take()
    }
}

/// A channel's queue. The operations that may wait take the handle, so that
/// no drop is pending in the caller's frame across a wait.
pub struct Queue<T>(Rc<UnsafeCell<chan::Channel<T>>>);

impl<T> Clone for Queue<T> {
    fn clone(&self) -> Self {
        Self(Rc::clone(&self.0))
    }
}

impl<T> Queue<T> {
    /// `Channel::push`.
    /// # Errors
    /// `value`, not queued, with the queue's error: it is closed, or the wait
    /// timed out or was canceled. Outside a coroutine, also Swoole's fatal
    /// error (deferred).
    pub fn push(self, value: T, timeout: f64) -> Result<(), (T, i32)> {
        let Some(mut php) = Php::current(timeout) else {
            let error = self.stats().error;
            return Err((value, error));
        };
        // No drop is pending across the effects: a coroutine that never
        // resumes keeps the queue, as upstream's frames keep their channel.
        let queue = ManuallyDrop::new(self);
        let pushed = chan::push(&queue.0, &mut php, value);
        let queue = ManuallyDrop::into_inner(queue);
        pushed.map_err(|value| (value, queue.stats().error))
    }

    /// `Channel::pop`: the oldest value.
    /// # Errors
    /// The queue's error: it is closed and empty, or the wait timed out or was
    /// canceled. Outside a coroutine, also Swoole's fatal error (deferred).
    pub fn pop(self, timeout: f64) -> Result<T, i32> {
        let Some(mut php) = Php::current(timeout) else {
            return Err(self.stats().error);
        };
        let queue = ManuallyDrop::new(self);
        let value = chan::pop(&queue.0, &mut php);
        let queue = ManuallyDrop::into_inner(queue);
        value.ok_or_else(|| queue.stats().error)
    }

    /// `Channel::close`: wakes every waiter; false if closed already.
    #[must_use]
    pub fn close(self) -> bool {
        let mut php = Php {
            co: ptr::null_mut(),
            timeout: 0.0,
        };
        let queue = ManuallyDrop::new(self);
        let closed = chan::close(&queue.0, &mut php);
        drop(ManuallyDrop::into_inner(queue));
        closed
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

/// PHP's coroutines for one operation; `co` is null for close.
struct Php {
    co: Co,
    timeout: f64,
}

impl Php {
    /// The running coroutine, or Swoole's fatal error (deferred) outside one,
    /// where upstream's `Coroutine::get_current_safe()` raises it.
    #[inline]
    fn current(timeout: f64) -> Option<Self> {
        let co = host::current();
        if co.is_null() {
            out_of_coroutine();
            return None;
        }
        Some(Self { co, timeout })
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
    fn current(&self) -> *mut c_void {
        self.co
    }

    /// Upstream's timer and cancel function remove the waiter, then resume
    /// it; the queue removes it here before anything else runs.
    fn wait(&mut self) -> u32 {
        match host::wait(self.co, self.timeout) {
            RC_CANCELED => CANCELED,
            RC_TIMEDOUT => TIMEDOUT,
            _ => 0,
        }
    }

    fn resume(&mut self, waiter: *mut c_void) {
        host::resume(waiter);
    }
}
