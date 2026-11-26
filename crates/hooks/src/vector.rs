//! The coroutine socket's `network::IOVector` ([`IoVector`]), and its
//! `Socket::readv_all` and `Socket::writev_all`: transfer a whole vector, waiting
//! for the socket while it would block. The member itself, [`Co::vector`], is in
//! `socket_members.rs`; it passes the `network::Socket` call and the vector's
//! remaining count.
use crate::co_io::{Timer, available, check, errno, set_err, would_block};
use crate::co_socket::Co;
use crate::socket_wait::wait_retry;
use core::ffi::c_int;
use core::marker::PhantomData;
use std::io::{IoSlice, IoSliceMut};
use swoole_rs_corebase::lent::Spare;
use swoole_rs_corebase::reactor::Events;
use swoole_rs_corebase::reactor::{
    Drain, Plain, ReactorIov, Socket, SpareVectors, swr_v1_reactor_iov_view,
    swr_v1_socket_iov_owned,
};
use swoole_rs_corebase::tls::upstream::{ssl_recv_spare, ssl_send_drain};

/// A `msghdr` over the `count` iovecs at `iov`, with no address and no control data.
fn message_header(iov: *mut libc::iovec, count: usize) -> libc::msghdr {
    /// `count` as the platform's `msg_iovlen`; none of the vectors when it does not fit.
    fn fit<T: TryFrom<usize> + Default>(count: usize) -> T {
        T::try_from(count).unwrap_or_default()
    }
    let mut header: libc::msghdr = swoole_rs_corebase::sys::zeroed();
    header.msg_iov = iov;
    header.msg_iovlen = fit(count);
    header
}

/// A `sendmsg(2)` message: the buffers it borrows, with no address and no control data.
/// Transparent over its header, which [`sendmsg`] takes.
#[repr(transparent)]
pub struct SendMessage<'a> {
    header: libc::msghdr,
    lent: PhantomData<&'a [u8]>,
}

impl<'a> SendMessage<'a> {
    /// The message of `vectors`' bytes, in order.
    #[must_use]
    pub fn new(vectors: &'a [IoSlice<'_>]) -> Self {
        // An `IoSlice` is ABI-compatible with an iovec; a send only reads them.
        let iov = vectors.as_ptr().cast::<libc::iovec>().cast_mut();
        Self {
            header: message_header(iov, vectors.len()),
            lent: PhantomData,
        }
    }

    /// The header, for a call that reads it while the message is borrowed.
    #[cfg(feature = "iouring")]
    pub(crate) const fn as_ptr(&self) -> *const libc::msghdr {
        &raw const self.header
    }

    /// `sendmsg(2)` of the message on `fd`.
    pub(crate) fn send(&self, fd: c_int, flags: c_int) -> Option<usize> {
        usize::try_from(sendmsg(fd, self, flags)).ok()
    }
}

/// A `recvmsg(2)` message: the buffers it borrows to fill, with no address and no
/// control data. Transparent over its header, which [`recvmsg`] takes.
#[repr(transparent)]
pub struct RecvMessage<'a> {
    header: libc::msghdr,
    lent: PhantomData<&'a mut [u8]>,
}

impl<'a> RecvMessage<'a> {
    /// The message that fills `vectors`' buffers, in order.
    #[must_use]
    pub fn new(vectors: &'a mut [IoSliceMut<'_>]) -> Self {
        // An `IoSliceMut` is ABI-compatible with an iovec.
        let iov = vectors.as_mut_ptr().cast::<libc::iovec>();
        Self {
            header: message_header(iov, vectors.len()),
            lent: PhantomData,
        }
    }

    /// `msg_flags`, as the last receive left them.
    #[must_use]
    pub const fn flags(&self) -> c_int {
        self.header.msg_flags
    }

    /// The header, for a call that writes it while the message is borrowed.
    #[cfg(feature = "iouring")]
    pub(crate) const fn as_mut_ptr(&mut self) -> *mut libc::msghdr {
        &raw mut self.header
    }

    /// `recvmsg(2)` into the message on `fd`.
    pub(crate) fn recv(&mut self, fd: c_int, flags: c_int) -> Option<usize> {
        usize::try_from(recvmsg(fd, self, flags)).ok()
    }
}

// libc's, over a message only `new` makes: its header's vectors, and their bytes, are
// borrowed for the message's lifetime, and it names no address and no control data.
// They read the vectors (`recvmsg` fills their bytes and the header's lengths and
// flags) and keep nothing, so they are declared `safe`.
unsafe extern "C" {
    safe fn sendmsg(fd: c_int, message: &SendMessage<'_>, flags: c_int) -> isize;
    safe fn recvmsg(fd: c_int, message: &mut RecvMessage<'_>, flags: c_int) -> isize;
}

/// What an [`IoVector`] is made over: buffers a read fills, or bytes a write sends.
pub enum Vectors<'a, 'b> {
    Read(&'a mut [ReadVec<'b>]),
    Write(&'a [IoSlice<'b>]),
}

