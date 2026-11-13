//! A `Socket` this side made and owns until it frees it: [`OwnedSocket`], and over it
//! [`EventSocket`], a descriptor its caller keeps, on the thread's reactor:
//! `Swoole\Event`'s user sockets.
use crate::upstream::socket_free;
use std::ffi::c_int;
use swoole_rs_reactor::{Socket, SocketRecord, event_api, socket_upstream};

/// A new `network::Socket` its maker holds alone until [`OwnedSocket::free`]
/// (`Socket::free`) frees it or [`OwnedSocket::into_raw`] hands it on. Handles to it
/// borrow the owner, so none outlives the free; the thread's reactor, which
/// [`OwnedSocket::add`] registers it with, is told by `Socket::free` itself.
pub struct OwnedSocket(Socket<'static>);

impl OwnedSocket {
    /// `record`, which nothing else holds.
    #[must_use]
    pub fn new(record: Box<SocketRecord>) -> Self {
        Self(record.into_socket())
    }

    /// The socket's handle, for as long as the owner is borrowed.
    #[must_use]
    pub fn get(&self) -> Socket<'_> {
        self.0
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

    /// `swoole_event_write(socket, data, len)`.
    #[must_use]
    pub fn write(&self, data: &[u8]) -> isize {
        event_api::event_write(self.0, data)
    }

    /// The record, which its new holder frees.
    #[must_use]
    pub fn into_raw(self) -> *mut SocketRecord {
        self.0.as_ptr()
    }

    /// `socket->free()`: closes the fd, unless it is -1, and frees the record.
    pub fn free(self) {
        // SAFETY: the record this owned, which nothing uses afterwards: every handle
        // borrowed the owner, and a reactor still holding it is told by `free` itself.
        unsafe { socket_free(self.0.as_ptr()) };
    }
}

/// `make_socket(fd, fd_type)`, non-blocking, which this owns until [`EventSocket::free`].
/// Its event calls do nothing, and fail, without a reactor.
pub struct EventSocket(OwnedSocket);

fn reactor() -> bool {
    !swoole_rs_corebase::thread_reactor().is_null()
}

impl EventSocket {
    #[must_use]
    pub fn new(fd: c_int, fd_type: c_int) -> Self {
        let socket = OwnedSocket::new(Box::new(SocketRecord::with_fd(fd, fd_type)));
        socket
            .get()
            .edit(|record| socket_upstream::socket_set_fd_option(record, 1, -1));
        Self(socket)
    }

    #[must_use]
    pub fn events(&self) -> c_int {
        self.0.get().read(|record| record.events)
    }

    #[must_use]
    pub fn removed(&self) -> bool {
        self.0.get().read(|record| record.removed)
    }

    /// `swoole_event_add(socket, events)`.
    #[must_use]
    pub fn add(&self, events: c_int) -> c_int {
        self.0.add(events)
    }

    /// `swoole_event_set(socket, events)`.
    #[must_use]
    pub fn set(&self, events: c_int) -> c_int {
        self.0.set(events)
    }

    /// `swoole_event_del(socket)`.
    #[must_use]
    pub fn del(&self) -> c_int {
        event_api::event_del(self.0.get())
    }

    /// `swoole_event_write(socket, data, len)`.
    #[must_use]
    pub fn write(&self, data: &[u8]) -> isize {
        if !reactor() {
            return -1;
        }
        self.0.write(data)
    }

    /// `socket->free()`, which closes the fd unless `keep_fd` (`socket->fd = -1`
    /// first, once the reactor no longer holds it).
    pub fn free(self, keep_fd: bool) {
        if keep_fd {
            if !self.removed() {
                let _deleted = self.del();
            }
            self.0.get().edit(|record| record.fd = -1);
        }
        self.0.free();
    }
}
