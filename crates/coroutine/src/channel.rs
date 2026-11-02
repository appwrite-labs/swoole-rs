// SPDX-License-Identifier: Apache-2.0
//! `swoole::coroutine::Channel`: capacity, closed flag, last error, the data
//! FIFO and both waiter queues, and pop/push/close as whole operations.
//!
//! C++ supplies the effects through [`Effects`]: arming and deleting the timeout
//! timer, suspending the current coroutine (with its cancel function) and
//! resuming a waiter. No Rust reference to the channel lives across an effect:
//! other coroutines use the channel while this one is suspended, and a resumed
//! waiter may delete it, so an operation never touches it after a resume that
//! ends it.
use core::ffi::c_void;
use std::collections::VecDeque;

pub const PRODUCER: u32 = 1;
pub const CONSUMER: u32 = 2;
pub const ERROR_OK: i32 = 0;
pub const ERROR_TIMEOUT: i32 = -1;
pub const ERROR_CLOSED: i32 = -2;
pub const ERROR_CANCELED: i32 = -3;
/// An operation could not allocate; C++ raises `std::bad_alloc` after return.
pub const ALLOCATION_REFUSED: i32 = -5;

type Token = *mut c_void;

#[derive(Default)]
pub struct Channel {
    capacity: usize,
    closed: bool,
    error: i32,
    data: VecDeque<Token>,
    producers: VecDeque<Token>,
    consumers: VecDeque<Token>,
}

#[repr(C)]
pub struct ChannelView {
    pub length: usize,
    pub producers: usize,
    pub consumers: usize,
    pub error: i32,
    pub closed: bool,
    pub full: bool,
    pub empty: bool,
}

/// C++ effects for one operation; `ctx` is its per-call state.
#[repr(C)]
pub struct Effects {
    pub ctx: *mut c_void,
    /// The current coroutine, queued as a waiter.
    pub current: Token,
    /// Arms the timeout timer (only called for a positive timeout).
    pub arm_timer: unsafe extern "C" fn(*mut c_void),
    /// Deletes the timer if it is still armed.
    pub delete_timer: unsafe extern "C" fn(*mut c_void),
    /// Yields the current coroutine, queued under `role`, until it is resumed,
    /// canceled or timed out. Returns `CANCELED` and/or `TIMEDOUT` bits.
    pub suspend: unsafe extern "C" fn(*mut c_void, u32) -> u32,
    /// Resumes a waiter just taken from the `role` queue.
    pub resume: unsafe extern "C" fn(*mut c_void, u32, Token),
    /// Trace points: `PUSHED` with the new length, `CLOSED`.
    pub trace: unsafe extern "C" fn(*mut c_void, u32, usize),
}

pub const CANCELED: u32 = 1;
pub const TIMEDOUT: u32 = 2;
pub const TRACE_PUSHED: u32 = 0;
pub const TRACE_CLOSED: u32 = 1;

#[repr(C)]
pub struct Outcome {
    /// `ERROR_OK`, a negative `ERROR_*` or `ALLOCATION_REFUSED`.
    pub status: i32,
    /// The popped item.
    pub data: Token,
}

impl Channel {
    fn queue(&mut self, role: u32) -> &mut VecDeque<Token> {
        // Upstream's enqueue/dequeue test for PRODUCER; removal tests for CONSUMER.
        if role == PRODUCER {
            &mut self.producers
        } else {
            &mut self.consumers
        }
    }

    fn remove(&mut self, role: u32, co: Token) {
        let queue = if role == CONSUMER {
            &mut self.consumers
        } else {
            &mut self.producers
        };
        queue.retain(|&entry| entry != co);
    }

    #[must_use]
    pub fn view(&self) -> ChannelView {
        ChannelView {
            length: self.data.len(),
            producers: self.producers.len(),
            consumers: self.consumers.len(),
            error: self.error,
            closed: self.closed,
            full: self.data.len() == self.capacity,
            empty: self.data.is_empty(),
        }
    }
}

fn push_back(queue: &mut VecDeque<Token>, token: Token) -> bool {
    queue.try_reserve(1).is_ok() && {
        queue.push_back(token);
        true
    }
}

