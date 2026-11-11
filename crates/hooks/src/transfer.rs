//! The coroutine socket's transfers as whole operations over its record: one call
//! retried while the socket would block (`recv`, `send`, `read`, `write`, `readv`,
//! `writev`, `recvmsg`, `sendmsg`, `recvfrom`, `sendto`), and the all-or-nothing
//! transfer of `recv_all`/`send_all`, each with its availability check, timer and
//! waits. Native code passes only the `network::Socket` (or syscall) call.
use crate::co_io::{Timer, available, check, errno, set_err, would_block};
use crate::co_socket::Co;
use crate::shutdown::{READ, WRITE};
use crate::socket_wait::{wait, wait_barrier};
use core::marker::PhantomData;
use swoole_rs_corebase::net_types::AddressStorage;
use swoole_rs_corebase::sys::ReadSlot;
use swoole_rs_reactor::Socket;

/// The transfer of `length` bytes at `buffer`; sets errno when it fails. A native
/// call must not throw.
pub type Call<'a> = &'a mut dyn FnMut(*mut u8, usize) -> isize;

/// A transfer's buffer and direction: the caller's slots a read fills, the bytes a
/// write sends, or none for the vector and message calls, which carry their own.
pub struct Buffer<'a> {
    /// Null, or `length` bytes the borrow keeps live: writable for a read,
    /// readable for a write.
    at: *mut u8,
    length: usize,
    event: u32,
    borrow: PhantomData<&'a mut [u8]>,
}

impl<'a> Buffer<'a> {
    /// The caller's one-byte slots, any value valid, which a read fills.
    pub fn read(buf: &'a mut [impl ReadSlot]) -> Self {
        let (at, length) = (buf.as_mut_ptr().cast(), buf.len());
        Self::new(at, length, READ)
    }

    /// The caller's bytes, which a write only reads (its wait may move them to the
    /// write buffer, a copy).
    #[must_use]
    pub fn write(buf: &'a [u8]) -> Self {
        Self::new(buf.as_ptr().cast_mut(), buf.len(), WRITE)
    }

    /// No buffer, in `event`'s direction.
    #[must_use]
    pub const fn none(event: u32) -> Self {
        Self::new(core::ptr::null_mut(), 0, event)
    }

    /// The buffer's start, length and direction; its borrow outlives the transfer.
    const fn into_parts(self) -> (*mut u8, usize, u32) {
        (self.at, self.length, self.event)
    }

