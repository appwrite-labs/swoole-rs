//! Hooked-fd timeout and poll preparation over the coroutine socket's
//! `network::Socket` timeouts. A virtual poll's wait saves the directional timeout
//! and its caller restores it afterwards.
use crate::co_io::set_errno;
use crate::co_socket::Co;
use libc::{c_int, pollfd};

use swoole_rs_reactor::TimeoutKind;

/// Set the timeout on a looked-up socket; false with EINVAL for a missing fd or
/// unknown option.
pub(crate) fn fd_timeout(socket: Option<Co>, which: c_int, timeout: f64, connect: bool) -> bool {
    let kind = if connect {
        Some(TimeoutKind::DNS | TimeoutKind::CONNECT)
    } else if which == libc::SO_RCVTIMEO {
        Some(TimeoutKind::READ)
    } else if which == libc::SO_SNDTIMEO {
        Some(TimeoutKind::WRITE)
    } else {
        None
    };
    let (Some(socket), Some(kind)) = (socket, kind) else {
        set_errno(libc::EINVAL);
        return false;
    };
    socket.set_timeout(timeout, kind);
    true
}

/// The one-fd fake poll after the nfds check: whether it faked one ready fd, with the
/// requested read and write bits; false leaves it to libc's poll (no socket or no
/// timeout).
pub(crate) fn fd_poll_fake(fd: &mut pollfd, timeout: c_int, socket: Option<Co>) -> bool {
    let Some(socket) = socket.filter(|_| timeout != 0) else {
        return false;
    };
    socket.set_timeout(f64::from(timeout) / 1000.0, TimeoutKind::ALL);
    fd.revents |= fd.events & (libc::POLLIN | libc::POLLOUT);
    true
}

/// Prepare the native no-socket poll; its cancellation point runs after Rust returns.
#[must_use]
pub extern "C" fn swr_v1_hook_fd_poll(fd: c_int, event: c_int) -> pollfd {
    pollfd {
        fd,
        events: swoole_rs_reactor::swr_v1_reactor_poll_events_to(
            swoole_rs_reactor::Events::from_bits(event),
        ),
        revents: 0,
    }
}

/// Saves the timeout of `event`'s direction and sets every direction to `timeout`
/// before a virtual poll; the caller restores every direction to the saved one.
pub(crate) fn fd_wait(socket: Co, event: u32, timeout: f64) -> f64 {
    let saved = socket.timeout(crate::transfer::timeout_kind(event));
    socket.set_timeout(timeout, TimeoutKind::ALL);
    saved
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::co_socket::CoSocket;
    use crate::shutdown::READ;
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
                .set(Some(Box::new(SocketRecord::new()).into_socket()));
            Self(socket)
        }

        fn co(&mut self) -> Co {
            unsafe { Co::new(&raw mut *self.0) }
        }

        fn record(&mut self) -> &mut SocketRecord {
            unsafe { &mut *self.0.socket_ptr() }
        }
    }

    impl Drop for Records {
        fn drop(&mut self) {
            drop(unsafe { Box::from_raw(self.0.socket_ptr()) });
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
            assert!(fd_timeout(Some(r.co()), option, 0.25, connect));
            let t = r.record().timeouts;
            assert_eq!([t.dns, t.connect, t.read, t.write], expected);
            assert_eq!(crate::co_io::errno(), 123);
        }
        let mut r = Records::new();
        assert!(!fd_timeout(Some(r.co()), -1, 1.0, false));
        assert_eq!(crate::co_io::errno(), libc::EINVAL);
        assert_eq!(r.record().timeouts.read, 60.0);
        assert!(!fd_timeout(None, libc::SO_RCVTIMEO, 1.0, false));
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
            r.co().set_timeout(saved, TimeoutKind::ALL);
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
            assert!(fd_poll_fake(&mut fd, 250, Some(r.co())));
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
        assert!(!fd_poll_fake(&mut fd, 0, None));
        assert_eq!(fd.revents, libc::POLLERR);
        let poll = swr_v1_hook_fd_poll(-1, READ.cast_signed());
        assert_eq!(poll.fd, -1);
        assert_eq!(poll.events, libc::POLLIN);
        assert_eq!(poll.revents, 0);
    }
}
