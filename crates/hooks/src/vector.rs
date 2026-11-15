//! The coroutine socket's `network::IOVector` ([`IoVector`]), and its
//! `Socket::readv_all` and `Socket::writev_all`: transfer a whole vector, waiting
//! for the socket while it would block. The member itself, [`Co::vector`], is in
//! `socket_members.rs`; it passes the `network::Socket` call and the vector's
//! remaining count.
use crate::co_io::{Timer, available, check, errno, set_err, would_block};
use crate::co_socket::Co;
use crate::socket_wait::wait_barrier;
use core::ffi::c_int;
use core::marker::PhantomData;
use core::mem::MaybeUninit;
use std::io::{IoSlice, IoSliceMut};
use swoole_rs_corebase::sys::ReadSlot;
use swoole_rs_reactor::{
    Drain, Fill, ReactorIov, Socket, swr_v1_reactor_iov_view, swr_v1_socket_iov_owned,
};
use swoole_rs_tls::upstream::{ssl_recv_fill, ssl_send_drain};

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
    pub(crate) fn send(&self, fd: c_int, flags: c_int) -> isize {
        sendmsg(fd, self, flags)
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
    pub(crate) fn recv(&mut self, fd: c_int, flags: c_int) -> isize {
        recvmsg(fd, self, flags)
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

/// One buffer a vectored read fills, as an `iovec`: slots that may not be written
/// yet, lent for `'b`.
#[repr(transparent)]
pub struct ReadVec<'b> {
    iov: libc::iovec,
    lent: PhantomData<&'b mut [MaybeUninit<u8>]>,
}

impl<'b> ReadVec<'b> {
    /// The vector over `slots`.
    #[must_use]
    pub fn new(slots: &'b mut [impl ReadSlot]) -> Self {
        Self {
            iov: libc::iovec {
                iov_base: slots.as_mut_ptr().cast(),
                iov_len: slots.len(),
            },
            lent: PhantomData,
        }
    }
}

/// A `network::IOVector` over a copy of the caller's iovecs, destroyed on drop, and
/// the direction it was made for. The buffers the iovecs name are borrowed for `'a`
/// as [`Vectors`] lent them: writable and reached through nothing else for a read,
/// readable for a write.
#[repr(C)]
pub struct IoVector<'a> {
    owner: Box<ReactorIov>,
    write: bool,
    lent: PhantomData<Vectors<'a, 'a>>,
}

