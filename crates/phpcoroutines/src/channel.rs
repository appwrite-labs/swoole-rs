//! `swoole::coroutine::Channel` of owned values on PHP's coroutines: the
//! coroutine crate's queue, whose waits are `Coroutine::yield_ex` (a timeout
//! timer, cancelable). The queue carries slot numbers; the values stay here.
use crate::host::{self, Co};
use std::{
    cell::{Cell, RefCell, UnsafeCell},
    ffi::{c_int, c_void},
    mem::ManuallyDrop,
    ptr,
    rc::Rc,
};
pub use swoole_rs_coroutine::chan::Stats;
use swoole_rs_coroutine::chan::{self, CANCELED, Effects, TIMEDOUT};
use swoole_rs_zend::bailout;

/// `SW_ERROR_CO_OUT_OF_COROUTINE`.
const OUT_OF_COROUTINE: c_int = 10001;

/// One queue and the values it carries, by slot.
struct Inner<T> {
    queue: UnsafeCell<chan::Channel>,
    values: RefCell<Slots<T>>,
}

impl<T> Inner<T> {
    /// The value in the slot the queue gave back (null for none).
    fn value(&self, data: *mut c_void) -> Option<T> {
        self.values.borrow_mut().take(data.addr())
    }
}

/// Values by slot number (1-based: the queue's null is no value).
struct Slots<T> {
    slots: Vec<Option<T>>,
    free: Vec<usize>,
}

impl<T> Slots<T> {
    fn put(&mut self, value: T) -> usize {
        if let Some(slot) = self.free.pop()
            && let Some(free) = self.slots.get_mut(slot - 1)
        {
            *free = Some(value);
            return slot;
        }
        self.slots.push(Some(value));
        self.slots.len()
    }

    fn take(&mut self, slot: usize) -> Option<T> {
        let value = self.slots.get_mut(slot.checked_sub(1)?)?.take()?;
        self.free.push(slot);
        Some(value)
    }
}

/// A coroutine channel slot (upstream's `Channel *` of a channel object): none
/// until [`Channel::open`]. Each operation keeps its queue alive until it
/// returns, so a queue replaced or released meanwhile outlives its waiters.
pub struct Channel<T>(Cell<Option<Rc<Inner<T>>>>);

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

    fn get(&self) -> Option<Rc<Inner<T>>> {
        let inner = self.0.take();
        let copy = inner.clone();
        self.0.set(inner);
        copy
    }

    /// A new queue of `capacity` values in place of any other, whose waiters
    /// keep it.
    pub fn open(&self, capacity: usize) {
        self.0.set(Some(Rc::new(Inner {
            queue: UnsafeCell::new(chan::Channel::new(capacity)),
            values: RefCell::new(Slots {
                slots: Vec::new(),
                free: Vec::new(),
            }),
        })));
    }

    #[must_use]
    pub fn is_open(&self) -> bool {
        self.get().is_some()
    }

    /// `Channel::push`: `value` back when the queue is closed, the wait timed
    /// out or was canceled ([`Stats::error`] says which), there is no queue,
    /// or this is no coroutine (Swoole's fatal error, deferred).
    /// # Errors
    /// `value`, not queued.
    pub fn push(&self, value: T, timeout: f64) -> Result<(), T> {
        let Some(inner) = self.get() else {
            return Err(value);
        };
        let Some(mut php) = Php::current(timeout) else {
            return Err(value);
        };
        let slot = inner.values.borrow_mut().put(value);
        // No drop is pending across the effects: a coroutine that never
        // resumes keeps the queue, as upstream's frames keep their channel.
        let inner = ManuallyDrop::new(inner);
        let data = ptr::without_provenance_mut(slot);
        let pushed = chan::push(&inner.queue, &mut php, data);
        let inner = ManuallyDrop::into_inner(inner);
        if pushed {
            return Ok(());
        }
        // The queue refused the slot, which still holds the value.
        inner.value(data).map_or(Ok(()), Err)
    }

    /// `Channel::pop`: None when the queue is closed and empty, the wait timed
    /// out or was canceled ([`Stats::error`]), there is no queue, or this is
    /// no coroutine (Swoole's fatal error, deferred).
    pub fn pop(&self, timeout: f64) -> Option<T> {
        let inner = self.get()?;
        let mut php = Php::current(timeout)?;
        let inner = ManuallyDrop::new(inner);
        let data = chan::pop(&inner.queue, &mut php);
        ManuallyDrop::into_inner(inner).value(data)
    }

    /// `Channel::close`: wakes every waiter; false if closed already or none.
    pub fn close(&self) -> bool {
        let Some(inner) = self.get() else {
            return false;
        };
        let mut php = Php {
            co: ptr::null_mut(),
            timeout: 0.0,
        };
        let inner = ManuallyDrop::new(inner);
        let closed = chan::close(&inner.queue, &mut php);
        drop(ManuallyDrop::into_inner(inner));
        closed
    }

    /// The queue's [`Stats`]: its last error, items and waiters.
    #[must_use]
    pub fn stats(&self) -> Option<Stats> {
        self.get().map(|inner| chan::stats(&inner.queue))
    }

    /// The oldest queued value, without waiting.
    pub fn take(&self) -> Option<T> {
        let inner = self.get()?;
        inner.value(chan::take(&inner.queue))
    }

    /// Give the queue up, with its last [`Stats`].
    pub fn release(&self) -> Option<Stats> {
        self.0.take().map(|inner| chan::stats(&inner.queue))
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
    fn current(timeout: f64) -> Option<Self> {
        let co = host::current();
        if co.is_null() {
            bailout::defer(swoole_rs_phpswoole::swoole_fatal_error(
                OUT_OF_COROUTINE,
                c"API must be called in the coroutine",
            ));
            return None;
        }
        Some(Self { co, timeout })
    }
}

impl Effects for Php {
    fn current(&self) -> *mut c_void {
        self.co
    }

    /// Upstream's timer and cancel function remove the waiter, then resume
    /// it; the queue removes it here before anything else runs. Unlike
    /// `yield_ex`, a channel wait leaves Swoole's last error alone.
    fn wait(&mut self, _: &UnsafeCell<chan::Channel>, _: u32) -> u32 {
        let error = swoole_rs_corebase::log::last_error();
        if host::yield_ex(self.co, self.timeout) {
            return 0;
        }
        swoole_rs_corebase::log::set_last_error(error);
        if host::is_canceled(self.co) {
            CANCELED
        } else {
            TIMEDOUT
        }
    }

    fn resume(&mut self, waiter: *mut c_void) {
        host::resume(waiter);
    }
}
