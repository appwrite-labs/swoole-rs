//! The coroutine socket's `network::IOVector` ([`IoVector`]), and its
//! `Socket::readv_all` and `Socket::writev_all`: transfer a whole vector, waiting
//! for the socket while it would block. The member itself, [`Co::vector`], is in
//! `socket_members.rs`; it passes the `network::Socket` call and the vector's
//! remaining count.
use crate::co_io::{Timer, available, check, errno, set_err, would_block};
use crate::co_socket::Co;
use crate::socket_wait::wait_barrier;
use core::ffi::{c_int, c_void};
use core::ptr::NonNull;
use swoole_rs_reactor::{ReactorIov, swr_v1_reactor_iov_view};

/// A `network::IOVector` over the caller's iovecs, destroyed on drop, and the
/// direction it was made for. Its owner is its first member, so a pointer to it is
/// the `IOVector *` the `network::Socket` calls read (they read only that member).
#[repr(C)]
pub struct IoVector {
    owner: NonNull<ReactorIov>,
    write: bool,
}

impl IoVector {
    /// A vector over a copy of the `count` iovecs at `vectors`, to write from when
    /// `write` and to read into otherwise; none when the copy fails, where
    /// `IOVector`'s constructor throws.
    ///
    /// # Safety
    /// `vectors` holds `count` iovecs, and their buffers stay valid while the
    /// vector lives: readable when `write`, writable otherwise.
    #[must_use]
    pub unsafe fn new(vectors: *const libc::iovec, count: c_int, write: bool) -> Option<Self> {
        let vectors = match usize::try_from(count).ok()? {
            0 => &[][..],
            // SAFETY: as the caller promises; the iovecs are only copied.
            count => unsafe { core::slice::from_raw_parts(vectors, count) },
        };
        let owner = NonNull::from(Box::leak(ReactorIov::new(vectors)?));
        Some(Self { owner, write })
    }

    /// Whether the vector was made to write from.
    pub(crate) const fn write(&self) -> bool {
        self.write
    }

    fn owner(&self) -> &ReactorIov {
        // SAFETY: the owner `new` made, live until drop.
        unsafe { self.owner.as_ref() }
    }

    /// The iovec index and byte offset where the transfers stopped.
    #[must_use]
    pub fn stopped(&self) -> (c_int, usize) {
        let state = swr_v1_reactor_iov_view(self.owner());
        (state.index, state.offset_bytes)
    }

    /// The iovecs left to transfer.
    pub(crate) fn remaining(&self) -> usize {
        usize::try_from(swr_v1_reactor_iov_view(self.owner()).remain_count).unwrap_or(0)
    }

    /// The iovecs left to transfer, the current one trimmed to where the transfers
    /// stopped. The borrow ends before [`update`](Self::update) can move them.
    #[cfg(feature = "iouring")]
    pub(crate) fn iovecs(&self) -> &[libc::iovec] {
        let view = swr_v1_reactor_iov_view(self.owner());
        let count = usize::try_from(view.remain_count).unwrap_or(0);
        if count == 0 {
            return &[];
        }
        // SAFETY: the owner's iterator names `remain_count` live iovecs, which only
        // `update` (taking `&mut self`) changes.
        unsafe { core::slice::from_raw_parts(view.iterator, count) }
    }

    /// Advance past a transfer's `result`. It takes `&mut self`, so no transfer
    /// holding [`iovecs`](Self::iovecs) can advance the vector itself.
    #[cfg(feature = "iouring")]
    pub(crate) fn update(&mut self, result: isize) {
        // SAFETY: the live owner; no reference to it is held across the call.
        unsafe { self.owner.as_mut() }.advance(result);
    }

    /// The `IOVector *` the `network::Socket` calls take.
    pub(crate) fn as_ptr(&self) -> *const c_void {
        core::ptr::from_ref(self).cast()
    }
}

impl Drop for IoVector {
    fn drop(&mut self) {
        // SAFETY: the box `new` leaked, released once.
        drop(unsafe { Box::from_raw(self.owner.as_ptr()) });
    }
}

/// Retries the transfer once the socket is ready; true to keep waiting. Installed
/// only for the wait that lends it ([`wait_barrier`]), which erases its lifetime.
pub type Barrier = *mut (dyn FnMut() -> bool + 'static);

struct Transfer<T, R> {
    socket: Co,
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
        would_block(self.socket)
    }
}

/// `readv_all`/`writev_all` in `event`'s direction, timed by the socket's read or
/// write timeout: the bytes transferred, the first call's result when it finished
/// or failed, or -1 when the socket is not available. `transfer` is one
/// `network::Socket` call over the vector and `remaining` its remaining count.
///
pub(crate) fn vector_all<T: FnMut() -> isize, R: FnMut() -> usize>(
    socket: Co,
    event: u32,
    transfer: T,
    remaining: R,
) -> isize {
    if !available(socket, event) {
        return -1;
    }
    let timeout = socket.timeout(crate::transfer::timeout_kind(event));
    let mut timer = Timer::new(socket, event, timeout);
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
        set_err(socket, errno());
        first
    // The iov must not be touched again once complete.
    } else if first == 0 || (state.remaining)() == 0 {
        first
    } else {
        // The barrier's retry: transfer until the vector is done or a call returns
        // <= 0; keep waiting only when the last call would block.
        let mut retry = || {
            loop {
                state.transfer();
                if state.result <= 0 || (state.remaining)() == 0 {
                    break;
                }
            }
            state.result < 0 && state.would_block()
        };
        if timer.start() && wait_barrier(socket, event, &mut retry) {
            check(socket, state.result);
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
    use crate::co_socket::Co;
    use crate::co_socket::TestSocket;
    use crate::shutdown::{READ, WRITE};
    use crate::socket_wait::ready;
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
        ON_SUSPEND.set(Some(Box::new(move |socket: Co, event| {
            while ready(socket, event, false).is_null() {
                log("again");
            }
            if failure == 3 {
                socket.set_err_code(libc::ETIMEDOUT);
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
        let result = vector_all(socket.co(), event, transfer, remaining);
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
