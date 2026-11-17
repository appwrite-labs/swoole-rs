//! The coroutine socket's transfers as whole operations over its record: one call
//! retried while the socket would block (`recv`, `send`, `read`, `write`, `readv`,
//! `writev`, `recvmsg`, `sendmsg`, `recvfrom`, `sendto`), and the all-or-nothing
//! transfer of `recv_all`/`send_all`, each with its availability check, timer and
//! waits. Native code passes only the `network::Socket` (or syscall) call.
use crate::co_io::{Timer, available, check, errno, set_err, would_block};
use crate::co_socket::Co;
use crate::socket_wait::{WriteSlot, wait, wait_retry};
use swoole_rs_corebase::net_types::SocketAddress;
use swoole_rs_corebase::reactor::Events;

/// Where a transfer's call moves its bytes: `skip` bytes into the caller's buffer
/// ([`At::Caller`]), or the socket's write buffer once a write's wait moved them
/// there.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum At {
    Caller(usize),
    WriteBuffer,
}

/// The transfer of the bytes [`At`] the buffer: the bytes moved, or `None` with errno
/// set. A native call must not throw.
pub type Call<'a> = &'a mut dyn FnMut(At) -> Option<usize>;

/// A transfer's length and direction, and a write's bytes, which its wait may move to
/// the write buffer. The call lends the buffer itself.
#[derive(Clone, Copy)]
pub struct Buffer<'a> {
    length: usize,
    event: Events,
    /// A write's bytes, none for a read.
    written: &'a [u8],
}

impl<'a> Buffer<'a> {
    /// A read of `length` slots.
    #[must_use]
    pub const fn read(length: usize) -> Self {
        Self {
            length,
            event: Events::READ,
            written: &[],
        }
    }

    /// The caller's bytes, which a write only reads (its wait may move them to the
    /// write buffer, a copy).
    #[must_use]
    pub const fn write(buf: &'a [u8]) -> Self {
        Self {
            length: buf.len(),
            event: Events::WRITE,
            written: buf,
        }
    }

    /// No buffer, in `event`'s direction.
    #[must_use]
    pub const fn none(event: Events) -> Self {
        Self {
            length: 0,
            event,
            written: &[],
        }
    }
}

/// `recv_all`/`send_all`: transfer every byte.
pub const ALL: u32 = 1;
/// Retry at once when the call fails with `EINTR` (the datagram calls).
pub const RETRY_INTERRUPTED: u32 = 2;
/// The caller checked availability already (`sendto`, after resolving).
pub const CHECKED: u32 = 4;

struct Op<'a> {
    socket: Co,
    call: Call<'a>,
}

impl Op<'_> {
    fn call(&mut self, at: At) -> Option<usize> {
        (self.call)(at)
    }

    fn would_block(&self) -> bool {
        would_block(self.socket)
    }
}

/// `recv_all`/`send_all`: `length` bytes, or fewer at EOF, error or timeout.
/// Returns the bytes transferred, `None` when none were, or the first call's result
/// when it finished at once or failed outright.
fn all(mut op: Op<'_>, timer: &mut Timer, event: Events, length: usize) -> Option<usize> {
    let first = op.call(At::Caller(0));
    if first == Some(0) || first == Some(length) {
        return first;
    }
    if first.is_none() && !op.would_block() {
        set_err(op.socket, errno());
        return None;
    }
    let socket = op.socket;
    let mut total = first.unwrap_or(0);
    let mut result = None;
    // The retry: transfer the rest; keep waiting while it would block or
    // is still short.
    let mut rest = || {
        result = op.call(At::Caller(total));
        match result {
            None => op.would_block(),
            Some(0) => false,
            Some(n) => {
                total += n;
                total < length
            }
        }
    };
    if timer.start() && wait_retry(socket, event, &mut rest) {
        check(socket, result);
    }
    (result.is_some() || total > 0).then_some(total)
}

/// One transfer, retried while the socket would block: start the timer, wait,
/// call again. A write's wait may move the bytes to the write buffer and the
/// retries send from there (upstream hands its buffer only to `send`, `write`
/// and `sendto`, which pass one). Returns the last result after
/// `check_return_value`.
fn one(
    mut op: Op<'_>,
    timer: &mut Timer,
    event: Events,
    flags: u32,
    written: &[u8],
) -> Option<usize> {
    let mut moved = false;
    let at = |moved| {
        if moved {
            At::WriteBuffer
        } else {
            At::Caller(0)
        }
    };
    let moves = event == Events::WRITE && !written.is_empty();
    let mut result = op.call(at(moved));
    while result.is_none() {
        if flags & RETRY_INTERRUPTED != 0 && errno() == libc::EINTR {
            result = op.call(at(moved));
            continue;
        }
        if !op.would_block() || !timer.start() {
            break;
        }
        let slot = moves.then(|| WriteSlot::new(&mut moved, written));
        if !wait(op.socket, event, slot) {
            break;
        }
        result = op.call(at(moved));
    }
    check(op.socket, result);
    result
}

