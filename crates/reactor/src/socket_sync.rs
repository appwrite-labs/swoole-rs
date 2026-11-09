//! `network::Socket`'s blocking calls as whole operations: `wait_for`'s loop
//! (prepare, retry classification, the ENOBUFS backoff, poll and timeout) and each
//! `*_sync` method's progress rule. Native code performs each I/O call, keeping its
//! counters and TLS, and reads the socket's bitfields, through [`SyncHooks`].
use std::io::Error;
use std::os::raw::{c_int, c_void};
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

/// What a blocking call needs from its native `Socket`.
#[repr(C)]
pub struct SyncHooks {
    pub socket: *mut c_void,
    pub fd: c_int,
    pub dont_restart: bool,
    /// `Socket::kernel_nobufs`, which `catch_error` sets on ENOBUFS.
    pub kernel_nobufs: *mut bool,
    /// `set_nonblock` when blocking and `ssl_clear_error` when TLS.
    pub prepare: unsafe extern "C" fn(socket: *mut c_void),
    /// `what_event_want(event)` as poll bits.
    pub events: unsafe extern "C" fn(socket: *mut c_void, event: c_int) -> i16,
    /// One native I/O call on `length` bytes at `buffer`.
    pub io: unsafe extern "C" fn(
        socket: *mut c_void,
        buffer: *mut c_void,
        length: usize,
        flags: c_int,
    ) -> isize,
    pub set_last_error: unsafe extern "C" fn(error: c_int),
    pub dispatch: Option<unsafe extern "C" fn()>,
}

enum Step {
    Ready,
    Continue,
    Error,
}

fn errno() -> c_int {
    Error::last_os_error().raw_os_error().unwrap_or(0)
}

pub(crate) fn set_errno(value: c_int) {
    #[cfg(target_os = "linux")]
    // SAFETY: this thread's errno slot.
    let slot = unsafe { libc::__errno_location() };
    #[cfg(not(target_os = "linux"))]
    // SAFETY: this thread's errno slot.
    let slot = unsafe { libc::__error() };
    // SAFETY: the slot is this thread's live errno.
    unsafe { *slot = value };
}

