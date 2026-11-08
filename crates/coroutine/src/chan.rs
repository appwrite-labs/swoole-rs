// SPDX-License-Identifier: Apache-2.0
//! `swoole::coroutine::Channel`: capacity, closed flag, last error, the data
//! FIFO and both waiter queues, and pop/push/close as whole operations.
//!
//! The host runs the coroutines through [`Effects`]: suspending the current one
//! (with its timeout timer and cancel function) and resuming a waiter. No Rust
//! reference to the channel lives across an effect: other coroutines use the
//! channel while this one is suspended, and a resumed waiter may delete it, so
//! an operation never touches it after a resume that ends it.
use core::ffi::c_void;
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
    /// Suspends the current coroutine, already queued under `role`, until it
    /// is resumed, canceled or its timeout fires. A cancel or timeout removes
    /// it ([`Channel::remove`]) before resuming it, and the timer is deleted
    /// before this returns `CANCELED` and/or `TIMEDOUT` bits.
    ///
    /// # Safety
    /// `ch` is live until the coroutine is resumed.
    unsafe fn wait(&mut self, ch: *mut Channel, role: u32) -> u32;
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

/// Queues the current coroutine as a `role` waiter and suspends it. Returns the
/// error to report, or `ERROR_OK` to proceed.
///
/// # Safety
/// `ch` is live and unborrowed.
unsafe fn wait(ch: *mut Channel, fx: &mut impl Effects, role: u32) -> i32 {
    let co = fx.current();
    // SAFETY: live; the borrow ends before the effect.
    unsafe { (*ch).queue(role) }.push_back(co);
    // SAFETY: forwarded from the caller.
    let flags = unsafe { fx.wait(ch, role) };
    if flags & CANCELED != 0 {
        ERROR_CANCELED
    } else if flags & TIMEDOUT != 0 {
        ERROR_TIMEOUT
    } else {
        ERROR_OK
    }
}

/// `Channel::pop`: the oldest item, or null with the error set.
///
/// # Safety
/// `ch` is live and unborrowed. After the final resume it is not touched.
pub unsafe fn pop(ch: *mut Channel, fx: &mut impl Effects) -> *mut c_void {
    {
        // SAFETY: live; the borrow ends before the next effect.
        let c = unsafe { &mut *ch };
        if c.closed && c.data.is_empty() {
            c.error = ERROR_CLOSED;
            return core::ptr::null_mut();
        }
        if c.data.is_empty() || !c.consumers.is_empty() {
            // SAFETY: forwarded from the caller.
            let mut status = unsafe { wait(ch, fx, CONSUMER) };
            // SAFETY: live again after the suspension.
            let c = unsafe { &mut *ch };
            if status == ERROR_OK && c.closed && c.data.is_empty() {
                status = ERROR_CLOSED;
            }
            if status != ERROR_OK {
                c.error = status;
                return core::ptr::null_mut();
            }
        }
    }
    // SAFETY: live; the borrow ends before the resume.
    let c = unsafe { &mut *ch };
    let data = c.pop_data();
    if let Some(co) = c.producers.pop_front() {
        fx.resume(co);
    }
    data
}

/// `Channel::push`: false with the error set when it fails.
///
/// # Safety
/// As for [`pop`].
pub unsafe fn push(ch: *mut Channel, fx: &mut impl Effects, data: *mut c_void) -> bool {
    {
        // SAFETY: live; the borrow ends before the next effect.
        let c = unsafe { &mut *ch };
        if c.closed {
            c.error = ERROR_CLOSED;
            return false;
        }
        if c.is_full() || !c.producers.is_empty() {
            // SAFETY: forwarded from the caller.
            let mut status = unsafe { wait(ch, fx, PRODUCER) };
            // SAFETY: live again after the suspension.
            let c = unsafe { &mut *ch };
            if status == ERROR_OK && c.closed {
                status = ERROR_CLOSED;
            }
            if status != ERROR_OK {
                c.error = status;
                return false;
            }
        }
    }
    // SAFETY: live; the borrow ends before the resume.
    let c = unsafe { &mut *ch };
    c.data.push_back(data);
    if let Some(co) = c.consumers.pop_front() {
        fx.resume(co);
    }
    true
}

/// `Channel::close`: wakes producers, then consumers.
///
/// # Safety
/// As for [`pop`]; the channel stays alive until close returns.
pub unsafe fn close(ch: *mut Channel, fx: &mut impl Effects) -> bool {
    // SAFETY: live.
    if unsafe { (*ch).closed } {
        return false;
    }
    // SAFETY: live.
    unsafe { (*ch).closed = true };
    for role in [PRODUCER, CONSUMER] {
        // Re-read the queue after each resume: the woken waiter may change it.
        // SAFETY: live; the borrow ends before the resume.
        while let Some(co) = unsafe { (*ch).queue(role).pop_front() } {
            fx.resume(co);
        }
    }
    true
}

#[cfg(test)]
#[path = "../../../tests/modules/coroutine/channel.rs"]
mod tests;
