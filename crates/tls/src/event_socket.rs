//! A `Socket` record this owns, on the thread's reactor: `Swoole\Event`'s user
//! sockets, a pipe's ends and an event waiter's watch.
use std::ffi::c_int;
use swoole_rs_reactor::{Socket, SocketRecord, event_api, socket_upstream};

/// A heap record this owns until [`EventSocket::free`], lending it only for a borrow
/// of `self`; the reactor reaches it only through the calls below, and `free` takes
/// it off. Its event calls do nothing, and fail, without a reactor.
#[repr(transparent)]
pub struct EventSocket(Socket<'static>);

fn reactor() -> bool {
    !swoole_rs_corebase::thread_reactor().is_null()
}

impl EventSocket {
    /// `make_socket(fd, fd_type)`, non-blocking.
    #[must_use]
    pub fn new(fd: c_int, fd_type: c_int) -> Self {
        let socket = Self::adopt(Box::new(SocketRecord::with_fd(fd, fd_type)));
        socket
            .0
            .edit(|record| socket_upstream::socket_set_fd_option(record, 1, -1));
        socket
    }

    /// `record`, which this owns from here.
    #[must_use]
    pub fn adopt(record: Box<SocketRecord>) -> Self {
        Self(record.into_socket())
    }

    /// The record, which the caller owns from here.
    #[must_use]
    pub fn into_raw(self) -> *mut SocketRecord {
        self.0.as_ptr()
    }

    /// The record, for as long as this is borrowed.
    #[must_use]
    pub const fn socket(&self) -> Socket<'_> {
        self.0
    }

    #[must_use]
    pub fn events(&self) -> swoole_rs_reactor::Events {
        self.0.read(|record| record.events)
    }

    #[must_use]
    pub fn removed(&self) -> bool {
        self.0.read(|record| record.removed)
    }

    /// `swoole_event_add(socket, events)`: whether it worked.
    #[must_use]
    pub fn add(&self, events: swoole_rs_reactor::Events) -> bool {
        event_api::event_add(self.0, events)
    }

    /// `swoole_event_set(socket, events)`: whether it worked.
    #[must_use]
    pub fn set(&self, events: swoole_rs_reactor::Events) -> bool {
        event_api::event_set(self.0, events)
    }

    /// `swoole_event_del(socket)`: whether it worked.
    #[must_use]
    pub fn del(&self) -> bool {
        event_api::event_del(self.0)
    }

    /// `swoole_event_write(socket, data, len)`: the bytes taken, `None` when it failed
    /// or no reactor runs.
    #[must_use]
    pub fn write(&self, data: &[u8]) -> Option<usize> {
        if !reactor() {
            return None;
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
        self.release();
    }

    /// `void Socket::free()` of the record as it is: deletes the timers and buffers,
    /// then, while the thread's event loop is available, removes the record from it
    /// and defers the close to it; otherwise closes its fd (`-1` for none) and frees
    /// it at once. Outside the hybrid there are no timers, buffers or loop.
    pub fn release(self) {
        #[cfg(feature = "upstream-symbols")]
        {
            let record = self.0;
            // The buffers leave the record here and go below, after the timers and
            // outside the record's borrow: an entry's disposer runs as its buffer goes.
            let (timers, buffers) = record.edit(|record| {
                (
                    [record.recv_timer, record.output.send_timer],
                    [record.in_buffer.take(), record.output.out_buffer.take()],
                )
            });
            for timer in timers.into_iter().flatten() {
                swoole_rs_timer::del_id(timer);
            }
            // `delete in_buffer; delete out_buffer`.
            drop(buffers);
            if socket_upstream::event_reactor().is_some() {
                if record.read(|record| record.fd != -1 && !record.removed) {
                    // `swoole_event_del(this)` on the thread's reactor, which logs a
                    // failure.
                    _ = event_api::event_del(record);
                }
                // `swoole_event_defer(socket_free_defer, this)`: the loop may still read
                // the record through an event of this pass. A reactor freed before it
                // runs frees it all the same.
                match event_api::swoole_event_defer_value(FreeDeferred(self)) {
                    Ok(()) => return,
                    Err(FreeDeferred(this)) => return this.close_and_free(),
                }
            }
        }
        self.close_and_free();
    }

    /// base.cc's `socket_free_defer`: closes the descriptor and frees the record.
    fn close_and_free(self) {
        // SAFETY: the record this owns, the leaked box `adopt` (or the caller of
        // `socket_free`) gave it, which nothing uses afterwards.
        unsafe { Box::from_raw(self.0.as_ptr()) }.close_and_free();
    }
}

/// The record [`EventSocket::release`] leaves the loop to free.
#[cfg(feature = "upstream-symbols")]
struct FreeDeferred(EventSocket);

#[cfg(feature = "upstream-symbols")]
impl event_api::Deferred for FreeDeferred {
    fn run(self) {
        self.0.close_and_free();
    }

    fn dispose(self) {
        self.0.close_and_free();
    }
}

/// `void Socket::free()`: [`EventSocket::release`] of the record.
///
/// # Safety
/// `socket` is none or a record [`SocketRecord`] allocated, which its caller gives up as
/// an `EventSocket` owns one: nothing uses it afterwards.
#[cfg_attr(
    feature = "upstream-symbols",
    unsafe(export_name = "_ZN6swoole7network6Socket4freeEv")
)]
pub unsafe extern "C-unwind" fn socket_free(socket: Option<Socket<'static>>) {
    if let Some(record) = socket {
        EventSocket(record).release();
    }
}
