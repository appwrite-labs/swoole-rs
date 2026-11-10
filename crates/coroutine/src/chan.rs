// SPDX-License-Identifier: Apache-2.0
//! `swoole::coroutine::Channel`: capacity, closed flag, last error, the data
//! FIFO and both waiter queues, and pop/push/close as whole operations.
//!
//! The host runs the coroutines through [`Effects`]: suspending the current one
//! (with its timeout timer and cancel function) and resuming a waiter. No Rust
//! reference into the channel lives across an effect: other coroutines use the
//! channel while this one is suspended, and a resumed waiter may delete it, so
//! an operation never touches it after a resume that ends it. The operations
//! take the channel's cell, which is not `Sync`.
use core::ffi::c_void;
use std::cell::UnsafeCell;
use std::collections::VecDeque;

pub const PRODUCER: u32 = 1;
pub const CONSUMER: u32 = 2;
pub const ERROR_OK: i32 = 0;
pub const ERROR_TIMEOUT: i32 = -1;
pub const ERROR_CLOSED: i32 = -2;
pub const ERROR_CANCELED: i32 = -3;
/// [`Effects::wait`]'s result bits.
pub const CANCELED: u32 = 1;
pub const TIMEDOUT: u32 = 2;

#[derive(Default)]
pub struct Channel {
    capacity: usize,
    closed: bool,
    error: i32,
    data: VecDeque<*mut c_void>,
    producers: VecDeque<*mut c_void>,
    consumers: VecDeque<*mut c_void>,
}

/// The host's coroutines, for one operation.
pub trait Effects {
    /// The current coroutine, queued as a waiter.
    fn current(&self) -> *mut c_void;
    /// Suspends the current coroutine, already queued under `role` in `ch`,
    /// until it is resumed, canceled or its timeout fires. The timer is
    /// deleted before this returns `CANCELED` and/or `TIMEDOUT` bits; the
    /// operation then removes the waiter ([`Channel::remove`]) if a cancel or
    /// timeout has not.
    fn wait(&mut self, ch: &UnsafeCell<Channel>, role: u32) -> u32;
    /// Resumes a waiter just taken from a queue.
    fn resume(&mut self, co: *mut c_void);
}

impl Channel {
    #[must_use]
    pub fn new(capacity: usize) -> Self {
        Self {
            capacity,
            ..Self::default()
        }
    }

    fn queue(&mut self, role: u32) -> &mut VecDeque<*mut c_void> {
        // Upstream's enqueue/dequeue test for PRODUCER; removal tests for CONSUMER.
        if role == PRODUCER {
            &mut self.producers
        } else {
            &mut self.consumers
        }
    }

    /// Drops every queued `co` waiter of `role`: a timeout or cancel before resume.
    pub fn remove(&mut self, role: u32, co: *mut c_void) {
        let queue = if role == CONSUMER {
            &mut self.consumers
        } else {
            &mut self.producers
        };
        queue.retain(|&entry| entry != co);
    }

    /// Removes and returns the oldest item, or null when empty.
    pub fn pop_data(&mut self) -> *mut c_void {
        self.data.pop_front().unwrap_or(core::ptr::null_mut())
    }

    #[must_use]
    pub fn length(&self) -> usize {
        self.data.len()
    }

    #[must_use]
    pub fn producer_num(&self) -> usize {
        self.producers.len()
    }

    #[must_use]
    pub fn consumer_num(&self) -> usize {
        self.consumers.len()
    }

    #[must_use]
    pub fn error(&self) -> i32 {
        self.error
    }

    #[must_use]
    pub fn is_full(&self) -> bool {
        self.data.len() == self.capacity
    }

    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.data.is_empty()
    }
}

/// `f` of the channel, which no other borrow reaches meanwhile: the cell is not
/// `Sync`, and `f` (only the bodies below) runs no effect.
fn with<R>(ch: &UnsafeCell<Channel>, f: impl FnOnce(&mut Channel) -> R) -> R {
    // SAFETY: as above.
    f(unsafe { &mut *ch.get() })
}

