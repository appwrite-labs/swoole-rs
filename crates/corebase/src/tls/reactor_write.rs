// SPDX-License-Identifier: Apache-2.0
// Compatibility derived from Swoole v6.2.4 src/reactor/base.cc (Swoole contributors).
//! The reactor's write path from base.cc (`Reactor::_write`, `_writev`,
//! `_writable_callback` and `drain_write_buffer`), defined under their upstream
//! symbols over the Rust socket methods. `swoole_rs_reactor`'s
//! `swr_v1_reactor_write`, `_flush` and `_drain` decide; these are their effects.
use crate::CCast;
use crate::log;
use crate::reactor::reactor_upstream::{interest, trigger_close_event};
use crate::reactor::{
    Drain, Event, FlushEffects, Heap, Interest, OwnedSocket, Running, SOCKET_DEFAULTS,
    SocketFields, WriteEffects, WriteError, WritePayload,
};
use crate::tls::upstream::{send_drain, socket_free, socket_handle_send, socket_handle_sendfile};
use std::cell::Cell;
use std::ffi::c_int;
use std::io::IoSlice;

const SW_ERROR_OPERATION_NOT_SUPPORT: c_int =
    swoole_rs_error::ErrorCode::SW_ERROR_OPERATION_NOT_SUPPORT as c_int;
const SW_ERROR_OUTPUT_BUFFER_OVERFLOW: c_int =
    swoole_rs_error::ErrorCode::SW_ERROR_OUTPUT_BUFFER_OVERFLOW as c_int;
const SW_ERROR_PACKAGE_LENGTH_TOO_LARGE: c_int =
    swoole_rs_error::ErrorCode::SW_ERROR_PACKAGE_LENGTH_TOO_LARGE as c_int;

/// One write: the reactor, the heap socket and the payload.
struct Write<'a, F> {
    reactor: Running<'a>,
    socket: Heap<'a>,
    send: F,
}

/// One flush or drain: the reactor, the registered socket and its event, which the
/// close handler may change and the caller then sees.
struct Flush<'a> {
    reactor: Running<'a>,
    socket: Heap<'static>,
    event: &'a Cell<Event>,
}

impl<F: Fn() -> Option<usize>> WriteEffects for Write<'_, F> {
    fn set_nonblock(&self) {
        self.socket
            .edit(|record| record.set_fd_option_or_warn(1, -1));
    }

    fn send(&self) -> Option<usize> {
        (self.send)()
    }

    fn create_buffer(&self) {
        self.socket
            .edit(crate::reactor::SocketFields::open_chunked_output_queue);
    }

    fn watch_writable(&self) {
        if !self
            .socket
            .read(|record| record.events)
            .intersects(crate::reactor::Events::WRITE)
        {
            // `reactor->add_write_event(socket)`, which logs a failure.
            _ = interest(
                self.reactor,
                self.socket,
                Interest::AddWrite,
                crate::reactor::Events::NONE,
            );
        }
    }
}

/// `Reactor::write_func`: the bytes written or queued, or `None` once the failure is
/// logged or set as the last error.
fn write(
    reactor: Running<'_>,
    socket: Heap<'_>,
    payload: WritePayload<'_>,
    send: impl Fn() -> Option<usize>,
) -> Option<usize> {
    let ops = Write {
        reactor,
        socket,
        send,
    };
    let default_size = SOCKET_DEFAULTS.buffer_size();
    let result = crate::reactor::swr_v1_reactor_write(*socket, default_size, payload, &ops);
    let origin = c"Reactor::write_func";
    match result {
        Ok(value) => return Some(value),
        Err(WriteError::TooLarge) => log::error_log(
            log::LogLevel::SW_LOG_WARNING,
            SW_ERROR_PACKAGE_LENGTH_TOO_LARGE,
            origin,
            format_args!("data packet is too large, cannot exceed the socket buffer size"),
        ),
        Err(WriteError::Overflow(queued)) => {
            let (fd, size) = (socket.fd(), socket.read(SocketFields::output_limit));
            log::error_log(
                log::LogLevel::SW_LOG_WARNING,
                SW_ERROR_OUTPUT_BUFFER_OVERFLOW,
                origin,
                format_args!("socket#{fd} output buffer overflow: ({queued}/{size})"),
            );
        }
        Err(WriteError::Send) => {
            log::set_last_error(std::io::Error::last_os_error().raw_os_error().unwrap_or(0));
        }
    }
    None
}

