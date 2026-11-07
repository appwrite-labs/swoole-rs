//! `Socket::readv_all` and `Socket::writev_all` of the coroutine socket: transfer a
//! whole `IOVector`, waiting for the socket while it would block. The members
//! themselves are in `socket_members.rs`; they pass the `network::Socket` call and
//! the vector's remaining count.
use crate::co_io::{Timer, available, check, errno, set_err, would_block};
use crate::co_socket::CoSocket;
use crate::shutdown::READ;
use crate::socket_wait::wait;
use core::ffi::c_void;

/// Retries the transfer once the socket is ready; true to keep waiting.
pub type Barrier = unsafe extern "C" fn(*mut c_void) -> bool;

struct Transfer<T, R> {
    socket: *mut CoSocket,
    transfer: T,
    remaining: R,
    result: isize,
    total: isize,
}

impl<T: FnMut() -> isize, R: FnMut() -> usize> Transfer<T, R> {
    fn transfer(&mut self) {
        self.result = (self.transfer)();
        if self.result > 0 {
            self.total = self.total.wrapping_add(self.result);
        }
    }

    fn would_block(&self) -> bool {
        // SAFETY: an open record (it was available).
        unsafe { would_block(self.socket) }
    }
}

/// The barrier's retry: transfer until the vector is done or a call returns <= 0;
/// keep waiting only when the last call would block.
unsafe extern "C" fn retry<T: FnMut() -> isize, R: FnMut() -> usize>(state: *mut c_void) -> bool {
    // SAFETY: `state` is the Transfer that `vector_all` lent to the wait.
    let transfer = unsafe { &mut *state.cast::<Transfer<T, R>>() };
    loop {
        transfer.transfer();
        if transfer.result <= 0 || (transfer.remaining)() == 0 {
            break;
        }
    }
    transfer.result < 0 && transfer.would_block()
}

/// `readv_all`/`writev_all` in `event`'s direction, timed by the socket's read or
/// write timeout: the bytes transferred, the first call's result when it finished
/// or failed, or -1 when the socket is not available. `transfer` is one
/// `network::Socket` call over the vector and `remaining` its remaining count.
///
/// # Safety
/// `socket` is a live record, not borrowed elsewhere for the call.
pub(crate) unsafe fn vector_all<T: FnMut() -> isize, R: FnMut() -> usize>(
    socket: *mut CoSocket,
    event: u32,
    transfer: T,
    remaining: R,
) -> isize {
    // SAFETY: as the caller promises.
    if !unsafe { available(socket, event) } {
        return -1;
    }
    // SAFETY: as the caller promises.
    let record = unsafe { &*socket };
    // SAFETY: an available record is open; the read ends here.
    let timeouts = unsafe { record.record() }.timeouts;
    let timeout = if event == READ {
        timeouts.read
    } else {
        timeouts.write
    };
    // SAFETY: the live record; the timer stops before this returns.
    let mut timer = unsafe { Timer::new(socket, event, timeout) };
    let mut state = Transfer {
        socket,
        transfer,
        remaining,
        result: 0,
        total: 0,
    };
    state.transfer();
    let first = state.result;
    let result = if first < 0 && !state.would_block() {
        // SAFETY: the live record.
        unsafe { set_err(socket, errno()) };
        first
    // The iov must not be touched again once complete.
    } else if first == 0 || (state.remaining)() == 0 {
        first
    } else {
        let barrier: (Barrier, *mut c_void) = (retry::<T, R>, (&raw mut state).cast());
        // SAFETY: `state` outlives the wait, which calls `retry` with it.
        if timer.start() && unsafe { wait(socket, event, Some(barrier), core::ptr::null_mut(), 0) }
        {
            // SAFETY: the live record.
            unsafe { check(socket, state.result) };
        }
        state.total
    };
    timer.stop();
    result
}

#[cfg(test)]
#[expect(
    clippy::cast_possible_wrap,
    clippy::cast_sign_loss,
    reason = "the test vector drives the native integer interface with small lengths"
)]
mod tests {
    use super::*;
    use crate::co_io::set_errno;
    use crate::co_io::test_host::{ON_SUSPEND, TIMER_OK, install, log, take};
    use crate::co_socket::TestSocket;
    use crate::shutdown::WRITE;
    use crate::socket_wait::swr_v1_hook_socket_ready;
    use std::cell::RefCell;
    use std::collections::VecDeque;

    const EAGAIN: isize = -(libc::EAGAIN as isize);
    const EINTR: isize = -(libc::EINTR as isize);
    const EINVAL: isize = -(libc::EINVAL as isize);
    const EIO: isize = -(libc::EIO as isize);

    /// An `IOVector` of entries of these sizes, moved by scripted call results
    /// (bytes, or -errno), each clamped to the bytes left.
    struct Vector {
        entries: Vec<usize>,
        index: usize,
        offset: usize,
        results: VecDeque<isize>,
    }

    impl Vector {
        fn new(entries: &[usize], results: &[isize]) -> Self {
            Self {
                entries: entries.to_vec(),
                index: 0,
                offset: 0,
                results: results.iter().copied().collect(),
            }
        }

        fn count(&self) -> usize {
            self.entries.len() - self.index
        }

        /// `get_remain_count`, logged.
        fn remaining(&self, effects: &mut Vec<String>) -> usize {
            effects.push(format!("vectors:{}", self.count()));
            self.count()
        }

        /// One readv/writev: the result, and errno when it fails.
        fn io(&mut self, effects: &mut Vec<String>) -> (isize, i32) {
            effects.push("io".into());
            let result = self.results.pop_front().unwrap_or(0);
            if result < 0 {
                return (-1, i32::try_from(-result).unwrap());
            }
            let left = self.entries[self.index..].iter().sum::<usize>() - self.offset;
            let mut amount = (result as usize).min(left);
            let moved = amount;
            while self.index < self.entries.len()
                && amount >= self.entries[self.index] - self.offset
            {
                amount -= self.entries[self.index] - self.offset;
                self.index += 1;
                self.offset = 0;
            }
            self.offset += amount;
            (moved as isize, 0)
        }
    }