    const fn new(at: *mut u8, length: usize, event: u32) -> Self {
        Self {
            at,
            length,
            event,
            borrow: PhantomData,
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
    fn call(&mut self, buffer: *mut u8, length: usize) -> isize {
        (self.call)(buffer, length)
    }

    fn would_block(&self) -> bool {
        would_block(self.socket)
    }
}

/// `recv_all`/`send_all`: `length` bytes, or fewer at EOF, error or timeout.
/// Returns the bytes transferred, -1 when none were, or the first call's result
/// when it finished at once or failed outright.
fn all(mut op: Op<'_>, timer: &mut Timer, event: u32, buffer: *mut u8, length: usize) -> isize {
    let first = op.call(buffer, length);
    if first == 0 || first > 0 && first.unsigned_abs() == length {
        return first;
    }
    if first < 0 && !op.would_block() {
        set_err(op.socket, errno());
        return first;
    }
    let socket = op.socket;
    let mut total = if first > 0 { first.unsigned_abs() } else { 0 };
    let mut result = -1;
    // The barrier's retry: transfer the rest; keep waiting while it would block or
    // is still short.
    let mut rest = || {
        result = op.call(buffer.wrapping_add(total), length - total);
        if result > 0 {
            total += result.unsigned_abs();
        }
        result < 0 && op.would_block() || result > 0 && total < length
    };
    if timer.start() && wait_barrier(socket, event, &mut rest) {
        check(socket, result);
    }
    if result < 0 && total == 0 {
        -1
    } else {
        total.cast_signed()
    }
}

/// One transfer, retried while the socket would block: start the timer, wait,
/// call again. A write's wait may move the bytes to the write buffer and the
/// retries send from there (upstream hands its buffer only to `send`, `write`
/// and `sendto`, which pass one). Returns the last result after
/// `check_return_value`.
fn one(
    mut op: Op<'_>,
    timer: &mut Timer,
    event: u32,
    flags: u32,
    buffer: *mut u8,
    length: usize,
) -> isize {
    let mut buffer = buffer;
    let slot = if event == WRITE && !buffer.is_null() {
        &raw mut buffer
    } else {
        core::ptr::null_mut()
    };
    let mut result = op.call(buffer, length);
    while result < 0 {
        if flags & RETRY_INTERRUPTED != 0 && errno() == libc::EINTR {
            result = op.call(buffer, length);
            continue;
        }
        if !op.would_block() || !timer.start() {
            break;
        }
        // SAFETY: `slot` is null or a write's `buffer`, whose borrow keeps it live
        // across the wait.
        if !unsafe { wait(op.socket, event, slot, length) } {
            break;
        }
        result = op.call(buffer, length);
    }
    check(op.socket, result);
    result
}

/// A transfer of `buffer` in its direction, timed by the socket's read or write
/// timeout; `flags` are [`ALL`], [`RETRY_INTERRUPTED`] and [`CHECKED`]. `call`
/// gets the buffer, or the write buffer a write's wait moved it to. -1 when the
/// socket is not available.
pub(crate) fn transfer(socket: Co, flags: u32, buffer: Buffer<'_>, call: Call<'_>) -> isize {
    let (buffer, length, event) = buffer.into_parts();
    if flags & CHECKED == 0 && !available(socket, event) {
        return -1;
    }
    let event = if event == READ { READ } else { WRITE };
    let mut timer = Timer::new(socket, event, socket.timeout(timeout_kind(event)));
    let op = Op { socket, call };
    let result = if flags & ALL != 0 {
        all(op, &mut timer, event, buffer, length)
    } else {
        one(op, &mut timer, event, flags, buffer, length)
    };
    timer.stop();
    result
}

/// `Socket::recvfrom(buffer, length)`: `call(addr, addrlen)` receives into the
/// `network::Socket`'s peer address, whose path a local socket's unnamed peer leaves
/// empty.
pub(crate) fn recvfrom_peer(
    socket: Co,
    call: impl FnOnce(*mut libc::sockaddr, *mut libc::socklen_t) -> isize,
) -> isize {
    if !available(socket, READ) {
        return -1;
    }
    socket
        .with_network(|network| recvfrom_into(network, call))
        .unwrap_or(-1)
}

/// [`recvfrom_peer`] over the socket's handle.
fn recvfrom_into(
    network: Socket<'_>,
    call: impl FnOnce(*mut libc::sockaddr, *mut libc::socklen_t) -> isize,
) -> isize {
    // The pointers outlive the borrow: `call` fills them, and nothing else touches the
    // record until it returns.
    let (local, addr, addrlen) = network.edit(|network| {
        network.info.len = socklen(size_of::<AddressStorage>());
        (
            network.is_local(),
            &raw mut network.info.addr,
            &raw mut network.info.len,
        )
    });
    let result = call(addr.cast(), addrlen);
    if result >= 0 && local {
        network.edit(|network| {
            let info = &mut network.info;
            if info.len as usize <= core::mem::offset_of!(libc::sockaddr_un, sun_path) {
                info.addr.unix_mut().sun_path[0] = 0;
            }
        });
    }
    result
}

/// `SW_TIMEOUT_READ` for a read, else `SW_TIMEOUT_WRITE`.
pub(crate) fn timeout_kind(event: u32) -> core::ffi::c_int {
    if event == READ { 1 << 2 } else { 1 << 3 }
}

fn socklen(length: usize) -> libc::socklen_t {
    libc::socklen_t::try_from(length).unwrap_or(libc::socklen_t::MAX)
}

#[cfg(test)]
#[expect(
    clippy::cast_possible_wrap,
    clippy::cast_sign_loss,
    reason = "the test socket drives the native integer interface with small lengths"
)]
mod tests {
    use super::*;
    use crate::co_io::set_errno;
    use crate::co_io::test_host::{ON_SUSPEND, REACTOR_OK, TIMER_OK, install, log, take};
    use crate::co_socket::Co;
    use crate::co_socket::TestSocket;
    use crate::socket_wait::ready;
    use core::ffi::c_void;
    use std::cell::RefCell;
    use std::collections::VecDeque;