pub use swoole_rs_corebase::reactor::ReadVec;

/// A `network::IOVector` over a copy of the caller's iovecs, destroyed on drop. The
/// buffers the iovecs name are borrowed for `'a` as [`Vectors`] lent them: writable
/// and reached through nothing else for a read, readable for a write.
pub struct IoVector<'a> {
    owner: ReactorIov<'a>,
}

impl<'a> IoVector<'a> {
    /// A vector over a copy of `vectors`' iovecs, to read into or to write from as
    /// they were lent; none when the copy fails, where `IOVector`'s constructor throws.
    #[must_use]
    pub fn new(vectors: Vectors<'a, '_>) -> Option<Self> {
        let owner = match vectors {
            Vectors::Read(read) => ReactorIov::reading(read),
            Vectors::Write(written) => ReactorIov::writing(written),
        }?;
        Some(Self { owner })
    }

    fn owner(&self) -> &ReactorIov<'a> {
        &self.owner
    }

    /// The iovec index and byte offset where the transfers stopped.
    #[must_use]
    pub fn stopped(&self) -> (c_int, usize) {
        let state = swr_v1_reactor_iov_view(self.owner());
        (state.index, state.offset_bytes)
    }
}

/// What the coroutine socket's vector members transfer over: an [`IoVector`], or rooms
/// that count what a receive wrote into them ([`SpareVectors`]).
pub trait Vectored {
    /// Whether it was made to write from.
    fn write(&self) -> bool;

    /// The iovecs left to transfer.
    fn remaining(&self) -> usize;

    /// One transfer: `plain`'s over the iovecs left, or with `tls` one buffer at a time,
    /// as [`swr_v1_socket_iov_owned`].
    fn io(
        &mut self,
        plain: Plain<'_>,
        tls: Option<&(
            impl Fn(&mut Spare<'_>) -> Option<usize>,
            impl Fn(Drain<'_>) -> Option<usize>,
        )>,
    ) -> Option<usize>;

    /// `Socket::readv(io_vector)` or `writev` on `socket`, in the direction the vector
    /// was made for: readv(2) or writev(2) over the iovecs left, or OpenSSL one buffer
    /// at a time when the socket holds an SSL (upstream's `ssl_readv` and `ssl_writev`),
    /// advancing the vector past what moved.
    fn transfer(&mut self, socket: Socket<'_>) -> Option<usize> {
        let (plain, ssl) = socket.read(|record| (record.plain(), record.ssl()));
        let calls = (
            |spare: &mut Spare<'_>| ssl_recv_spare(socket, spare),
            |data: Drain<'_>| ssl_send_drain(socket, data),
        );
        self.io(plain, (!ssl.is_null()).then_some(&calls))
    }
}

impl Vectored for IoVector<'_> {
    fn write(&self) -> bool {
        self.owner.write()
    }

    fn remaining(&self) -> usize {
        usize::try_from(swr_v1_reactor_iov_view(self.owner()).remain_count).unwrap_or(0)
    }

    fn io(
        &mut self,
        plain: Plain<'_>,
        tls: Option<&(
            impl Fn(&mut Spare<'_>) -> Option<usize>,
            impl Fn(Drain<'_>) -> Option<usize>,
        )>,
    ) -> Option<usize> {
        swr_v1_socket_iov_owned(plain, &mut self.owner, tls)
    }
}

impl Vectored for SpareVectors<'_> {
    fn write(&self) -> bool {
        false
    }

    fn remaining(&self) -> usize {
        usize::try_from(self.state().remain_count).unwrap_or(0)
    }

    fn io(
        &mut self,
        plain: Plain<'_>,
        tls: Option<&(
            impl Fn(&mut Spare<'_>) -> Option<usize>,
            impl Fn(Drain<'_>) -> Option<usize>,
        )>,
    ) -> Option<usize> {
        self.transfer(plain, tls)
    }
}

struct Transfer<'a, V, T, R> {
    socket: Co,
    vector: &'a mut V,
    transfer: T,
    remaining: R,
    result: Option<usize>,
    total: usize,
}

impl<V, T: FnMut(&mut V) -> Option<usize>, R: FnMut(&V) -> usize> Transfer<'_, V, T, R> {
    fn transfer(&mut self) {
        self.result = (self.transfer)(self.vector);
        if let Some(n) = self.result {
            self.total = self.total.wrapping_add(n);
        }
    }

    fn remaining(&mut self) -> usize {
        (self.remaining)(self.vector)
    }

    fn would_block(&self) -> bool {
        would_block(self.socket)
    }
}

/// `readv_all`/`writev_all` in `event`'s direction, timed by the socket's read or
/// write timeout: the bytes transferred, the first call's result when it finished
/// or failed, or `None` when the socket is not available. `transfer` is one
/// `network::Socket` call over `vector` and `remaining` its remaining count.
pub(crate) fn vector_all<V>(
    socket: Co,
    event: Events,
    vector: &mut V,
    transfer: impl FnMut(&mut V) -> Option<usize>,
    remaining: impl FnMut(&V) -> usize,
) -> Option<usize> {
    if !available(socket, event) {
        return None;
    }
    let timeout = socket.timeout(crate::transfer::timeout_kind(event));
    let mut timer = Timer::new(socket, event, timeout);
    let mut state = Transfer {
        socket,
        vector,
        transfer,
        remaining,
        result: Some(0),
        total: 0,
    };
    state.transfer();
    let first = state.result;
    let result = if first.is_none() && !state.would_block() {
        set_err(socket, errno());
        first
    // The iov must not be touched again once complete.
    } else if first == Some(0) || state.remaining() == 0 {
        first
    } else {
        // The retry: transfer until the vector is done or a call returns
        // <= 0; keep waiting only when the last call would block.
        let mut retry = || {
            loop {
                state.transfer();
                if state.result <= Some(0) || state.remaining() == 0 {
                    break;
                }
            }
            state.result.is_none() && state.would_block()
        };
        if timer.start() && wait_retry(socket, event, &mut retry) {
            check(socket, state.result);
        }
        Some(state.total)
    };
    timer.stop();
    result
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::co_io::set_errno;
    use crate::co_io::test_host::{ON_SUSPEND, TIMER_OK, install, log, take};
    use crate::co_socket::TestSocket;
    use crate::socket_wait::ready;
    use std::cell::RefCell;
    use std::collections::VecDeque;
    use swoole_rs_corebase::CCast;

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
            let mut amount = result.c_cast::<usize>().min(left);
            let moved = amount;
            while self.index < self.entries.len()
                && amount >= self.entries[self.index] - self.offset
            {
                amount -= self.entries[self.index] - self.offset;
                self.index += 1;
                self.offset = 0;
            }
            self.offset += amount;
            (moved.c_cast::<isize>(), 0)
        }
    }

