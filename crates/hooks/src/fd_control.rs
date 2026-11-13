//! Hooked-fd timeout and poll preparation over the coroutine socket's
//! `network::Socket` timeouts. A virtual poll's wait saves the directional timeout
//! and its caller restores it afterwards.
use crate::co_io::set_errno;
use crate::co_socket::Co;
use crate::shutdown::READ;
use libc::{c_int, pollfd};

const DNS: c_int = 1;
const CONNECT: c_int = 2;
const RECEIVE: c_int = 4;
const SEND: c_int = 8;
/// Every timeout direction.
pub(crate) const ALL: c_int = DNS | CONNECT | RECEIVE | SEND;

/// Set the timeout on a looked-up socket, or EINVAL for a missing fd/unknown option.
pub(crate) fn fd_timeout(socket: Option<Co>, which: c_int, timeout: f64, connect: bool) -> c_int {
    let kind = if connect {
        DNS | CONNECT
    } else if which == libc::SO_RCVTIMEO {
        RECEIVE
    } else if which == libc::SO_SNDTIMEO {
        SEND
    } else {
        0
    };
    let Some(socket) = socket.filter(|_| kind != 0) else {
        set_errno(libc::EINVAL);
        return -1;
    };
    socket.set_timeout(timeout, kind);
    0
}

/// The one-fd fake poll after the nfds check: 2 for libc's poll (no socket or no
/// timeout), else 1 with the requested read and write bits ready.
pub(crate) fn fd_poll_fake(fd: &mut pollfd, timeout: c_int, socket: Option<Co>) -> c_int {
    let Some(socket) = socket.filter(|_| timeout != 0) else {
        return 2;
    };
    socket.set_timeout(f64::from(timeout) / 1000.0, ALL);
    fd.revents |= fd.events & (libc::POLLIN | libc::POLLOUT);
    1
}

/// Prepare the native no-socket poll; its cancellation point runs after Rust returns.
#[must_use]
pub extern "C" fn swr_v1_hook_fd_poll(fd: c_int, event: c_int) -> pollfd {
    pollfd {
        fd,
        events: swoole_rs_reactor::swr_v1_reactor_poll_events_to(event),
        revents: 0,
    }
}

/// Saves the timeout of `event`'s direction and sets every direction to `timeout`
/// before a virtual poll; the caller restores every direction to the saved one.
pub(crate) fn fd_wait(socket: Co, event: u32, timeout: f64) -> f64 {
    let saved = socket.timeout(if event == READ { RECEIVE } else { SEND });
    socket.set_timeout(timeout, ALL);
    saved
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::co_socket::CoSocket;
    use swoole_rs_reactor::SocketRecord;

    /// A zeroed record over a fresh network socket. Tests reach the network socket
    /// through the record's pointer, as the code does, so no second owner aliases it.
    struct Records(Box<CoSocket>);

    impl Records {
        fn new() -> Self {
            // Every field is a scalar, raw pointer or Protocol of those; zero is valid.
            let socket = Box::new(unsafe { core::mem::zeroed::<CoSocket>() });
            socket
                .socket
                .set(Box::into_raw(Box::new(SocketRecord::new())));
            Self(socket)
        }

        fn co(&mut self) -> Co {
            unsafe { Co::new(&raw mut *self.0) }
        }

        fn record(&mut self) -> &mut SocketRecord {
            unsafe { &mut *self.0.socket.get() }
        }
    }

    impl Drop for Records {
        fn drop(&mut self) {
            drop(unsafe { Box::from_raw(self.0.socket.get()) });
        }
    }

    #[test]
    fn socket_options_select_only_the_original_direction_and_preserve_errno() {
        for (option, connect, expected) in [
            (libc::SO_RCVTIMEO, false, [60.0, 10.0, 0.25, 60.0]),
            (libc::SO_SNDTIMEO, false, [60.0, 10.0, 60.0, 0.25]),
            (-1, true, [0.25, 0.25, 60.0, 60.0]),
        ] {
            let mut r = Records::new();
            set_errno(123);
            assert_eq!(fd_timeout(Some(r.co()), option, 0.25, connect), 0);
            let t = r.record().timeouts;
            assert_eq!([t.dns, t.connect, t.read, t.write], expected);
            assert_eq!(crate::co_io::errno(), 123);
        }
        let mut r = Records::new();
        assert_eq!(fd_timeout(Some(r.co()), -1, 1.0, false), -1);
        assert_eq!(crate::co_io::errno(), libc::EINVAL);
        assert_eq!(r.record().timeouts.read, 60.0);
        assert_eq!(fd_timeout(None, libc::SO_RCVTIMEO, 1.0, false), -1);
        assert_eq!(crate::co_io::errno(), libc::EINVAL);
    }

    #[test]
    fn virtual_poll_continuations_save_direction_and_restore_all_without_callback() {
        for event in [READ, crate::shutdown::WRITE, 0] {
            let mut r = Records::new();
            r.record().timeouts.read = 4.0;
            r.record().timeouts.write = 8.0;
            let saved = fd_wait(r.co(), event, 0.25);
            assert_eq!(r.record().timeouts.read, 0.25);
            assert_eq!(r.record().timeouts.write, 0.25);
            r.co().set_timeout(saved, ALL);
            let expected = if event == READ { 4.0 } else { 8.0 };
            let t = r.record().timeouts;
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
            let mut r = Records::new();
            let mut fd = pollfd {
                fd: -1,
                events,
                revents: libc::POLLERR,
            };
            assert_eq!(fd_poll_fake(&mut fd, 250, Some(r.co())), 1);
            assert_eq!(
                fd.revents,
                libc::POLLERR | (events & (libc::POLLIN | libc::POLLOUT))
            );
            assert_eq!(r.record().timeouts.read, 0.25);
        }
        let mut fd = pollfd {
            fd: -1,
            events: libc::POLLIN,
            revents: libc::POLLERR,
        };
        assert_eq!(fd_poll_fake(&mut fd, 0, None), 2);
        assert_eq!(fd.revents, libc::POLLERR);
        let poll = swr_v1_hook_fd_poll(-1, READ.cast_signed());
        assert_eq!(poll.fd, -1);
        assert_eq!(poll.events, libc::POLLIN);
        assert_eq!(poll.revents, 0);
    }
}