    thread_local! {
        /// Scripted call results: bytes moved, or -errno.
        static RESULTS: RefCell<VecDeque<isize>> = const { RefCell::new(VecDeque::new()) };
    }

    fn call(at: *mut u8, length: usize) -> isize {
        let result = RESULTS.with_borrow_mut(VecDeque::pop_front).unwrap_or(0);
        log(format!("io:{length}:{}", !at.is_null()));
        if result < 0 {
            set_errno(i32::try_from(-result).unwrap());
            return -1;
        }
        set_errno(0);
        result.min(length as isize)
    }

    const EAGAIN: isize = -(libc::EAGAIN as isize);
    const EINTR: isize = -(libc::EINTR as isize);
    const EINVAL: isize = -(libc::EINVAL as isize);
    const ECONNRESET: isize = -(libc::ECONNRESET as isize);

    /// How a scenario fails besides its results: 1 the socket is closed, 2 the
    /// timer can't be added, 3 the wait wakes with an error.
    fn run(
        event: u32,
        flags: u32,
        buffered: bool,
        results: &[isize],
        failure: u32,
    ) -> (isize, i32, Vec<String>) {
        install();
        let mut socket = TestSocket::new();
        unsafe { socket.0.record() }.timeouts.read = 1.0;
        unsafe { socket.0.record() }.timeouts.write = 1.0;
        RESULTS.set(results.iter().copied().collect());
        if failure == 1 {
            socket.0.sock_fd = -1;
        }
        TIMER_OK.set(failure != 2);
        // A woken wait runs the installed barrier while it keeps waiting.
        ON_SUSPEND.set(Some(Box::new(move |socket: Co, event| {
            while ready(socket, event, false).is_null() {
                log("again");
            }
            if failure == 3 {
                socket.set_err_code(libc::ETIMEDOUT);
            }
        })));
        let mut buffer = [0u8; 16];
        let at = if buffered {
            buffer.as_mut_ptr()
        } else {
            core::ptr::null_mut()
        };
        let result = transfer(socket.co(), flags, Buffer::new(at, 8, event), &mut call);
        let error = socket.0.err_code;
        socket.0.sock_fd = 100;
        let events = take()
            .into_iter()
            .filter(|e| !e.starts_with("last-error"))
            .collect();
        (result, error, events)
    }