/// A transfer of `buffer` in its direction, timed by the socket's read or write
/// timeout; `flags` are [`ALL`], [`RETRY_INTERRUPTED`] and [`CHECKED`]. `call`
/// moves the bytes [`At`] the buffer, or at the write buffer a write's wait moved
/// them to. `None` when the socket is not available.
pub(crate) fn transfer(
    socket: Co,
    flags: u32,
    buffer: Buffer<'_>,
    call: Call<'_>,
) -> Option<usize> {
    let Buffer {
        length,
        event,
        written,
    } = buffer;
    if flags & CHECKED == 0 && !available(socket, event) {
        return None;
    }
    let event = if event == Events::READ {
        Events::READ
    } else {
        Events::WRITE
    };
    let mut timer = Timer::new(socket, event, socket.timeout(timeout_kind(event)));
    let op = Op { socket, call };
    let result = if flags & ALL != 0 {
        all(op, &mut timer, event, length)
    } else {
        one(op, &mut timer, event, flags, written)
    };
    timer.stop();
    result
}

/// `Socket::recvfrom(buffer, length)`: `call` receives into the `network::Socket`'s
/// peer address, whose path a local socket's unnamed peer leaves empty.
pub(crate) fn recvfrom_peer(socket: Co, call: impl FnOnce() -> Option<usize>) -> Option<usize> {
    if !available(socket, Events::READ) {
        return None;
    }
    let local = socket.net(|network| {
        network.info.len = SocketAddress::CAPACITY;
        network.is_local()
    })?;
    let result = call();
    if result.is_some() && local {
        socket.net(|network| {
            let info = &mut network.info;
            if info.len as usize <= core::mem::offset_of!(libc::sockaddr_un, sun_path) {
                info.addr.edit_unix(|un| un.sun_path[0] = 0);
            }
        });
    }
    result
}

