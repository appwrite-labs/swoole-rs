//! `network::Socket`'s blocking calls as whole operations: `wait_for`'s loop
//! (prepare, retry classification, the ENOBUFS backoff, poll and timeout) and each
//! `*_sync` method's progress rule. Native code performs each I/O call, keeping its
//! counters and TLS, and reads the socket's bitfields, through [`SyncHooks`].
use std::os::raw::c_int;
use std::time::{Duration, Instant};

/// `swReturnCode`'s `SW_WAIT` (from `catch_error`).
const WAIT: c_int = 2;
/// `SW_ERROR_SOCKET_POLL_TIMEOUT`.
const POLL_TIMEOUT: c_int = 6002;

/// `send`: everything, retrying short writes.
pub const SYNC_SEND: c_int = 0;
/// `recv`: once, or everything under `MSG_WAITALL` (cleared for each call).
pub const SYNC_RECV: c_int = 1;
/// `read`/`write`/`writev`/`sendto`/`recvfrom`: one call that moves bytes.
pub const SYNC_ONCE: c_int = 2;
/// `sendfile`: until `length` bytes are sent; a zero return is an error.
pub const SYNC_SENDFILE: c_int = 3;

/// What a blocking call needs from its `Socket` (`crates/tls`'s `upstream.rs`).
pub trait SyncEffects {
    /// The socket's descriptor (asked for each wait, as upstream's `wait_event`
    /// reads it) and `dont_restart`.
    fn fd(&self) -> c_int;
    fn dont_restart(&self) -> bool;
    /// `set_nonblock` when blocking and `ssl_clear_error` when TLS.
    fn prepare(&self);
    /// `what_event_want(event)` as poll bits.
    fn events(&self, event: c_int) -> i16;
    /// One I/O call on the bytes the call has left.
    fn io(&mut self, flags: c_int) -> isize;
    /// Steps past the `moved` bytes the last call moved: whether any are left.
    fn advance(&mut self, moved: usize) -> bool;
    /// `catch_error(error)` over the socket's `kernel_nobufs`, which it sets on ENOBUFS.
    fn catch_error(&self, error: c_int) -> c_int;
    /// `has_kernel_nobufs()`: reads and clears the flag.
    fn take_nobufs(&self) -> bool;
    /// `swoole_set_last_error`.
    fn set_last_error(&self, error: c_int);
}

enum Step {
    Ready,
    Continue,
    Error,
}

use swoole_rs_corebase::sys::errno;
pub(crate) use swoole_rs_corebase::sys::set_errno;