/// What a channel reports: its last error, items and waiters.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Stats {
    pub error: i32,
    pub length: usize,
    pub producers: usize,
    pub consumers: usize,
    pub full: bool,
}

/// The channel's [`Stats`].
pub fn stats(ch: &UnsafeCell<Channel>) -> Stats {
    with(ch, |c| Stats {
        error: c.error,
        length: c.length(),
        producers: c.producer_num(),
        consumers: c.consumer_num(),
        full: c.is_full(),
    })
}

/// [`Channel::pop_data`]: the oldest item without waiting, or null.
pub fn take(ch: &UnsafeCell<Channel>) -> *mut c_void {
    with(ch, Channel::pop_data)
}

/// Queues the current coroutine as a `role` waiter and suspends it. Returns the
/// error to report, or `ERROR_OK` to proceed.
fn wait(ch: &UnsafeCell<Channel>, fx: &mut impl Effects, role: u32) -> i32 {
    let co = fx.current();
    with(ch, |c| c.queue(role).push_back(co));
    let flags = fx.wait(ch, role);
    if flags != 0 {
        with(ch, |c| c.remove(role, co));
    }
    if flags & CANCELED != 0 {
        ERROR_CANCELED
    } else if flags & TIMEDOUT != 0 {
        ERROR_TIMEOUT
    } else {
        ERROR_OK
    }
}

/// Sets the error unless it is `ERROR_OK`; whether it was.
fn ok(ch: &UnsafeCell<Channel>, status: i32) -> bool {
    if status != ERROR_OK {
        with(ch, |c| c.error = status);
    }
    status == ERROR_OK
}

/// `Channel::pop`: the oldest item, or null with the error set. After the
/// final resume the channel is not touched.
pub fn pop(ch: &UnsafeCell<Channel>, fx: &mut impl Effects) -> *mut c_void {
    let (closed, wait_first) = with(ch, |c| {
        (
            c.closed && c.data.is_empty(),
            c.data.is_empty() || !c.consumers.is_empty(),
        )
    });
    if closed {
        ok(ch, ERROR_CLOSED);
        return core::ptr::null_mut();
    }
    if wait_first {
        let status = wait(ch, fx, CONSUMER);
        let closed = with(ch, |c| c.closed && c.data.is_empty());
        if !ok(
            ch,
            if status == ERROR_OK && closed {
                ERROR_CLOSED
            } else {
                status
            },
        ) {
            return core::ptr::null_mut();
        }
    }
    let (data, producer) = with(ch, |c| (c.pop_data(), c.producers.pop_front()));
    if let Some(co) = producer {
        fx.resume(co);
    }
    data
}

/// `Channel::push`: false with the error set when it fails. After the final
/// resume the channel is not touched.
pub fn push(ch: &UnsafeCell<Channel>, fx: &mut impl Effects, data: *mut c_void) -> bool {
    let (closed, wait_first) = with(ch, |c| (c.closed, c.is_full() || !c.producers.is_empty()));
    if closed {
        return ok(ch, ERROR_CLOSED);
    }
    if wait_first {
        let status = wait(ch, fx, PRODUCER);
        let closed = with(ch, |c| c.closed);
        if !ok(
            ch,
            if status == ERROR_OK && closed {
                ERROR_CLOSED
            } else {
                status
            },
        ) {
            return false;
        }
    }
    let consumer = with(ch, |c| {
        c.data.push_back(data);
        c.consumers.pop_front()
    });
    if let Some(co) = consumer {
        fx.resume(co);
    }
    true
}

/// `Channel::close`: wakes producers, then consumers.
pub fn close(ch: &UnsafeCell<Channel>, fx: &mut impl Effects) -> bool {
    if with(ch, |c| std::mem::replace(&mut c.closed, true)) {
        return false;
    }
    for role in [PRODUCER, CONSUMER] {
        // Re-read the queue after each resume: the woken waiter may change it.
        while let Some(co) = with(ch, |c| c.queue(role).pop_front()) {
            fx.resume(co);
        }
    }
    true
}

#[cfg(test)]
#[path = "../../../tests/modules/coroutine/channel.rs"]
mod tests;
