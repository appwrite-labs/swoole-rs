// SPDX-License-Identifier: Apache-2.0
// Compatibility derived from Swoole v6.2.4 src/reactor/base.cc (Swoole contributors).
//! The reactor's write path from base.cc (`Reactor::_write`, `_writev`,
//! `_writable_callback` and `drain_write_buffer`), defined under their upstream
//! symbols over the Rust socket methods. `swoole_rs_reactor`'s
//! `swr_v1_reactor_write`, `_flush` and `_drain` decide; these are their effects.
use crate::upstream::{
    send_drain, socket_free, socket_handle_send, socket_handle_sendfile, writev,
};
use std::cell::Cell;
use std::ffi::c_int;
use std::io::IoSlice;
use swoole_rs_corebase::CCast;
use swoole_rs_corebase::log;
use swoole_rs_reactor::event_api::Writers;
use swoole_rs_reactor::reactor_upstream::{interest, reactor_interest, trigger_close_event};
use swoole_rs_reactor::socket_upstream::socket_set_fd_option;
use swoole_rs_reactor::{
    Drain, Event, FlushEffects, Interest, OwnedSocket, Running, SOCKET_DEFAULTS, Socket,
    WriteEffects, WriteError, WritePayload,
};

const SW_ERROR_OPERATION_NOT_SUPPORT: c_int =
    swoole_rs_error::ErrorCode::SW_ERROR_OPERATION_NOT_SUPPORT as c_int;
const SW_ERROR_OUTPUT_BUFFER_OVERFLOW: c_int =
    swoole_rs_error::ErrorCode::SW_ERROR_OUTPUT_BUFFER_OVERFLOW as c_int;
const SW_ERROR_PACKAGE_LENGTH_TOO_LARGE: c_int =
    swoole_rs_error::ErrorCode::SW_ERROR_PACKAGE_LENGTH_TOO_LARGE as c_int;

/// One write: the reactor, the socket, never freed while registered, and the payload.
struct Write<'a> {
    reactor: Running<'a>,
    socket: Socket<'static>,
    payload: WritePayload<'a>,
}

/// One flush or drain: the reactor, the socket, never freed while registered, and its
/// event, which the close handler may change and the caller then sees.
struct Flush<'a> {
    reactor: Running<'a>,
    socket: Socket<'static>,
    event: &'a Cell<Event>,
}

impl WriteEffects for Write<'_> {
    fn set_nonblock(&self) {
        self.socket
            .edit(|record| socket_set_fd_option(record, 1, -1));
    }

    fn send(&self) -> Option<usize> {
        match self.payload {
            // `socket->send(buf, n, 0)`.
            WritePayload::Bytes(data) => send_drain(self.socket, data, 0),
            // `socket->writev(iov, iovcnt)`.
            WritePayload::Vectors(vectors) => writev(self.socket.fd(), vectors),
        }
    }

    fn create_buffer(&self) {
        let socket = self.socket;
        let chunk_size = socket.read(|record| record.chunk_size);
        let buffer = swoole_rs_buffer::cpp::new_buffer(chunk_size);
        socket.edit(|record| record.output.out_buffer = Some(buffer));
    }

    fn watch_writable(&self) {
        if !self
            .socket
            .read(|record| record.events)
            .intersects(swoole_rs_reactor::Events::WRITE)
        {
            // `reactor->add_write_event(socket)`, which logs a failure.
            _ = interest(
                self.reactor,
                self.socket,
                Interest::AddWrite,
                swoole_rs_reactor::Events::NONE,
            );
        }
    }
}