/// Queues the current coroutine as a `role` waiter and suspends it. Returns the
/// error to report, `ERROR_OK` to proceed, or `ALLOCATION_REFUSED`.
///
/// # Safety
/// `ch` is live and unborrowed; `fx` holds valid C++ effects.
unsafe fn wait(ch: *mut Channel, fx: &Effects, role: u32, timeout: bool) -> i32 {
    if timeout {
        // SAFETY: a valid C++ effect.
        unsafe { (fx.arm_timer)(fx.ctx) };
    }
    // SAFETY: live; the borrow ends before the next effect.
    if !push_back(unsafe { (*ch).queue(role) }, fx.current) {
        // SAFETY: a valid C++ effect.
        unsafe { (fx.delete_timer)(fx.ctx) };
        return ALLOCATION_REFUSED;
    }
    // SAFETY: a valid C++ effect.
    let flags = unsafe { (fx.suspend)(fx.ctx, role) };
    // SAFETY: a valid C++ effect.
    unsafe { (fx.delete_timer)(fx.ctx) };
    if flags & CANCELED != 0 {
        ERROR_CANCELED
    } else if flags & TIMEDOUT != 0 {
        ERROR_TIMEOUT
    } else {
        ERROR_OK
    }
}

/// # Safety
/// As for [`wait`]. After the final resume the channel is not touched.
unsafe fn pop(ch: *mut Channel, fx: &Effects, timeout: bool) -> Outcome {
    let mut out = Outcome {
        status: ERROR_OK,
        data: core::ptr::null_mut(),
    };
    {
        // SAFETY: live; the borrow ends before the next effect.
        let c = unsafe { &mut *ch };
        if c.closed && c.data.is_empty() {
            c.error = ERROR_CLOSED;
            out.status = ERROR_CLOSED;
            return out;
        }
        if c.data.is_empty() || !c.consumers.is_empty() {
            // SAFETY: forwarded from the caller.
            let mut status = unsafe { wait(ch, fx, CONSUMER, timeout) };
            if status == ALLOCATION_REFUSED {
                out.status = status;
                return out;
            }
            // SAFETY: live again after the suspension.
            let c = unsafe { &mut *ch };
            if status == ERROR_OK && c.closed && c.data.is_empty() {
                status = ERROR_CLOSED;
            }
            if status != ERROR_OK {
                c.error = status;
                out.status = status;
                return out;
            }
        }
    }
    // SAFETY: live; the borrow ends before the resume.
    let c = unsafe { &mut *ch };
    out.data = c.data.pop_front().unwrap_or(core::ptr::null_mut());
    if let Some(co) = c.producers.pop_front() {
        // SAFETY: a valid C++ effect; the channel is not touched afterwards.
        unsafe { (fx.resume)(fx.ctx, PRODUCER, co) };
    }
    out
}

/// # Safety
/// As for [`pop`].
unsafe fn push(ch: *mut Channel, fx: &Effects, timeout: bool, data: Token) -> i32 {
    {
        // SAFETY: live; the borrow ends before the next effect.
        let c = unsafe { &mut *ch };
        if c.closed {
            c.error = ERROR_CLOSED;
            return ERROR_CLOSED;
        }
        if c.data.len() == c.capacity || !c.producers.is_empty() {
            // SAFETY: forwarded from the caller.
            let mut status = unsafe { wait(ch, fx, PRODUCER, timeout) };
            if status == ALLOCATION_REFUSED {
                return status;
            }
            // SAFETY: live again after the suspension.
            let c = unsafe { &mut *ch };
            if status == ERROR_OK && c.closed {
                status = ERROR_CLOSED;
            }
            if status != ERROR_OK {
                c.error = status;
                return status;
            }
        }
    }
    // SAFETY: live; the borrow ends before the trace.
    let c = unsafe { &mut *ch };
    if !push_back(&mut c.data, data) {
        return ALLOCATION_REFUSED;
    }
    let length = c.data.len();
    // SAFETY: a valid C++ effect.
    unsafe { (fx.trace)(fx.ctx, TRACE_PUSHED, length) };
    // SAFETY: live; the borrow ends before the resume.
    if let Some(co) = unsafe { (*ch).consumers.pop_front() } {
        // SAFETY: a valid C++ effect; the channel is not touched afterwards.
        unsafe { (fx.resume)(fx.ctx, CONSUMER, co) };
    }
    ERROR_OK
}

/// # Safety
/// As for [`pop`]; the channel stays alive until close returns.
unsafe fn close(ch: *mut Channel, fx: &Effects) -> bool {
    // SAFETY: live.
    if unsafe { (*ch).closed } {
        return false;
    }
    // SAFETY: a valid C++ effect.
    unsafe { (fx.trace)(fx.ctx, TRACE_CLOSED, 0) };
    // SAFETY: live.
    unsafe { (*ch).closed = true };
    for role in [PRODUCER, CONSUMER] {
        // Re-read the queue after each resume: the woken waiter may change it.
        // SAFETY: live; the borrow ends before the resume.
        while let Some(co) = unsafe { (*ch).queue(role).pop_front() } {
            // SAFETY: a valid C++ effect.
            unsafe { (fx.resume)(fx.ctx, role, co) };
        }
    }
    true
}