/// What is left of one of an [`IoVector`]'s buffers, lent in the vector's direction:
/// slots to read into, or bytes to write.
#[cfg(feature = "iouring")]
pub(crate) enum Lent<'a> {
    Read(&'a mut [core::mem::MaybeUninit<u8>]),
    Write(&'a [u8]),
}

impl<'a> IoVector<'a> {
    /// A vector over a copy of `vectors`' iovecs, to read into or to write from as
    /// they were lent; none when the copy fails, where `IOVector`'s constructor throws.
    #[must_use]
    pub fn new(vectors: Vectors<'a, '_>) -> Option<Self> {
        let (at, count, write) = match vectors {
            Vectors::Read(read) => (read.as_ptr().cast::<libc::iovec>(), read.len(), false),
            Vectors::Write(written) => (written.as_ptr().cast(), written.len(), true),
        };
        // SAFETY: `IoSlice` and `ReadVec` are ABI-compatible with `iovec`, so these
        // are the borrowed slice's `count` iovecs, which are only copied.
        let vectors = unsafe { core::slice::from_raw_parts(at, count) };
        let owner = ReactorIov::new(vectors)?;
        Some(Self {
            owner,
            write,
            lent: PhantomData,
        })
    }

    /// Whether the vector was made to write from.
    pub(crate) const fn write(&self) -> bool {
        self.write
    }

    fn owner(&self) -> &ReactorIov {
        &self.owner
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
        self.owner.advance(result);
    }

    /// `io` over each remaining buffer in order while it moves data, again over what a
    /// short transfer left of one. The bytes moved, or the failing result when none
    /// moved; the caller advances the vector by that, once ([`update`](Self::update)).
    #[cfg(feature = "iouring")]
    pub(crate) fn transfer_each(&mut self, mut io: impl FnMut(Lent<'_>) -> isize) -> isize {
        let write = self.write;
        let mut total = 0isize;
        let mut result = 0isize;
        'iovecs: for iov in self.iovecs() {
            let mut done = 0usize;
            while done < iov.iov_len {
                let (at, n) = (iov.iov_base.wrapping_byte_add(done), iov.iov_len - done);
                result = io(if write {
                    // SAFETY: the `n > 0` bytes left of a remaining iovec's buffer,
                    // readable while the vector lives (the type's invariant).
                    Lent::Write(unsafe { core::slice::from_raw_parts(at.cast(), n) })
                } else {
                    // SAFETY: the `n > 0` bytes left of a remaining iovec's buffer,
                    // writable and unreferenced while the vector lives (the type's
                    // invariant); lent for this call only, one buffer at a time.
                    Lent::Read(unsafe { core::slice::from_raw_parts_mut(at.cast(), n) })
                });
                if result <= 0 {
                    break 'iovecs;
                }
                total += result;
                done += result.unsigned_abs();
            }
        }
        if total > 0 { total } else { result }
    }

    /// `Socket::readv(io_vector)` or `writev` on `socket`, in the direction the vector
    /// was made for: readv(2) or writev(2) over the iovecs left, or OpenSSL one buffer
    /// at a time when the socket holds an SSL (upstream's `ssl_readv` and `ssl_writev`),
    /// advancing the vector past what moved.
    pub fn transfer(&mut self, socket: Socket<'_>) -> isize {
        let (fd, ssl) = socket.read(|record| (record.fd, record.ssl()));
        let calls = (
            |buffer: Fill<'_>| ssl_recv_fill(socket, buffer),
            |data: Drain<'_>| ssl_send_drain(socket, data),
        );
        let tls = (!ssl.is_null()).then_some(&calls);
        // SAFETY: buffers valid in the vector's direction while it lives (the type's
        // invariant).
        unsafe { swr_v1_socket_iov_owned(fd, self.write, &mut self.owner, tls) }
    }
}

/// Retries the transfer once the socket is ready; true to keep waiting. Installed
/// only for the wait that lends it ([`wait_barrier`]), which erases its lifetime.
pub type Barrier = *mut (dyn FnMut() -> bool + 'static);

struct Transfer<'a, V, T, R> {
    socket: Co,
    vector: &'a mut V,
    transfer: T,
    remaining: R,
    result: isize,
    total: isize,
}

impl<V, T: FnMut(&mut V) -> isize, R: FnMut(&V) -> usize> Transfer<'_, V, T, R> {
    fn transfer(&mut self) {
        self.result = (self.transfer)(self.vector);
        if self.result > 0 {
            self.total = self.total.wrapping_add(self.result);
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
/// or failed, or -1 when the socket is not available. `transfer` is one
/// `network::Socket` call over `vector` and `remaining` its remaining count.
pub(crate) fn vector_all<V>(
    socket: Co,
    event: u32,
    vector: &mut V,
    transfer: impl FnMut(&mut V) -> isize,
    remaining: impl FnMut(&V) -> usize,
) -> isize {
    if !available(socket, event) {
        return -1;
    }
    let timeout = socket.timeout(crate::transfer::timeout_kind(event));
    let mut timer = Timer::new(socket, event, timeout);
    let mut state = Transfer {
        socket,
        vector,
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
    } else if first == 0 || state.remaining() == 0 {
        first
    } else {
        // The barrier's retry: transfer until the vector is done or a call returns
        // <= 0; keep waiting only when the last call would block.
        let mut retry = || {
            loop {
                state.transfer();
                if state.result <= 0 || state.remaining() == 0 {
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
        event: u32,
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
            socket.0.sock_fd.set(-1);
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
        let result = vector_all(
            socket.co(),
            event,
            &mut (),
            |()| transfer(),
            |()| remaining(),
        );
        let error = socket.0.err_code.get();
        socket.0.sock_fd.set(100);
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
