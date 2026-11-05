//! Hooked-fd timeout and poll preparation over the actual coroutine/network records.
//! Native virtual poll runs only after preparation returns; restoration receives
//! the saved scalar afterwards, so no Rust owner frame surrounds arbitrary code.
use crate::co_io::set_errno;
use crate::co_socket::CoSocket;
use crate::shutdown::READ;
use libc::{c_int, pollfd};
use swoole_rs_reactor::SocketRecord;

const DNS: c_int = 1;
const CONNECT: c_int = 2;
const RECEIVE: c_int = 4;
const SEND: c_int = 8;
const ALL: c_int = DNS | CONNECT | RECEIVE | SEND;

/// A native virtual poll continuation's saved timeout. No ownership or Drop effect.
#[repr(C)]
#[derive(Clone, Copy)]
pub struct FdWait {
    pub socket: *mut SocketRecord,
    pub timeout: f64,
}

/// The caller lends the live record exclusively until this helper returns.
unsafe fn set_timeout(record: *mut SocketRecord, timeout: f64, kind: c_int) {
    // SAFETY: the caller lends the canonical timeout group.
    let timeouts = unsafe { &raw mut (*record).timeouts };
    // SAFETY: the group is live and exclusively accessed for this call.
    unsafe { swoole_rs_reactor::swr_v1_socket_set_timeout(timeouts, timeout, kind) };
}

/// Set the timeout on a looked-up socket, or EINVAL for a missing fd/unknown option.
///
/// # Safety
/// `socket` is null or a live, exclusively accessed coroutine socket.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn swr_v1_hook_fd_timeout(
    socket: *mut CoSocket,
    which: c_int,
    timeout: f64,
    connect: bool,
) -> c_int {
    let kind = if connect {
        DNS | CONNECT
    } else if which == libc::SO_RCVTIMEO {
        RECEIVE
    } else if which == libc::SO_SNDTIMEO {
        SEND
    } else {
        0
    };
    if socket.is_null() || kind == 0 {
        set_errno(libc::EINVAL);
        return -1;
    }
    // SAFETY: the caller lends the live socket record for this operation.
    let record = unsafe { (*socket).socket };
    // SAFETY: its canonical timeout group is live and exclusively accessed.
    unsafe { set_timeout(record, timeout, kind) };
    0
}

/// The one-fd fake poll after native's nfds/error and coroutine checks.
/// Returns2 for native poll, which remains outside Rust for pthread cancellation.
///
/// # Safety
/// `fd` points to one writable pollfd. `socket` is null or a live record.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn swr_v1_hook_fd_poll_fake(
    fd: *mut pollfd,
    timeout: c_int,
    socket: *mut CoSocket,
) -> c_int {
    if timeout == 0 || socket.is_null() {
        return 2;
    }
    // SAFETY: the live socket is accessed only until this function returns.
    let record = unsafe { (*socket).socket };
    // SAFETY: its canonical timeout group is live and exclusively accessed.
    unsafe { set_timeout(record, f64::from(timeout) / 1000.0, ALL) };
    // SAFETY: one writable pollfd, as the caller promises.
    let fd = unsafe { &mut *fd };
    fd.revents |= fd.events & (libc::POLLIN | libc::POLLOUT);
    1
}

/// Prepare the native no-socket poll; its cancellation point runs after Rust returns.
#[unsafe(no_mangle)]
pub extern "C" fn swr_v1_hook_fd_poll(fd: c_int, event: c_int) -> pollfd {
    pollfd {
        fd,
        events: swoole_rs_reactor::swr_v1_reactor_poll_events_to(event),
        revents: 0,
    }
}

/// Save the directional timeout and set all directions before native virtual poll.
///
/// # Safety
/// `socket` and its network socket are live and exclusively accessed for this call.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn swr_v1_hook_fd_wait(
    socket: *mut CoSocket,
    event: u32,
    timeout: f64,
) -> FdWait {
    // SAFETY: the live record is lent only for this preparation.
    let record = unsafe { (*socket).socket };
    // SAFETY: the socket owns this timeout group; no native code is running now.
    let timeouts = unsafe { &mut (*record).timeouts };
    let saved = if event == READ {
        timeouts.read
    } else {
        timeouts.write
    };
    // SAFETY: the group is live and exclusively borrowed during this call.
    unsafe { swoole_rs_reactor::swr_v1_socket_set_timeout(timeouts, timeout, ALL) };
    FdWait {
        socket: record,
        timeout: saved,
    }
}

