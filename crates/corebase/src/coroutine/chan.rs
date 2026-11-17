// SPDX-License-Identifier: Apache-2.0
//! `swoole::coroutine::Channel`: capacity, closed flag, last error, the data
//! FIFO (of any item type) and both waiter queues, and pop/push/close as whole
//! operations.
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

/// Which queue a waiter is in.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Role {
    Producer,
    Consumer,
}

/// Numeric errors reported by [`Channel::error`] and [`Stats`].
pub const ERROR_OK: i32 = 0;
pub const ERROR_TIMEOUT: i32 = -1;
pub const ERROR_CLOSED: i32 = -2;
pub const ERROR_CANCELED: i32 = -3;
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Error {
    Timeout = ERROR_TIMEOUT as isize,
    Closed = ERROR_CLOSED as isize,
    Canceled = ERROR_CANCELED as isize,
}

/// [`Effects::wait`]'s result bits.
pub const CANCELED: u32 = 1;
pub const TIMEDOUT: u32 = 2;

/// A channel of `T` whose waiters are the host's coroutine handles, `W`.
pub struct Channel<T, W = *mut c_void> {
    capacity: usize,
    closed: bool,
    error: Option<Error>,
    data: VecDeque<T>,
    producers: VecDeque<W>,
    consumers: VecDeque<W>,
}

/// The host's coroutines, for one operation.
pub trait Effects {
    /// The host's handle on a coroutine.
    type Waiter: Copy + PartialEq;
    /// The current coroutine, queued as a waiter.
    fn current(&self) -> Self::Waiter;
    /// Suspends the current coroutine, already queued as a waiter, until it
    /// is resumed, canceled or its timeout fires. The timer is deleted before
    /// this returns `CANCELED` and/or `TIMEDOUT` bits; the operation then
    /// removes the waiter ([`Channel::remove`]) before anything else runs.
    fn wait(&mut self) -> u32;
    /// Resumes a waiter just taken from a queue.
    fn resume(&mut self, co: Self::Waiter);
}

impl<T, W: Copy + PartialEq> Channel<T, W> {
    #[must_use]
    pub fn new(capacity: usize) -> Self {
        Self {
            capacity,
            closed: false,
            error: None,
            data: [].into(),
            producers: [].into(),
            consumers: [].into(),
        }
    }

    fn queue(&mut self, role: Role) -> &mut VecDeque<W> {
        match role {
            Role::Producer => &mut self.producers,
            Role::Consumer => &mut self.consumers,
        }
    }

    /// Drops every queued `co` waiter of `role`: a timeout or cancel before resume.
    pub fn remove(&mut self, role: Role, co: W) {
        self.queue(role).retain(|&entry| entry != co);
    }

    /// Removes and returns the oldest item.
    pub fn pop_data(&mut self) -> Option<T> {
        self.data.pop_front()
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
        self.error.map_or(ERROR_OK, |error| error as i32)
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
fn with<T, W, R>(ch: &UnsafeCell<Channel<T, W>>, f: impl FnOnce(&mut Channel<T, W>) -> R) -> R {
    // SAFETY: as above.
    f(unsafe { ch.as_mut_unchecked() })
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
pub fn stats<T, W: Copy + PartialEq>(ch: &UnsafeCell<Channel<T, W>>) -> Stats {
    with(ch, |c| Stats {
        error: c.error(),
        length: c.length(),
        producers: c.producer_num(),
        consumers: c.consumer_num(),
        full: c.is_full(),
    })
}

/// [`Channel::pop_data`]: the oldest item without waiting.
pub fn take<T, W: Copy + PartialEq>(ch: &UnsafeCell<Channel<T, W>>) -> Option<T> {
    with(ch, Channel::pop_data)
}

/// Queues and suspends the current coroutine, then publishes any error before
/// proceeding. Cancellation wins over timeout, and both win over a closed channel.
fn wait<T, F: Effects>(
    ch: &UnsafeCell<Channel<T, F::Waiter>>,
    fx: &mut F,
    role: Role,
) -> Result<(), Error> {
    let co = fx.current();
    with(ch, |c| c.queue(role).push_back(co));
    let flags = fx.wait();
    if flags != 0 {
        with(ch, |c| c.remove(role, co));
    }
    with(ch, |c| {
        let error = if flags & CANCELED != 0 {
            Error::Canceled
        } else if flags & TIMEDOUT != 0 {
            Error::Timeout
        } else if c.closed && (role == Role::Producer || c.data.is_empty()) {
            Error::Closed
        } else {
            return Ok(());
        };
        c.error = Some(error);
        Err(error)
    })
}

/// `Channel::pop`: the oldest item, or None with the error set. After the
/// final resume the channel is not touched.
pub fn pop<T, F: Effects>(ch: &UnsafeCell<Channel<T, F::Waiter>>, fx: &mut F) -> Option<T> {
    let (closed, wait_first) = with(ch, |c| {
        (
            c.closed && c.data.is_empty(),
            c.data.is_empty() || !c.consumers.is_empty(),
        )
    });
    if closed {
        with(ch, |c| c.error = Some(Error::Closed));
        return None;
    }
    if wait_first && wait(ch, fx, Role::Consumer).is_err() {
        return None;
    }
    let (data, producer) = with(ch, |c| (c.pop_data(), c.producers.pop_front()));
    if let Some(co) = producer {
        fx.resume(co);
    }
    data
}

/// `Channel::push`: `data` back, with the error set, when it fails. After the
/// final resume the channel is not touched.
/// # Errors
/// `data`, not queued: the channel is closed, or the wait timed out or was canceled.
pub fn push<T, F: Effects>(
    ch: &UnsafeCell<Channel<T, F::Waiter>>,
    fx: &mut F,
    data: T,
) -> Result<(), T> {
    let (closed, wait_first) = with(ch, |c| (c.closed, c.is_full() || !c.producers.is_empty()));
    if closed {
        with(ch, |c| c.error = Some(Error::Closed));
        return Err(data);
    }
    if wait_first && wait(ch, fx, Role::Producer).is_err() {
        return Err(data);
    }
    let consumer = with(ch, |c| {
        c.data.push_back(data);
        c.consumers.pop_front()
    });
    if let Some(co) = consumer {
        fx.resume(co);
    }
    Ok(())
}

/// `Channel::close`: wakes producers, then consumers.
pub fn close<T, F: Effects>(ch: &UnsafeCell<Channel<T, F::Waiter>>, fx: &mut F) -> bool {
    if with(ch, |c| std::mem::replace(&mut c.closed, true)) {
        return false;
    }
    for role in [Role::Producer, Role::Consumer] {
        // Re-read the queue after each resume: the woken waiter may change it.
        while let Some(co) = with(ch, |c| c.queue(role).pop_front()) {
            fx.resume(co);
        }
    }
    true
}

#[cfg(test)]
#[path = "../../../../tests/modules/coroutine/channel.rs"]
mod tests;