/// A new channel, or null when allocation fails.
#[unsafe(no_mangle)]
pub extern "C" fn swr_v1_channel_new(capacity: usize) -> *mut Channel {
    Box::into_raw(Box::new(Channel {
        capacity,
        ..Channel::default()
    }))
}

/// # Safety
/// `ch` came from [`swr_v1_channel_new`] and is freed once.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn swr_v1_channel_free(ch: *mut Channel) {
    // SAFETY: the caller transfers ownership back.
    drop(unsafe { Box::from_raw(ch) });
}

/// # Safety
/// `ch` is live.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn swr_v1_channel_read(ch: *const Channel) -> ChannelView {
    // SAFETY: the caller passes a live channel.
    unsafe { &*ch }.view()
}

/// Removes and returns the oldest item, or null when empty.
/// # Safety
/// `ch` is live and unborrowed.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn swr_v1_channel_pop_data(ch: *mut Channel) -> Token {
    // SAFETY: the caller lends a live channel exclusively.
    unsafe { &mut *ch }
        .data
        .pop_front()
        .unwrap_or(core::ptr::null_mut())
}

/// Drops every queued `co` waiter of `role`: a timeout or cancel before resume.
/// # Safety
/// `ch` is live and unborrowed.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn swr_v1_channel_remove(ch: *mut Channel, role: u32, co: Token) {
    // SAFETY: the caller lends a live channel exclusively.
    unsafe { &mut *ch }.remove(role, co);
}

/// # Safety
/// `ch` is live and unborrowed; `fx` is valid and its effects uphold the
/// module contract.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn swr_v1_channel_pop(
    ch: *mut Channel,
    positive_timeout: bool,
    fx: *const Effects,
) -> Outcome {
    // SAFETY: the caller passes valid effects.
    let fx = unsafe { &*fx };
    // SAFETY: forwarded from the caller.
    unsafe { pop(ch, fx, positive_timeout) }
}

/// # Safety
/// As for [`swr_v1_channel_pop`].
#[unsafe(no_mangle)]
pub unsafe extern "C" fn swr_v1_channel_push(
    ch: *mut Channel,
    positive_timeout: bool,
    data: Token,
    fx: *const Effects,
) -> i32 {
    // SAFETY: the caller passes valid effects.
    let fx = unsafe { &*fx };
    // SAFETY: forwarded from the caller.
    unsafe { push(ch, fx, positive_timeout, data) }
}