/// Restore all timeout directions after native virtual poll returns, as upstream does.
///
/// # Safety
/// `wait` is the copied preparation result; its socket remains live after poll.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn swr_v1_hook_fd_wait_restore(wait: FdWait) {
    // SAFETY: the caller returns the saved, still-live canonical socket.
    unsafe { set_timeout(wait.socket, wait.timeout, ALL) };
}

#[cfg(test)]
mod tests {
    use super::*;

    fn records() -> (SocketRecord, CoSocket) {
        // Every field is a scalar, raw pointer or Protocol of those; zero is valid.
        (SocketRecord::new(), unsafe { core::mem::zeroed() })
    }

    #[test]
    fn socket_options_select_only_the_original_direction_and_preserve_errno() {
        for (option, connect, expected) in [
            (libc::SO_RCVTIMEO, false, [60.0, 10.0, 0.25, 60.0]),
            (libc::SO_SNDTIMEO, false, [60.0, 10.0, 60.0, 0.25]),
            (-1, true, [0.25, 0.25, 60.0, 60.0]),
        ] {
            let (mut record, mut socket) = records();
            socket.socket = &raw mut record;
            set_errno(123);
            assert_eq!(
                unsafe { swr_v1_hook_fd_timeout(&raw mut socket, option, 0.25, connect) },
                0
            );
            let t = record.timeouts;
            assert_eq!([t.dns, t.connect, t.read, t.write], expected);
            assert_eq!(crate::co_io::errno(), 123);
        }
        let (mut record, mut socket) = records();
        socket.socket = &raw mut record;
        assert_eq!(
            unsafe { swr_v1_hook_fd_timeout(&raw mut socket, -1, 1.0, false) },
            -1
        );
        assert_eq!(crate::co_io::errno(), libc::EINVAL);
        assert_eq!(record.timeouts.read, 60.0);
        assert_eq!(
            unsafe { swr_v1_hook_fd_timeout(core::ptr::null_mut(), libc::SO_RCVTIMEO, 1.0, false) },
            -1
        );
        assert_eq!(crate::co_io::errno(), libc::EINVAL);
    }

    #[test]
    fn virtual_poll_continuations_save_direction_and_restore_all_without_callback() {
        for event in [READ, crate::shutdown::WRITE, 0] {
            let (mut record, mut socket) = records();
            socket.socket = &raw mut record;
            record.timeouts.read = 4.0;
            record.timeouts.write = 8.0;
            let wait = unsafe { swr_v1_hook_fd_wait(&raw mut socket, event, 0.25) };
            assert_eq!(record.timeouts.read, 0.25);
            assert_eq!(record.timeouts.write, 0.25);
            unsafe { swr_v1_hook_fd_wait_restore(wait) };
            let expected = if event == READ { 4.0 } else { 8.0 };
            let t = record.timeouts;
            assert_eq!([t.dns, t.connect, t.read, t.write], [expected; 4]);
        }
    }

    #[test]
    fn fake_poll_preserves_existing_revents_and_reports_only_requested_read_write_bits() {
        for events in [
            0,
            libc::POLLIN,
            libc::POLLOUT,
            libc::POLLIN | libc::POLLOUT,
            libc::POLLPRI,
        ] {
            let (mut record, mut socket) = records();
            socket.socket = &raw mut record;
            let mut fd = pollfd {
                fd: -1,
                events,
                revents: libc::POLLERR,
            };
            assert_eq!(
                unsafe { swr_v1_hook_fd_poll_fake(&raw mut fd, 250, &raw mut socket) },
                1
            );
            assert_eq!(
                fd.revents,
                libc::POLLERR | (events & (libc::POLLIN | libc::POLLOUT))
            );
            assert_eq!(record.timeouts.read, 0.25);
        }
        let mut fd = pollfd {
            fd: -1,
            events: libc::POLLIN,
            revents: libc::POLLERR,
        };
        assert_eq!(
            unsafe { swr_v1_hook_fd_poll_fake(&raw mut fd, 0, core::ptr::null_mut()) },
            2
        );
        assert_eq!(fd.revents, libc::POLLERR);
        let poll = swr_v1_hook_fd_poll(-1, READ.cast_signed());
        assert_eq!(poll.fd, -1);
        assert_eq!(poll.events, libc::POLLIN);
        assert_eq!(poll.revents, 0);
    }
}
