//! A `Socket` over a descriptor its caller keeps, on the thread's reactor:
//! `Swoole\Event`'s user sockets.
use crate::upstream::socket_free;
use std::ffi::c_int;
use swoole_rs_reactor::{Socket, SocketRecord, event_api, socket_upstream};

/// `make_socket(fd, fd_type)`, non-blocking, which this owns until [`EventSocket::free`].
/// Its event calls do nothing, and fail, without a reactor.
pub struct EventSocket(Socket<'static>);

fn reactor() -> bool {
    !swoole_rs_corebase::thread_reactor().is_null()
}

impl EventSocket {
    #[must_use]
    pub fn new(fd: c_int, fd_type: c_int) -> Self {
        // The new record, which this owns until `free`; the reactor reaches it only
        // through the calls below.
        let socket = Box::new(SocketRecord::with_fd(fd, fd_type)).into_socket();
        socket.edit(|record| socket_upstream::socket_set_fd_option(record, 1, -1));
        Self(socket)
    }

    #[must_use]
    pub fn events(&self) -> c_int {
        self.0.read(|record| record.events)
    }

    #[must_use]
    pub fn removed(&self) -> bool {
        self.0.read(|record| record.removed)
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
        event_api::event_write(self.0, data)
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