    /// How a scenario fails besides its results: 1 the socket is closed, 2 the
    /// timer can't be added, 3 the wait wakes with an error. Returns the result,
    /// the socket's error, the effects in order and the vector's remaining count.
    fn run(
        event: Events,
        layout: &[usize],
        results: &[isize],
        failure: u32,
    ) -> (isize, i32, Vec<String>, usize) {
        install();
        let mut socket = TestSocket::new();
        unsafe { socket.0.record() }.timeouts.read = 1.0;
        unsafe { socket.0.record() }.timeouts.write = 1.0;
        socket.0.err_code.set(9);
        if failure == 1 {
            socket.0.set_test_fd(-1);
        }
        TIMER_OK.set(failure != 2);
        ON_SUSPEND.set(Some(Box::new(move |socket: Co, event| {
            assert_eq!(ready(socket, event, false), Some(event));
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
            usize::try_from(result).ok()
        };
        let remaining = || {
            let mut effects = Vec::new();
            let count = vector.borrow().remaining(&mut effects);
            effects.into_iter().for_each(log);
            count
        };
        let result = vector_all(
            socket.co(),
            event,
            &mut (),
            |()| transfer(),
            |()| remaining(),
        )
        .map_or(-1, usize::cast_signed);
        let error = socket.0.err_code.get();
        socket.0.set_test_fd(100);
        ON_SUSPEND.set(None);
        let effects = take()
            .into_iter()
            .filter(|e| !e.starts_with("last-error"))
            .collect();
        (result, error, effects, vector.borrow().count())
    }

    /// Upstream's `readv_all`/`writev_all`, transcribed.
    fn upstream(
        event: Events,
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
            // Upstream's barrier retries from the ready callback and holds the
            // coroutine; here the woken coroutine retries and suspends again.
            effects.push(format!("yield:{event}"));
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
        for event in [Events::READ, Events::WRITE] {
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
        let (result, error, effects, left) = run(Events::READ, &[8], &[8], 0);
        assert_eq!((result, error, left), (8, 9, 0));
        assert_eq!(effects, ["io", "vectors:0"]);
    }

    #[test]
    fn partial_transfers_wait_and_accumulate() {
        let (result, error, effects, left) = run(Events::WRITE, &[2, 3, 3], &[2, EAGAIN, 3, 3], 0);
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
                "yield:1024",
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
