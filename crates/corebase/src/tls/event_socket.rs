//! A `Socket` record this owns, on the thread's reactor: `Swoole\Event`'s user
//! sockets, a pipe's ends and an event waiter's watch.
use crate::reactor::{Heap, OwnedSocket, Socket, SocketFields, SocketRecord, event_api};
use std::ffi::c_int;

/// A heap record this owns until [`EventSocket::free`], lending it only for a borrow
/// of `self`; the reactor reaches it only through the calls below, and `free` takes
/// it off. Its event calls do nothing, and fail, without a reactor.
#[repr(transparent)]
pub struct EventSocket(OwnedSocket);

fn reactor() -> bool {
    !crate::thread_reactor().is_null()
}

impl EventSocket {
    /// `make_socket(fd, fd_type)`, non-blocking.
    #[must_use]
    pub fn new(fd: c_int, fd_type: c_int) -> Self {
        let socket = Self::adopt(Box::new(SocketRecord::with_fd(fd, fd_type)));
        socket
            .socket()
            .edit(|record| record.set_fd_option_or_warn(1, -1));
        socket
    }

    /// `record`, which this owns from here.
    #[must_use]
    pub fn adopt(record: Box<SocketRecord>) -> Self {
        Self(OwnedSocket::new(record))
    }

    /// The record, which the caller owns from here.
    #[must_use]
    pub fn into_raw(self) -> *mut SocketRecord {
        self.0.leak().as_ptr()
    }

    /// The record, for as long as this is borrowed.
    #[must_use]
    pub const fn socket(&self) -> Socket<'_> {
        self.0.socket()
    }

    /// The record for as long as this is borrowed, which a reactor may register: its
    /// free deletes it from the reactor first.
    #[must_use]
    pub const fn heap(&self) -> Heap<'_> {
        self.0.heap()
    }

    #[must_use]
    pub fn events(&self) -> crate::reactor::Events {
        self.socket().read(|record| record.events)
    }

    #[must_use]
    pub fn removed(&self) -> bool {
        self.socket().read(|record| record.removed)
    }

    /// `swoole_event_add(socket, events)`: whether it worked.
    #[must_use]
    pub fn add(&self, events: crate::reactor::Events) -> bool {
        event_api::event_add(self.heap(), events)
    }

    /// `swoole_event_set(socket, events)`: whether it worked.
    #[must_use]
    pub fn set(&self, events: crate::reactor::Events) -> bool {
        event_api::event_set(self.heap(), events)
    }

    /// `swoole_event_del(socket)`: whether it worked.
    #[must_use]
    pub fn del(&self) -> bool {
        event_api::event_del(self.socket())
    }

    /// `swoole_event_write(socket, data, len)`: the bytes taken, `None` when it failed
    /// or no reactor runs.
    #[must_use]
    pub fn write(&self, data: &[u8]) -> Option<usize> {
        if !reactor() {
            return None;
        }
        event_api::event_write(self.heap(), data)
    }

    /// `socket->free()`, which closes the fd unless `keep_fd` (`socket->fd = -1`
    /// first, once the reactor no longer holds it).
    pub fn free(self, keep_fd: bool) {
        if keep_fd && !self.removed() {
            // The upstream io_uring destructor detaches the fd and frees the socket even when del fails.
            let _deleted = self.del();
        }
        self.release_record(keep_fd);
    }

    /// `void Socket::free()` of the record as it is: deletes the timers and buffers,
    /// then, while the thread's event loop is available, removes the record from it
    /// and defers the close to it; otherwise closes its fd (`-1` for none) and frees
    /// it at once. Outside the hybrid there are no timers, buffers or loop.
    pub fn release(self) {
        self.release_record(false);
    }

    /// Release after deregistration when the descriptor stays with its caller.
    pub(crate) fn release_record(self, keep_fd: bool) {
        let owned = self.0;
        let owned = {
            let record = owned.socket();
            // The output queue leaves the record here and goes below, after the timers
            // and outside the record's borrow: an entry's disposer runs as it goes.
            let (recv_timer, send_timer, queue) = record.edit(|record| {
                if keep_fd {
                    record.forget_descriptor();
                }
                (
                    record.disarm_recv_timer(),
                    record.take_send_timer(),
                    record.take_output_queue(),
                )
            });
            crate::timer::del_disarmed(recv_timer);
            if let Some(timer) = send_timer {
                crate::timer::del_id(timer);
            }
            // `delete out_buffer`.
            drop(queue);
            if event_api::swoole_event_is_available() {
                if record.read(SocketFields::is_registered) {
                    // `swoole_event_del(this)` on the thread's reactor, which logs a
                    // failure.
                    _ = event_api::event_del(record);
                }
                // `swoole_event_defer(socket_free_defer, this)`; a reactor freed before
                // it runs closes the socket all the same.
                match crate::reactor::defer_free(owned) {
                    Ok(()) => return,
                    Err(owned) => owned,
                }
            } else {
                owned
            }
        };
        // base.cc's `socket_free_defer`, at once.
        owned.into_box().close_and_free();
    }
}

impl From<OwnedSocket> for EventSocket {
    fn from(owned: OwnedSocket) -> Self {
        Self(owned)
    }
}

/// `void Socket::free()`: [`EventSocket::release`] of the record.
pub fn socket_free(socket: Option<OwnedSocket>) {
    if let Some(owned) = socket {
        EventSocket::from(owned).release();
    }
}