/// # Safety
/// As for [`swr_v1_channel_pop`].
#[unsafe(no_mangle)]
pub unsafe extern "C" fn swr_v1_channel_close(ch: *mut Channel, fx: *const Effects) -> bool {
    // SAFETY: the caller passes valid effects.
    let fx = unsafe { &*fx };
    // SAFETY: forwarded from the caller.
    unsafe { close(ch, fx) }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::RefCell;

    /// Records effects; `suspend` runs a scripted closure standing in for the
    /// other coroutines that run while this one waits.
    #[derive(Default)]
    struct Log {
        events: Vec<String>,
        on_suspend: Option<Box<dyn FnMut()>>,
        flags: u32,
    }

    thread_local! { static LOG: RefCell<Log> = RefCell::default(); }

    fn event(e: String) {
        LOG.with(|l| l.borrow_mut().events.push(e));
    }
    unsafe extern "C" fn arm(_: *mut c_void) {
        event("arm".into());
    }
    unsafe extern "C" fn delete(_: *mut c_void) {
        event("delete".into());
    }
    unsafe extern "C" fn suspend(_: *mut c_void, role: u32) -> u32 {
        event(format!("suspend {role}"));
        let mut f = LOG.with(|l| l.borrow_mut().on_suspend.take());
        if let Some(f) = f.as_mut() {
            f();
        }
        LOG.with(|l| l.borrow().flags)
    }
    unsafe extern "C" fn resume(_: *mut c_void, role: u32, co: Token) {
        event(format!("resume {role} {}", co as usize));
    }
    unsafe extern "C" fn trace(_: *mut c_void, what: u32, n: usize) {
        event(format!("trace {what} {n}"));
    }

    fn fx(current: usize) -> Effects {
        Effects {
            ctx: core::ptr::null_mut(),
            current: current as Token,
            arm_timer: arm,
            delete_timer: delete,
            suspend,
            resume,
            trace,
        }
    }

    fn take() -> Vec<String> {
        LOG.with(|l| std::mem::take(&mut l.borrow_mut().events))
    }

    fn tok(n: usize) -> Token {
        n as Token
    }

    #[test]
    fn push_then_pop_transfers_without_waiting() {
        let ch = swr_v1_channel_new(1);
        // SAFETY: `ch` is live until freed below.
        unsafe {
            assert_eq!(push(ch, &fx(1), false, tok(7)), ERROR_OK);
            assert!((*ch).view().full);
            let out = pop(ch, &fx(1), false);
            assert_eq!((out.status, out.data), (ERROR_OK, tok(7)));
            swr_v1_channel_free(ch);
        }
        assert_eq!(take(), ["trace 0 1"]);
    }

    #[test]
    fn pop_waits_until_a_producer_pushes() {
        let ch = swr_v1_channel_new(1);
        let addr = ch as usize;
        LOG.with(|l| {
            l.borrow_mut().on_suspend = Some(Box::new(move || {
                let ch = addr as *mut Channel;
                // SAFETY: the test's channel is live.
                assert_eq!(unsafe { push(ch, &fx(2), false, tok(9)) }, ERROR_OK);
            }));
        });
        // SAFETY: live until freed.
        unsafe {
            let out = pop(ch, &fx(1), true);
            assert_eq!((out.status, out.data), (ERROR_OK, tok(9)));
            swr_v1_channel_free(ch);
        }
        assert_eq!(
            take(),
            ["arm", "suspend 2", "trace 0 1", "resume 2 1", "delete"]
        );
    }

    #[test]
    fn waits_report_cancel_before_timeout_and_close() {
        let ch = swr_v1_channel_new(1);
        LOG.with(|l| l.borrow_mut().flags = CANCELED | TIMEDOUT);
        // SAFETY: live until freed.
        unsafe {
            (*ch).closed = false;
            assert_eq!(pop(ch, &fx(1), false).status, ERROR_CANCELED);
            assert_eq!((*ch).error, ERROR_CANCELED);
            LOG.with(|l| l.borrow_mut().flags = TIMEDOUT);
            assert_eq!(pop(ch, &fx(1), false).status, ERROR_TIMEOUT);
            LOG.with(|l| l.borrow_mut().flags = 0);
            (*ch).closed = true;
            // Closed and empty fails before waiting; the waiter queue is untouched.
            assert_eq!(pop(ch, &fx(1), false).status, ERROR_CLOSED);
            swr_v1_channel_free(ch);
        }
        take();
    }

    #[test]
    fn closed_pop_still_drains_data() {
        let ch = swr_v1_channel_new(2);
        // SAFETY: live until freed.
        unsafe {
            push(ch, &fx(1), false, tok(3));
            assert!(close(ch, &fx(1)));
            assert!(!close(ch, &fx(1)));
            assert_eq!(push(ch, &fx(1), false, tok(4)), ERROR_CLOSED);
            assert_eq!(pop(ch, &fx(1), false).data, tok(3));
            assert_eq!(pop(ch, &fx(1), false).status, ERROR_CLOSED);
            swr_v1_channel_free(ch);
        }
        take();
    }

    #[test]
    fn close_wakes_producers_then_consumers() {
        let ch = swr_v1_channel_new(0);
        // SAFETY: live until freed.
        unsafe {
            (*ch).producers.extend([tok(1), tok(2)]);
            (*ch).consumers.push_back(tok(3));
            assert!(close(ch, &fx(9)));
            swr_v1_channel_free(ch);
        }
        assert_eq!(
            take(),
            ["trace 1 0", "resume 1 1", "resume 1 2", "resume 2 3"]
        );
    }

    #[test]
    fn remove_drops_every_entry_of_a_waiter() {
        let ch = swr_v1_channel_new(1);
        // SAFETY: live until freed.
        unsafe {
            (*ch).consumers.extend([tok(1), tok(2), tok(1)]);
            (*ch).producers.push_back(tok(1));
            swr_v1_channel_remove(ch, CONSUMER, tok(1));
            assert_eq!((*ch).view().consumers, 1);
            // Any role but CONSUMER removes from the producers.
            swr_v1_channel_remove(ch, 0, tok(1));
            assert_eq!((*ch).view().producers, 0);
            assert_eq!(swr_v1_channel_pop_data(ch), core::ptr::null_mut());
            swr_v1_channel_free(ch);
        }
    }
}