/// `Socket::wait_for`'s loop around `kind`'s progress rule on the bytes `hooks`
/// holds, waiting for `event` up to `timeout_ms` (no limit unless positive;
/// the elapsed time since the start is subtracted on every wait, as upstream does).
/// Returns the bytes moved, or -1 with the last error and errno set.
pub fn swr_v1_socket_sync(
    hooks: &mut impl SyncEffects,
    kind: c_int,
    flags: c_int,
    event: c_int,
    mut timeout_ms: c_int,
) -> isize {
    let began = Instant::now();
    hooks.prepare();
    let mut bytes: usize = 0;
    let call_flags = if kind == SYNC_RECV {
        flags & !libc::MSG_WAITALL
    } else {
        flags
    };
    loop {
        let n = hooks.io(call_flags);
        let step = match n.cmp(&0) {
            core::cmp::Ordering::Less => {
                let error = errno();
                let retry = error == libc::EINTR || hooks.catch_error(error) == WAIT;
                if retry {
                    Step::Continue
                } else {
                    hooks.set_last_error(error);
                    Step::Error
                }
            }
            core::cmp::Ordering::Equal if kind == SYNC_SENDFILE => Step::Error,
            core::cmp::Ordering::Equal => Step::Ready,
            core::cmp::Ordering::Greater => {
                let moved = n.cast_unsigned();
                bytes += moved;
                // Only a call that goes on steps past what it moved: `SYNC_ONCE` (and a
                // `recv` without `MSG_WAITALL`) ends here.
                let more = match kind {
                    SYNC_SEND | SYNC_SENDFILE => hooks.advance(moved),
                    SYNC_RECV if flags & libc::MSG_WAITALL != 0 => hooks.advance(moved),
                    _ => false,
                };
                if more { Step::Continue } else { Step::Ready }
            }
        };
        match step {
            Step::Error => return -1,
            Step::Ready => return bytes.cast_signed(),
            Step::Continue => {
                if hooks.take_nobufs() {
                    std::thread::sleep(Duration::from_millis(10));
                    continue;
                }
            }
        }
        let events = hooks.events(event);
        let waited = crate::fd_wait(hooks.fd(), events, timeout_ms, hooks.dont_restart());
        let failed = match waited.status {
            1 => Some((POLL_TIMEOUT, libc::EAGAIN)),
            2 => Some((waited.error, waited.error)),
            _ => None,
        };
        if let Some((last, error)) = failed {
            hooks.set_last_error(last);
            set_errno(error);
            return -1;
        }
        if timeout_ms > 0 {
            #[expect(
                clippy::cast_possible_truncation,
                reason = "upstream's sec2msec converts to int"
            )]
            let elapsed = began.elapsed().as_millis() as c_int;
            timeout_ms -= elapsed;
            if timeout_ms <= 0 {
                hooks.set_last_error(libc::ETIMEDOUT);
                set_errno(libc::ETIMEDOUT);
                return -1;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::RefCell;

    /// A socket over `fd` whose I/O is `io` and whose wanted events are `events`.
    #[derive(Clone, Copy)]
    struct SyncHooks {
        fd: c_int,
        kernel_nobufs: *mut bool,
        events: fn() -> i16,
        io: fn(usize, c_int) -> isize,
        /// The bytes the call has left.
        left: usize,
    }

    impl SyncEffects for SyncHooks {
        fn fd(&self) -> c_int {
            self.fd
        }
        fn dont_restart(&self) -> bool {
            false
        }
        fn prepare(&self) {}
        fn events(&self, _: c_int) -> i16 {
            (self.events)()
        }
        fn io(&mut self, flags: c_int) -> isize {
            (self.io)(self.left, flags)
        }
        fn advance(&mut self, moved: usize) -> bool {
            if moved >= self.left {
                return false;
            }
            self.left -= moved;
            true
        }
        fn catch_error(&self, error: c_int) -> c_int {
            crate::catch_error(error, unsafe { &mut *self.kernel_nobufs })
        }
        fn take_nobufs(&self) -> bool {
            unsafe { std::mem::replace(&mut *self.kernel_nobufs, false) }
        }
        fn set_last_error(&self, error: c_int) {
            LAST_ERROR.with(|last| *last.borrow_mut() = error);
        }
    }

    thread_local! {
        static CALLS: RefCell<Vec<(usize, c_int)>> = const { RefCell::new(Vec::new()) };
        static LAST_ERROR: RefCell<c_int> = const { RefCell::new(0) };
    }

    fn events() -> i16 {
        libc::POLLOUT
    }
    /// Moves at most 3 bytes per call.
    fn three(length: usize, flags: c_int) -> isize {
        CALLS.with(|calls| calls.borrow_mut().push((length, flags)));
        length.min(3).cast_signed()
    }
    fn fails(_: usize, _: c_int) -> isize {
        set_errno(libc::ECONNRESET);
        -1
    }

    fn hooks(fd: c_int, nobufs: &mut bool, io: fn(usize, c_int) -> isize) -> SyncHooks {
        SyncHooks {
            fd,
            kernel_nobufs: nobufs,
            events,
            io,
            left: 0,
        }
    }

    fn run(
        hooks: &SyncHooks,
        kind: c_int,
        length: usize,
        flags: c_int,
    ) -> (isize, Vec<(usize, c_int)>) {
        CALLS.with(|calls| calls.borrow_mut().clear());
        let mut hooks = SyncHooks {
            left: length,
            ..*hooks
        };
        let result = swr_v1_socket_sync(&mut hooks, kind, flags, 2, 1000);
        (result, CALLS.with(|calls| calls.borrow().clone()))
    }

    #[test]
    fn progress_rules() {
        let mut fds = [0; 2];
        // SAFETY: `fds` receives the pair; its write end is always writable.
        assert_eq!(
            unsafe { libc::socketpair(libc::AF_UNIX, libc::SOCK_STREAM, 0, fds.as_mut_ptr()) },
            0
        );
        let mut nobufs = false;
        let hooks = hooks(fds[0], &mut nobufs, three);
        assert_eq!(
            run(&hooks, SYNC_SEND, 7, 0),
            (7, vec![(7, 0), (4, 0), (1, 0)]),
            "send retries short writes"
        );
        assert_eq!(run(&hooks, SYNC_SENDFILE, 5, 0), (5, vec![(5, 0), (2, 0)]));
        assert_eq!(run(&hooks, SYNC_ONCE, 7, 9), (3, vec![(7, 9)]), "one call");
        assert_eq!(
            run(&hooks, SYNC_RECV, 7, 0),
            (3, vec![(7, 0)]),
            "recv returns what one call read"
        );
        let waitall = libc::MSG_WAITALL | 1;
        assert_eq!(
            run(&hooks, SYNC_RECV, 7, waitall),
            (7, vec![(7, 1), (4, 1), (1, 1)]),
            "WAITALL loops, cleared per call"
        );

        let hooks = SyncHooks { io: fails, ..hooks };
        assert_eq!(run(&hooks, SYNC_SEND, 7, 0).0, -1);
        assert_eq!(LAST_ERROR.with(|last| *last.borrow()), libc::ECONNRESET);
        // SAFETY: closes the pair this test opened.
        unsafe {
            libc::close(fds[0]);
            libc::close(fds[1]);
        }
    }

    #[test]
    fn times_out_waiting() {
        fn again(_: usize, _: c_int) -> isize {
            set_errno(libc::EAGAIN);
            -1
        }
        fn readable() -> i16 {
            libc::POLLIN
        }
        let mut fds = [0; 2];
        // SAFETY: `fds` receives the pair; nothing is ever written, so it never reads.
        assert_eq!(
            unsafe { libc::socketpair(libc::AF_UNIX, libc::SOCK_STREAM, 0, fds.as_mut_ptr()) },
            0
        );
        let mut nobufs = false;
        let mut hooks = SyncHooks {
            events: readable,
            left: 4,
            ..hooks(fds[0], &mut nobufs, again)
        };
        let result = swr_v1_socket_sync(&mut hooks, SYNC_ONCE, 0, 1, 20);
        assert_eq!((result, errno()), (-1, libc::EAGAIN));
        assert_eq!(LAST_ERROR.with(|last| *last.borrow()), POLL_TIMEOUT);
        // SAFETY: closes the pair this test opened.
        unsafe {
            libc::close(fds[0]);
            libc::close(fds[1]);
        }
    }

    /// Each wait asks for the descriptor again, as upstream's `wait_event` reads
    /// `this->fd`: a signal handler an earlier wait dispatched may have swapped it.
    #[test]
    fn each_wait_asks_for_the_descriptor() {
        thread_local! {
            static FD: std::cell::Cell<(c_int, c_int)> = const { std::cell::Cell::new((-1, -1)) };
        }
        struct Moving(SyncHooks);
        impl SyncEffects for Moving {
            fn fd(&self) -> c_int {
                FD.with(|fd| fd.get().0)
            }
            fn dont_restart(&self) -> bool {
                self.0.dont_restart()
            }
            fn prepare(&self) {}
            fn events(&self, _: c_int) -> i16 {
                libc::POLLIN
            }
            fn io(&mut self, _: c_int) -> isize {
                // The first call swaps in the readable descriptor and would block.
                let (now, next) = FD.with(std::cell::Cell::get);
                if now == next {
                    return self.0.left.cast_signed();
                }
                FD.with(|fd| fd.set((next, next)));
                set_errno(libc::EAGAIN);
                -1
            }
            fn advance(&mut self, moved: usize) -> bool {
                self.0.advance(moved)
            }
            fn catch_error(&self, error: c_int) -> c_int {
                self.0.catch_error(error)
            }
            fn take_nobufs(&self) -> bool {
                self.0.take_nobufs()
            }
            fn set_last_error(&self, error: c_int) {
                self.0.set_last_error(error);
            }
        }
        let (mut idle, mut ready) = ([0; 2], [0; 2]);
        for pair in [&mut idle, &mut ready] {
            // SAFETY: `pair` receives a socket pair.
            let made =
                unsafe { libc::socketpair(libc::AF_UNIX, libc::SOCK_STREAM, 0, pair.as_mut_ptr()) };
            assert_eq!(made, 0);
        }
        // SAFETY: one byte into the second pair, so its other end reads.
        assert_eq!(unsafe { libc::write(ready[1], b"x".as_ptr().cast(), 1) }, 1);
        FD.with(|fd| fd.set((idle[0], ready[0])));
        let mut nobufs = false;
        let mut hooks = Moving(SyncHooks {
            left: 4,
            ..hooks(idle[0], &mut nobufs, three)
        });
        let result = swr_v1_socket_sync(&mut hooks, SYNC_ONCE, 0, 1, 1000);
        assert_eq!(result, 4, "the wait polled the swapped descriptor");
        // SAFETY: closes the pairs this test opened.
        unsafe {
            for fd in idle.into_iter().chain(ready) {
                libc::close(fd);
            }
        }
    }
}