/// `Socket::wait_for`'s loop around `kind`'s progress rule on `length` bytes at
/// `buffer`, waiting for `event` up to `timeout_ms` (no limit unless positive;
/// the elapsed time since the start is subtracted on every wait, as upstream does).
/// Returns the bytes moved, or -1 with the last error and errno set.
///
/// # Safety
/// `hooks` is valid for this call: its functions are callable with its socket, and
/// `kernel_nobufs` is writable. `buffer` denotes `length` bytes valid for `io`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn swr_v1_socket_sync(
    hooks: *const SyncHooks,
    kind: c_int,
    buffer: *mut c_void,
    length: usize,
    flags: c_int,
    event: c_int,
    mut timeout_ms: c_int,
) -> isize {
    // SAFETY: the caller lends its hooks for this call.
    let hooks = unsafe { &*hooks };
    let began = Instant::now();
    // SAFETY: the caller's socket.
    unsafe { (hooks.prepare)(hooks.socket) };
    let mut bytes: usize = 0;
    loop {
        let (at, rest, call_flags) = match kind {
            SYNC_ONCE => (buffer, length, flags),
            SYNC_RECV => (
                buffer.wrapping_byte_add(bytes),
                length - bytes,
                flags & !libc::MSG_WAITALL,
            ),
            _ => (buffer.wrapping_byte_add(bytes), length - bytes, flags),
        };
        // SAFETY: the caller's I/O call on the rest of its buffer.
        let n = unsafe { (hooks.io)(hooks.socket, at, rest, call_flags) };
        let step = match n.cmp(&0) {
            core::cmp::Ordering::Less => {
                let error = errno();
                let retry = error == libc::EINTR
                // SAFETY: the caller's kernel_nobufs flag.
                || unsafe { crate::catch_error(error, &mut *hooks.kernel_nobufs) } == WAIT;
                if retry {
                    Step::Continue
                } else {
                    // SAFETY: native swoole_set_last_error.
                    unsafe { (hooks.set_last_error)(error) };
                    Step::Error
                }
            }
            core::cmp::Ordering::Equal if kind == SYNC_SENDFILE => Step::Error,
            core::cmp::Ordering::Equal => Step::Ready,
            core::cmp::Ordering::Greater => {
                bytes += n.cast_unsigned();
                match kind {
                    SYNC_SEND | SYNC_SENDFILE if bytes < length => Step::Continue,
                    SYNC_RECV if flags & libc::MSG_WAITALL != 0 && bytes < length => Step::Continue,
                    _ => Step::Ready,
                }
            }
        };
        match step {
            Step::Error => return -1,
            Step::Ready => return bytes.cast_signed(),
            Step::Continue => {
                // SAFETY: the caller's kernel_nobufs flag; has_kernel_nobufs reads and clears it.
                if unsafe { std::mem::replace(&mut *hooks.kernel_nobufs, false) } {
                    std::thread::sleep(Duration::from_millis(10));
                    continue;
                }
            }
        }
        // SAFETY: the caller's socket.
        let events = unsafe { (hooks.events)(hooks.socket, event) };
        let waited = crate::swr_v1_socket_wait(
            hooks.fd,
            events,
            timeout_ms,
            hooks.dont_restart,
            hooks.dispatch,
        );
        let failed = match waited.status {
            1 => Some((POLL_TIMEOUT, libc::EAGAIN)),
            2 => Some((waited.error, waited.error)),
            _ => None,
        };
        if let Some((last, error)) = failed {
            // SAFETY: native swoole_set_last_error.
            unsafe { (hooks.set_last_error)(last) };
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
                // SAFETY: native swoole_set_last_error.
                unsafe { (hooks.set_last_error)(libc::ETIMEDOUT) };
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

    thread_local! {
        static CALLS: RefCell<Vec<(usize, c_int)>> = const { RefCell::new(Vec::new()) };
        static LAST_ERROR: RefCell<c_int> = const { RefCell::new(0) };
    }

    unsafe extern "C" fn prepare(_: *mut c_void) {}
    unsafe extern "C" fn events(_: *mut c_void, _: c_int) -> i16 {
        libc::POLLOUT
    }
    unsafe extern "C" fn set_last_error(error: c_int) {
        LAST_ERROR.with(|last| *last.borrow_mut() = error);
    }
    /// Moves at most 3 bytes per call.
    unsafe extern "C" fn three(
        _: *mut c_void,
        _: *mut c_void,
        length: usize,
        flags: c_int,
    ) -> isize {
        CALLS.with(|calls| calls.borrow_mut().push((length, flags)));
        length.min(3).cast_signed()
    }
    unsafe extern "C" fn fails(_: *mut c_void, _: *mut c_void, _: usize, _: c_int) -> isize {
        set_errno(libc::ECONNRESET);
        -1
    }

    fn hooks(
        fd: c_int,
        nobufs: &mut bool,
        io: unsafe extern "C" fn(*mut c_void, *mut c_void, usize, c_int) -> isize,
    ) -> SyncHooks {
        SyncHooks {
            socket: std::ptr::null_mut(),
            fd,
            dont_restart: false,
            kernel_nobufs: nobufs,
            prepare,
            events,
            io,
            set_last_error,
            dispatch: None,
        }
    }

    fn run(
        hooks: &SyncHooks,
        kind: c_int,
        length: usize,
        flags: c_int,
    ) -> (isize, Vec<(usize, c_int)>) {
        CALLS.with(|calls| calls.borrow_mut().clear());
        let mut buffer = vec![0_u8; length];
        // SAFETY: the hooks are local and the buffer has `length` bytes.
        let result = unsafe {
            swr_v1_socket_sync(
                hooks,
                kind,
                buffer.as_mut_ptr().cast(),
                length,
                flags,
                2,
                1000,
            )
        };
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
        unsafe extern "C" fn again(_: *mut c_void, _: *mut c_void, _: usize, _: c_int) -> isize {
            set_errno(libc::EAGAIN);
            -1
        }
        unsafe extern "C" fn readable(_: *mut c_void, _: c_int) -> i16 {
            libc::POLLIN
        }
        let mut fds = [0; 2];
        // SAFETY: `fds` receives the pair; nothing is ever written, so it never reads.
        assert_eq!(
            unsafe { libc::socketpair(libc::AF_UNIX, libc::SOCK_STREAM, 0, fds.as_mut_ptr()) },
            0
        );
        let mut nobufs = false;
        let hooks = SyncHooks {
            events: readable,
            ..hooks(fds[0], &mut nobufs, again)
        };
        let mut buffer = [0_u8; 4];
        // SAFETY: local hooks and buffer.
        let result = unsafe {
            swr_v1_socket_sync(
                &raw const hooks,
                SYNC_ONCE,
                buffer.as_mut_ptr().cast(),
                4,
                0,
                1,
                20,
            )
        };
        assert_eq!((result, errno()), (-1, libc::EAGAIN));
        assert_eq!(LAST_ERROR.with(|last| *last.borrow()), POLL_TIMEOUT);
        // SAFETY: closes the pair this test opened.
        unsafe {
            libc::close(fds[0]);
            libc::close(fds[1]);
        }
    }
}