/// `Reactor::write_func`.
fn write(
    reactor: Running<'_>,
    socket: Socket<'static>,
    payload: WritePayload<'_>,
) -> Option<usize> {
    let ops = Write {
        reactor,
        socket,
        payload,
    };
    let default_size = SOCKET_DEFAULTS.buffer_size();
    let result = swoole_rs_reactor::swr_v1_reactor_write(socket, default_size, payload, &ops);
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
            let (fd, size) = socket.read(|record| (record.fd, record.output.buffer_size));
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
pub fn reactor_write(reactor: Running<'_>, socket: Socket<'static>, data: &[u8]) -> Option<usize> {
    write(reactor, socket, WritePayload::Bytes(Drain::new(data)))
}

/// `ssize_t Reactor::_writev(Reactor *, Socket *, const iovec *iov, size_t iovcnt)`:
/// not over TLS.
#[must_use]
pub fn reactor_writev(
    reactor: Running<'_>,
    socket: Socket<'static>,
    vectors: &[IoSlice<'_>],
) -> Option<usize> {
    if !socket.read(swoole_rs_reactor::SocketFields::ssl).is_null() {
        log::error_log(
            log::LogLevel::SW_LOG_WARNING,
            SW_ERROR_OPERATION_NOT_SUPPORT,
            c"Reactor::_writev",
            format_args!("does not support SSL"),
        );
        return None;
    }
    write(reactor, socket, WritePayload::Vectors(vectors))
}

/// The reactor's writes, which `swoole_init` registers
/// (`swoole_rs_reactor::event_api::set_writers`).
pub const WRITERS: Writers = Writers {
    write: reactor_write,
    writev: reactor_writev,
};

impl FlushEffects for Flush<'_> {
    fn handle_send(&self) -> bool {
        socket_handle_send(self.socket)
    }

    fn handle_sendfile(&self) -> bool {
        socket_handle_sendfile(self.socket)
    }

    fn close(&self) -> bool {
        // `reactor->close`, set at construction.
        let Some(close) = self.reactor.close() else {
            return false;
        };
        // SAFETY: `reactor->close(reactor, socket)`: the queued close of the flushed
        // socket, a heap record of the reactor's, which nothing uses after it.
        close(self.reactor, unsafe {
            OwnedSocket::from_socket(self.socket)
        }) >= 0
    }

    fn trigger_close(&self) -> bool {
        let mut event = self.event.get();
        let result = trigger_close_event(self.reactor, &mut event);
        self.event.set(event);
        result
    }

    fn delete_timer(&self, timer: swoole_rs_timer::Held) {
        swoole_rs_timer::del_id(timer);
    }

    fn remove_write_event(&self) {
        // `reactor->remove_write_event(socket)` on the flush's live reactor.
        _ = reactor_interest(
            self.reactor,
            Some(self.socket),
            Interest::RemoveWrite.code(),
            0,
        );
    }

    fn wait_writable(&self) -> bool {
        let timeout = SOCKET_DEFAULTS.write.get();
        let timeout = (timeout * 1000.0).c_cast::<c_int>();
        self.socket
            .wait_event(timeout, swoole_rs_reactor::Events::WRITE)
    }
}

/// `int Reactor::_writable_callback(Reactor *reactor, Event *ev)`: a reactor handler.
#[cfg_attr(
    feature = "upstream-symbols",
    unsafe(export_name = "_ZN6swoole7Reactor18_writable_callbackEPS0_PNS_5EventE")
)]
pub extern "C-unwind" fn reactor_writable_callback(
    reactor: Running<'_>,
    event: &mut Event,
) -> c_int {
    let lent = Cell::new(*event);
    let Some(socket) = event.socket() else {
        return -1;
    };
    let flush = Flush {
        reactor,
        socket,
        event: &lent,
    };
    let flushed = swoole_rs_reactor::swr_v1_reactor_flush(socket, &flush);
    *event = lent.get();
    if flushed { 0 } else { -1 }
}

/// `void Reactor::drain_write_buffer(Socket *socket)`.
pub fn reactor_drain_write_buffer(reactor: Running<'_>, record: Socket<'static>) {
    let event = Cell::new(Event::of(record.fd(), 0, 0, Some(record)));
    let flush = Flush {
        reactor,
        socket: record,
        event: &event,
    };
    swoole_rs_reactor::swr_v1_reactor_drain(record, &flush);
}

/// `int Reactor::_close(Reactor *reactor, Socket *socket)`: frees the socket.
#[cfg_attr(
    feature = "upstream-symbols",
    unsafe(export_name = "_ZN6swoole7Reactor6_closeEPS0_PNS_7network6SocketE")
)]
#[must_use]
pub extern "C-unwind" fn reactor_close(_reactor: Running<'_>, socket: OwnedSocket) -> c_int {
    socket_free(Some(socket));
    0
}