    /// Upstream's one-call loop (`recv`, `send`, `read`, `write`, `readv`,
    /// `writev`, `recvmsg`, `sendmsg`, `recvfrom`, `sendto`), transcribed: the
    /// result, the error it leaves and the native effects in order.
    fn upstream_one(
        event: u32,
        eintr: bool,
        buffered: bool,
        results: &[isize],
        failure: u32,
    ) -> (isize, i32, Vec<String>) {
        let mut effects = Vec::new();
        if failure == 1 {
            return (-1, libc::EBADF, effects);
        }
        let mut results: VecDeque<isize> = results.iter().copied().collect();
        let mut call = |effects: &mut Vec<String>| {
            effects.push(format!("io:8:{buffered}"));
            results.pop_front().unwrap_or(0)
        };
        let (mut timer, mut error) = (false, 0);
        let mut result = call(&mut effects);
        while result < 0 {
            if eintr && result == EINTR {
                result = call(&mut effects);
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
            if event == WRITE && buffered {
                effects.push("buffer:8".into());
            }
            effects.push(format!("yield:{event}"));
            effects.push(format!("remove:{event}"));
            if failure == 3 {
                error = libc::ETIMEDOUT;
                break;
            }
            result = call(&mut effects);
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
        for (event, buffered) in [(READ, true), (WRITE, true), (READ, false), (WRITE, false)] {
            for eintr in [false, true] {
                for results in modes {
                    for failure in 0..4 {
                        let flags = if eintr { RETRY_INTERRUPTED } else { 0 };
                        assert_eq!(
                            run(event, flags, buffered, results, failure),
                            upstream_one(event, eintr, buffered, results, failure),
                            "{event} {eintr} {buffered} {results:?} {failure}"
                        );
                    }
                }
            }
        }
    }

    /// Upstream's `recv_all`/`send_all`, transcribed.
    fn upstream_all(event: u32, results: &[isize], failure: u32) -> (isize, i32, Vec<String>) {
        let mut effects = Vec::new();
        if failure == 1 {
            return (-1, libc::EBADF, effects);
        }
        let mut results: VecDeque<isize> = results.iter().copied().collect();
        let mut call = |effects: &mut Vec<String>, length: usize| {
            effects.push(format!("io:{length}:true"));
            results.pop_front().unwrap_or(0).min(length as isize)
        };
        let first = call(&mut effects, 8);
        if first == 0 || first == 8 {
            return (first, 0, effects);
        }
        if first < 0 && first != EAGAIN {
            return (-1, i32::try_from(-first).unwrap(), effects);
        }
        let mut total = first.max(0) as usize;
        let mut result = -1;
        let mut error = 0;
        effects.push("timer:add:1".into());
        if failure != 2 {
            effects.push(format!("add:{event}"));
            effects.push(format!("yield:{event}"));
            loop {
                result = call(&mut effects, 8 - total);
                if result > 0 {
                    total += result as usize;
                }
                if !(result == EAGAIN || result > 0 && total < 8) {
                    break;
                }
                effects.push("again".into());
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
            total as isize
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
        for event in [READ, WRITE] {
            for results in cases {
                for failure in 0..4 {
                    assert_eq!(
                        run(event, ALL, true, results, failure),
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
        let buffer = Buffer::new(core::ptr::null_mut(), 8, READ);
        let result = transfer(socket.co(), CHECKED, buffer, &mut call);
        assert_eq!(result, -1);
        // A zero timeout starts no timer.
        assert!(!take().iter().any(|e| e.starts_with("timer")));
    }

    unsafe extern "C" fn peer(
        socket: Co,
        _: *mut c_void,
        _: usize,
        addr: *mut libc::sockaddr,
        length: *mut libc::socklen_t,
    ) -> isize {
        let result = RESULTS.with_borrow_mut(VecDeque::pop_front).unwrap_or(0);
        let expected = unsafe { &raw mut (*socket.socket()).fields.info };
        log(format!(
            "recvfrom:{}:{}",
            addr == unsafe { &raw mut (*expected).addr.ss },
            unsafe { *length }
        ));
        unsafe { (*addr.cast::<libc::sockaddr_un>()).sun_path[0] = 1 };
        unsafe { *length = 2 };
        result
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
            let got = unsafe {
                let s = socket.co();
                recvfrom_peer(s, |addr, len| peer(s, core::ptr::null_mut(), 8, addr, len))
            };
            assert_eq!(got, result);
            assert_eq!(take(), [format!("recvfrom:true:{size}")]);
            let info = unsafe { socket.0.record() }.info;
            assert_eq!(unsafe { info.addr.un.sun_path[0] }, i8::from(!cleared) as _);
        }
        install();
        let mut socket = TestSocket::new();
        socket.0.sock_fd = -1;
        assert_eq!(
            unsafe {
                let s = socket.co();
                recvfrom_peer(s, |addr, len| peer(s, core::ptr::null_mut(), 8, addr, len))
            },
            -1
        );
        assert!(!take().iter().any(|e| e.starts_with("recvfrom")));
        socket.0.sock_fd = 100;
    }
}