    /// How a scenario fails besides its results: 1 the socket is closed, 2 the
    /// timer can't be added, 3 the wait wakes with an error. Returns the result,
    /// the socket's error, the effects in order and the vector's remaining count.
    fn run(
        event: u32,
        layout: &[usize],
        results: &[isize],
        failure: u32,
    ) -> (isize, i32, Vec<String>, usize) {
        install();
        let mut socket = TestSocket::new();
        unsafe { socket.0.record() }.timeouts.read = 1.0;
        unsafe { socket.0.record() }.timeouts.write = 1.0;
        socket.0.err_code = 9;
        if failure == 1 {
            socket.0.sock_fd = -1;
        }
        TIMER_OK.set(failure != 2);
        ON_SUSPEND.set(Some(Box::new(move |socket: *mut CoSocket, event| {
            let state = unsafe { (*socket).state };
            while unsafe { swr_v1_hook_socket_ready(state, event, false) }.is_null() {
                log("again");
            }
            if failure == 3 {
                unsafe { (*socket).err_code = libc::ETIMEDOUT };
            }
        })));
        let vector = RefCell::new(Vector::new(layout, results));
        let transfer = || {
            let mut effects = Vec::new();
            let (result, error) = vector.borrow_mut().io(&mut effects);
            effects.into_iter().for_each(log);
            set_errno(error);
            result
        };
        let remaining = || {
            let mut effects = Vec::new();
            let count = vector.borrow().remaining(&mut effects);
            effects.into_iter().for_each(log);
            count
        };
        let result = unsafe { vector_all(socket.ptr(), event, transfer, remaining) };
        let error = socket.0.err_code;
        socket.0.sock_fd = 100;
        ON_SUSPEND.set(None);
        let effects = take()
            .into_iter()
            .filter(|e| !e.starts_with("last-error"))
            .collect();
        (result, error, effects, vector.borrow().count())
    }

    /// Upstream's `readv_all`/`writev_all`, transcribed.
    fn upstream(
        event: u32,
        layout: &[usize],
        results: &[isize],
        failure: u32,
    ) -> (isize, i32, Vec<String>, usize) {
        let mut effects = Vec::new();
        let mut vector = Vector::new(layout, results);
        if failure == 1 {
            return (-1, libc::EBADF, effects, vector.count());
        }
        let mut error = 9;
        let (first, errno) = vector.io(&mut effects);
        if first < 0 && errno != libc::EAGAIN {
            return (first, errno, effects, vector.count());
        }
        if first == 0 {
            return (first, error, effects, vector.count());
        }
        let mut total = first.max(0);
        if vector.remaining(&mut effects) == 0 {
            return (first, error, effects, vector.count());
        }
        effects.push("timer:add:1".into());
        if failure == 2 {
            return (total, error, effects, vector.count());
        }
        error = 0;
        effects.push(format!("add:{event}"));
        effects.push(format!("yield:{event}"));
        let (mut result, mut errno);
        loop {
            loop {
                (result, errno) = vector.io(&mut effects);
                if result <= 0 {
                    break;
                }
                total += result;
                if vector.remaining(&mut effects) == 0 {
                    break;
                }
            }
            if !(result < 0 && errno == libc::EAGAIN) {
                break;
            }
            effects.push("again".into());
        }
        effects.push(format!("remove:{event}"));
        if failure == 3 {
            error = libc::ETIMEDOUT;
        } else if result < 0 {
            error = errno;
        }
        effects.push("timer:delete".into());
        (total, error, effects, vector.count())
    }

    /// The packet gate's former vector scenarios 240..624: each direction, call
    /// results, failure and vector layout.
    #[test]
    fn ported_vector_all_matrix() {
        let modes: [&[isize]; 12] = [
            &[0],
            &[8],
            &[EINVAL],
            &[EAGAIN, 8],
            &[2, 3, 3],
            &[2, 0],
            &[2, EAGAIN, 3, 3],
            &[2, EAGAIN, EIO],
            &[EAGAIN, 0],
            &[EINTR],
            &[2, 2, EAGAIN, 4],
            &[2, 2, 4],
        ];
        let layouts: [&[usize]; 4] = [&[8], &[2, 3, 3], &[0, 3, 0, 5], &[]];
        for event in [READ, WRITE] {
            for results in modes {
                for failure in 0..4 {
                    for layout in layouts {
                        assert_eq!(
                            run(event, layout, results, failure),
                            upstream(event, layout, results, failure),
                            "{event} {results:?} {failure} {layout:?}"
                        );
                    }
                }
            }
        }
    }

    #[test]
    fn a_complete_first_transfer_returns_without_waiting() {
        let (result, error, effects, left) = run(READ, &[8], &[8], 0);
        assert_eq!((result, error, left), (8, 9, 0));
        assert_eq!(effects, ["io", "vectors:0"]);
    }

    #[test]
    fn partial_transfers_wait_and_accumulate() {
        let (result, error, effects, left) = run(WRITE, &[2, 3, 3], &[2, EAGAIN, 3, 3], 0);
        assert_eq!((result, error, left), (8, 0, 0));
        assert_eq!(
            effects,
            [
                "io",
                "vectors:2",
                "timer:add:1",
                "add:1024",
                "yield:1024",
                "io",
                "again",
                "io",
                "vectors:1",
                "io",
                "vectors:0",
                "remove:1024",
                "timer:delete"
            ]
        );
    }
}