/// `SW_TIMEOUT_READ` for a read, else `SW_TIMEOUT_WRITE`.
pub(crate) fn timeout_kind(event: Events) -> swoole_rs_corebase::reactor::TimeoutKind {
    if event == Events::READ {
        swoole_rs_corebase::reactor::TimeoutKind::READ
    } else {
        swoole_rs_corebase::reactor::TimeoutKind::WRITE
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::co_io::set_errno;
    use crate::co_io::test_host::{MOVES, ON_SUSPEND, REACTOR_OK, TIMER_OK, install, log, take};
    use crate::co_socket::TestSocket;
    use crate::socket_wait::ready;
    use std::cell::RefCell;
    use std::collections::VecDeque;
    use swoole_rs_corebase::CCast;

    thread_local! {
        /// Scripted call results: bytes moved, or -errno.
        static RESULTS: RefCell<VecDeque<isize>> = const { RefCell::new(VecDeque::new()) };
    }

    /// The scripted call over the `length` bytes of a buffer `buffered` lends, or of
    /// none, at `at`: it logs which source it moves, the caller's (`true`), none
    /// (`false`), or the write buffer a wait moved them to.
    fn call(length: usize, buffered: bool, at: At) -> Option<usize> {
        let (length, source) = match at {
            At::Caller(skip) => (length - skip, buffered.to_string()),
            At::WriteBuffer => (length, "write-buffer".to_owned()),
        };
        let result = RESULTS.with_borrow_mut(VecDeque::pop_front).unwrap_or(0);
        log(format!("io:{length}:{source}"));
        if result < 0 {
            set_errno(i32::try_from(-result).unwrap());
            return None;
        }
        set_errno(0);
        Some(result.unsigned_abs().min(length))
    }

    const EAGAIN: isize = -(libc::EAGAIN as isize);
    const EINTR: isize = -(libc::EINTR as isize);
    const EINVAL: isize = -(libc::EINVAL as isize);
    const ECONNRESET: isize = -(libc::ECONNRESET as isize);

    /// How a scenario fails besides its results: 1 the socket is closed, 2 the
    /// timer can't be added, 3 the wait wakes with an error. `moves`: a write's wait
    /// moves its bytes to the write buffer.
    fn run(
        event: Events,
        flags: u32,
        buffered: bool,
        moves: bool,
        results: &[isize],
        failure: u32,
    ) -> (isize, i32, Vec<String>) {
        install();
        MOVES.set(moves);
        let mut socket = TestSocket::new();
        unsafe { socket.0.record() }.timeouts.read = 1.0;
        unsafe { socket.0.record() }.timeouts.write = 1.0;
        RESULTS.set(results.iter().copied().collect());
        if failure == 1 {
            socket.0.sock_fd.set(-1);
        }
        TIMER_OK.set(failure != 2);
        // The ready callback wakes each wait.
        ON_SUSPEND.set(Some(Box::new(move |socket: Co, event| {
            assert_eq!(ready(socket, event, false), Some(event));
            if failure == 3 {
                socket.set_err_code(libc::ETIMEDOUT);
            }
        })));
        let buffer = [0u8; 8];
        let written = if buffered && event == Events::WRITE {
            &buffer[..]
        } else {
            &[]
        };
        let buffer = Buffer {
            length: 8,
            event,
            written,
        };
        let result = transfer(socket.co(), flags, buffer, &mut |at| call(8, buffered, at));
        let result = result.map_or(-1, usize::cast_signed);
        let error = socket.0.err_code.get();
        socket.0.sock_fd.set(100);
        let events = take()
            .into_iter()
            .filter(|e| !e.starts_with("last-error"))
            .collect();
        (result, error, events)
    }

    /// Upstream's one-call loop (`recv`, `send`, `read`, `write`, `readv`,
    /// `writev`, `recvmsg`, `sendmsg`, `recvfrom`, `sendto`), transcribed: the
    /// result, the error it leaves and the native effects in order. Once a write's
    /// wait moves the bytes (`moves`), the retries send from the write buffer.
    fn upstream_one(
        event: Events,
        eintr: bool,
        buffered: bool,
        moves: bool,
        results: &[isize],
        failure: u32,
    ) -> (isize, i32, Vec<String>) {
        let mut effects = Vec::new();
        if failure == 1 {
            return (-1, libc::EBADF, effects);
        }
        let mut results: VecDeque<isize> = results.iter().copied().collect();
        let mut call = |effects: &mut Vec<String>, moved: bool| {
            let source = if moved {
                "write-buffer".to_owned()
            } else {
                buffered.to_string()
            };
            effects.push(format!("io:8:{source}"));
            results.pop_front().unwrap_or(0)
        };
        let (mut timer, mut error, mut moved) = (false, 0, false);
        let mut result = call(&mut effects, moved);
        while result < 0 {
            if eintr && result == EINTR {
                result = call(&mut effects, moved);
                continue;
            }
            if result != EAGAIN {
                break;
            }
            if !timer {
                effects.push("timer:add:1".into());
                if failure == 2 {
                    break;
                }
                timer = true;
            }
            effects.push(format!("add:{event}"));
            if event == Events::WRITE && buffered {
                effects.push("buffer:8".into());
                moved = moves;
            }
            effects.push(format!("yield:{event}"));
            effects.push(format!("remove:{event}"));
            if failure == 3 {
                error = libc::ETIMEDOUT;
                break;
            }
            result = call(&mut effects, moved);
        }
        if result >= 0 {
            error = 0;
        } else if error == 0 {
            error = i32::try_from(-result).unwrap();
        }
        if timer {
            effects.push("timer:delete".into());
        }
        (result.max(-1), error, effects)
    }

    /// The ordered gate's former scenarios 1170..1434 (the stream calls) and
    /// 1440..1872 (the message and datagram calls, which retry `EINTR` at once).
    #[test]
    fn ported_transfer_matrix() {
        let modes: [&[isize]; 15] = [
            &[0],
            &[8],
            &[EINVAL],
            &[EINTR],
            &[EAGAIN, 8],
            &[EAGAIN, EAGAIN, 3],
            &[EAGAIN, 0],
            &[EAGAIN, ECONNRESET],
            &[EAGAIN, EINTR],
            &[EAGAIN, EAGAIN, 0],
            &[EAGAIN, EAGAIN, EINVAL],
            &[EINTR, 8],
            &[EINTR, EINTR, 0],
            &[EINTR, EAGAIN, 3],
            &[EINTR, EAGAIN, EAGAIN, 8],
        ];
        for (event, buffered) in [
            (Events::READ, true),
            (Events::WRITE, true),
            (Events::READ, false),
            (Events::WRITE, false),
        ] {
            for (eintr, moves) in [(false, false), (true, false), (false, true), (true, true)] {
                for results in modes {
                    for failure in 0..4 {
                        let flags = if eintr { RETRY_INTERRUPTED } else { 0 };
                        assert_eq!(
                            run(event, flags, buffered, moves, results, failure),
                            upstream_one(event, eintr, buffered, moves, results, failure),
                            "{event} {eintr} {buffered} {moves} {results:?} {failure}"
                        );
                    }
                }
            }
        }
    }

    /// Upstream's `recv_all`/`send_all`, transcribed.
    fn upstream_all(event: Events, results: &[isize], failure: u32) -> (isize, i32, Vec<String>) {
        let mut effects = Vec::new();
        if failure == 1 {
            return (-1, libc::EBADF, effects);
        }
        let mut results: VecDeque<isize> = results.iter().copied().collect();
        let mut call = |effects: &mut Vec<String>, length: usize| {
            effects.push(format!("io:{length}:true"));
            results
                .pop_front()
                .unwrap_or(0)
                .min(length.c_cast::<isize>())
        };
        let first = call(&mut effects, 8);
        if first == 0 || first == 8 {
            return (first, 0, effects);
        }
        if first < 0 && first != EAGAIN {
            return (-1, i32::try_from(-first).unwrap(), effects);
        }
        let mut total = first.max(0).c_cast::<usize>();
        let mut result = -1;
        let mut error = 0;
        effects.push("timer:add:1".into());
        if failure != 2 {
            effects.push(format!("add:{event}"));
            effects.push(format!("yield:{event}"));
            loop {
                result = call(&mut effects, 8 - total);
                if result > 0 {
                    total += result.c_cast::<usize>();
                }
                if !(result == EAGAIN || result > 0 && total < 8) {
                    break;
                }
                // Upstream's barrier retries from the ready callback and holds the
                // coroutine; here the woken coroutine retries and suspends again.
                effects.push(format!("yield:{event}"));
            }
            effects.push(format!("remove:{event}"));
            error = if failure == 3 {
                libc::ETIMEDOUT
            } else if result >= 0 {
                0
            } else {
                i32::try_from(-result).unwrap()
            };
            effects.push("timer:delete".into());
        }
        let total = if result < 0 && total == 0 {
            -1
        } else {
            total.c_cast::<isize>()
        };
        (total, error, effects)
    }

    /// The ordered gate's former scenarios 28..50.
    #[test]
    fn ported_transfer_all_matrix() {
        let cases: [&[isize]; 8] = [
            &[0],
            &[8],
            &[EINVAL],
            &[EAGAIN, 3, 5],
            &[2, 3, 3],
            &[2, EAGAIN, ECONNRESET],
            &[2, 0],
            &[EAGAIN, 0],
        ];
        for event in [Events::READ, Events::WRITE] {
            for results in cases {
                for failure in 0..4 {
                    assert_eq!(
                        run(event, ALL, true, true, results, failure),
                        upstream_all(event, results, failure),
                        "{event} {results:?} {failure}"
                    );
                }
            }
        }
    }

    #[test]
    fn a_failed_reactor_add_stops_the_wait() {
        install();
        REACTOR_OK.set(false);
        let mut socket = TestSocket::new();
        unsafe { socket.0.record() }.timeouts.read = 0.0;
        RESULTS.set([EAGAIN].into_iter().collect());
        let buffer = Buffer::read(8);
        let result = transfer(socket.co(), CHECKED, buffer, &mut |at| call(8, false, at));
        assert_eq!(result, None);
        // A zero timeout starts no timer.
        assert!(!take().iter().any(|e| e.starts_with("timer")));
    }

    /// A scripted recvfrom into the socket's peer address, which logs the length it
    /// was given and receives an address of 2 bytes.
    fn peer(socket: Co) -> Option<usize> {
        let result = RESULTS.with_borrow_mut(VecDeque::pop_front).unwrap_or(0);
        socket.net(|network| {
            log(format!("recvfrom:{}", network.info.len));
            network.info.addr.edit_unix(|un| un.sun_path[0] = 1);
            network.info.len = 2;
        });
        usize::try_from(result).ok()
    }

    // Upstream's recvfrom(buf, n): is_available, `info.len = sizeof(info.addr)`,
    // the virtual recvfrom, and an empty path for a local socket's unnamed peer.
    #[test]
    fn recvfrom_fills_the_sockets_peer_and_clears_a_local_unnamed_path() {
        let size = size_of::<swoole_rs_corebase::net_types::AddressStorage>();
        for (socket_type, result, cleared) in [(6, 4, true), (6, -1, false), (1, 4, false)] {
            install();
            let mut socket = TestSocket::new();
            unsafe { socket.0.record() }.socket_type = socket_type;
            RESULTS.set([result].into());
            let s = socket.co();
            let got = recvfrom_peer(s, || peer(s));
            assert_eq!(got.map_or(-1, usize::cast_signed), result);
            assert_eq!(take(), [format!("recvfrom:{size}")]);
            let info = unsafe { socket.0.record() }.info;
            assert_eq!(
                unsafe { info.addr.un.sun_path[0] },
                libc::c_char::from(!cleared)
            );
        }
        install();
        let mut socket = TestSocket::new();
        socket.0.sock_fd.set(-1);
        let s = socket.co();
        assert_eq!(recvfrom_peer(s, || peer(s)), None);
        assert!(!take().iter().any(|e| e.starts_with("recvfrom")));
        socket.0.sock_fd.set(100);
    }
}