/// `ssize_t Reactor::_write(Reactor *, Socket *, const void *buf, size_t n)`.
#[must_use]
pub fn reactor_write(reactor: Running<'_>, socket: Heap<'_>, data: &[u8]) -> Option<usize> {
    let data = Drain::new(data);
    write(reactor, socket, WritePayload::Bytes(data), || {
        send_drain(*socket, data, 0)
    })
}

/// `ssize_t Reactor::_writev(Reactor *, Socket *, const iovec *iov, size_t iovcnt)`:
/// not over TLS.
#[must_use]
pub fn reactor_writev(
    reactor: Running<'_>,
    socket: Heap<'_>,
    vectors: &[IoSlice<'_>],
) -> Option<usize> {
    if !socket.read(SocketFields::ssl).is_null() {
        log::error_log(
            log::LogLevel::SW_LOG_WARNING,
            SW_ERROR_OPERATION_NOT_SUPPORT,
            c"Reactor::_writev",
            format_args!("does not support SSL"),
        );
        return None;
    }
    write(reactor, socket, WritePayload::Vectors(vectors), || {
        socket.read(|record| record.writev(vectors))
    })
}

impl FlushEffects for Flush<'_> {
    fn handle_send(&self) -> bool {
        socket_handle_send(*self.socket)
    }

    fn handle_sendfile(&self) -> bool {
        socket_handle_sendfile(*self.socket)
    }

    fn close(&self) -> bool {
        // `reactor->close`, set at construction.
        let Some(close) = self.reactor.close() else {
            return false;
        };
        // SAFETY: `reactor->close(reactor, socket)`: the queued close of the flushed
        // socket, a heap record of the reactor's, which nothing uses after it.
        close(self.reactor, unsafe {
            OwnedSocket::from_socket(*self.socket)
        }) >= 0
    }

    fn trigger_close(&self) -> bool {
        let mut event = self.event.get();
        let result = trigger_close_event(self.reactor, &mut event);
        self.event.set(event);
        result
    }

    fn delete_timer(&self, timer: crate::timer::Held) {
        crate::timer::del_id(timer);
    }

    fn remove_write_event(&self) {
        // `reactor->remove_write_event(socket)` on the flush's live reactor.
        _ = interest(
            self.reactor,
            self.socket,
            Interest::RemoveWrite,
            crate::reactor::Events::NONE,
        );
    }

    fn wait_writable(&self) -> bool {
        let timeout = SOCKET_DEFAULTS.write.get();
        let timeout = (timeout * 1000.0).c_cast::<c_int>();
        self.socket
            .wait_event(timeout, crate::reactor::Events::WRITE)
    }
}

/// `int Reactor::_writable_callback(Reactor *reactor, Event *ev)`: a reactor handler.
pub fn reactor_writable_callback(reactor: Running<'_>, event: &mut Event) -> bool {
    let lent = Cell::new(*event);
    let Some(socket) = event.socket() else {
        return false;
    };
    let flush = Flush {
        reactor,
        socket,
        event: &lent,
    };
    let flushed = crate::reactor::swr_v1_reactor_flush(*socket, &flush);
    *event = lent.get();
    flushed
}

/// `void Reactor::drain_write_buffer(Socket *socket)`. The event names `record` as
/// the registry does, for this call.
pub fn reactor_drain_write_buffer(reactor: Running<'_>, record: Heap<'_>) {
    let record = record.named();
    let event = Cell::new(Event::of(record.fd(), 0, 0, Some(record)));
    let flush = Flush {
        reactor,
        socket: record,
        event: &event,
    };
    crate::reactor::swr_v1_reactor_drain(*record, &flush);
}

/// `int Reactor::_close(Reactor *reactor, Socket *socket)`: frees the socket.
#[must_use]
pub extern "C-unwind" fn reactor_close(_reactor: Running<'_>, socket: OwnedSocket) -> c_int {
    socket_free(Some(socket));
    0
}
