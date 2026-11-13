//! A `Socket` over a descriptor its caller keeps, on the thread's reactor:
//! `Swoole\Event`'s user sockets.
use crate::upstream::socket_free;
use std::ffi::c_int;
use swoole_rs_reactor::{Socket, event_api, socket_upstream};

/// `make_socket(fd, fd_type)`, non-blocking, which this owns until [`EventSocket::free`].
/// Its event calls do nothing, and fail, without a reactor.
pub struct EventSocket(Socket<'static>);

fn reactor() -> bool {
    !swoole_rs_corebase::thread_reactor().is_null()
}

impl EventSocket {
    #[must_use]
    pub fn new(fd: c_int, fd_type: c_int) -> Self {
        let record = socket_upstream::make_socket_fd(fd, fd_type);
        // SAFETY: the new record, which this owns until `free`; the reactor reaches it only
        // through the calls below.
        let Some(socket) = (unsafe { Socket::from_ptr(record) }) else {
            std::process::abort()
        };
        socket.edit(|record| socket_upstream::socket_set_fd_option(record, 1, -1));
        Self(socket)
    }

    #[must_use]
    pub fn events(&self) -> c_int {
        self.0.read(|record| record.events)
    }

    #[must_use]
    pub fn removed(&self) -> bool {
        self.0.read(|record| record.removed != 0)
    }

    /// `swoole_event_add(socket, events)`.
    #[must_use]
    pub fn add(&self, events: c_int) -> c_int {
        event_api::event_add(self.0, events)
    }

    /// `swoole_event_set(socket, events)`.
    #[must_use]
    pub fn set(&self, events: c_int) -> c_int {
        event_api::event_set(self.0, events)
    }

    /// `swoole_event_del(socket)`.
    #[must_use]
    pub fn del(&self) -> c_int {
        event_api::event_del(self.0)
    }

    /// `swoole_event_write(socket, data, len)`.
    #[must_use]
    pub fn write(&self, data: &[u8]) -> isize {
        if !reactor() {
            return -1;
        }
        // SAFETY: the live record, on the thread's live reactor, with the caller's bytes.
        unsafe { event_api::swoole_event_write(self.0.as_ptr(), data.as_ptr().cast(), data.len()) }
    }

    /// `socket->free()`, which closes the fd unless `keep_fd` (`socket->fd = -1`
    /// first, once the reactor no longer holds it).
    pub fn free(self, keep_fd: bool) {
        if keep_fd {
            if !self.removed() {
                let _deleted = self.del();
            }
            self.0.edit(|record| record.fd = -1);
        }
        // SAFETY: the record this owned, which nothing uses afterwards; a reactor
        // still holding it is told by `free` itself.
        unsafe { socket_free(self.0.as_ptr()) };
    }
}
