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
use std::ffi::{c_int, c_void};
use std::io::IoSlice;
use swoole_rs_corebase::CCast;
use swoole_rs_corebase::log;
use swoole_rs_reactor::reactor_upstream::{interest, reactor_interest, trigger_close_event};
use swoole_rs_reactor::socket_upstream::socket_set_fd_option;
use swoole_rs_reactor::{
    Drain, Event, FlushEffects, Interest, Running, SOCKET_DEFAULTS, Socket, SocketRecord,
    WriteEffects, WriteError, WritePayload,
};

const SW_ERROR_OPERATION_NOT_SUPPORT: c_int = 507;
const SW_ERROR_OUTPUT_BUFFER_OVERFLOW: c_int = 1009;
const SW_ERROR_PACKAGE_LENGTH_TOO_LARGE: c_int = 1201;
const SW_EVENT_WRITE: c_int = swoole_rs_corebase::constants::EventType::SW_EVENT_WRITE as c_int;

/// One write: the reactor, the socket, never freed while registered, and the payload.
struct Write<'a> {
    reactor: Running<'a>,
    socket: Socket<'static>,
    payload: WritePayload<'a>,
}

/// One flush or drain: the reactor, the socket and its event, which the close
/// handler may change and the caller then sees.
struct Flush<'a> {
    reactor: Running<'a>,
    socket: Socket<'a>,
    event: &'a Cell<Event>,
}

impl WriteEffects for Write<'_> {
    fn set_nonblock(&self) {
        self.socket
            .edit(|record| socket_set_fd_option(record, 1, -1));
    }

    fn send(&self) -> isize {
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
        if self.socket.read(|record| record.events) & SW_EVENT_WRITE == 0 {
            // `reactor->add_write_event(socket)`, which logs a failure.
            _ = interest(self.reactor, self.socket, Interest::AddWrite, 0);
        }
    }
}

/// `Reactor::write_func`.
fn write(reactor: Running<'_>, socket: Socket<'static>, payload: WritePayload<'_>) -> isize {
    let ops = Write {
        reactor,
        socket,
        payload,
    };
    let default_size = SOCKET_DEFAULTS.buffer_size();
    let result = swoole_rs_reactor::swr_v1_reactor_write(socket, default_size, payload, &ops);
    let origin = c"Reactor::write_func";
    match result {
        Ok(value) => return value,
        Err(WriteError::TooLarge) => log::error_log(
            log::LOG_WARNING,
            SW_ERROR_PACKAGE_LENGTH_TOO_LARGE,
            origin,
            format_args!("data packet is too large, cannot exceed the socket buffer size"),
        ),
        Err(WriteError::Overflow(queued)) => {
            let (fd, size) = socket.read(|record| (record.fd, record.output.buffer_size));
            log::error_log(
                log::LOG_WARNING,
                SW_ERROR_OUTPUT_BUFFER_OVERFLOW,
                origin,
                format_args!("socket#{fd} output buffer overflow: ({queued}/{size})"),
            );
        }
        Err(WriteError::Send) => {
            log::set_last_error(std::io::Error::last_os_error().raw_os_error().unwrap_or(0));
        }
    }
    -1
}

/// `ssize_t Reactor::_write(Reactor *, Socket *, const void *buf, size_t n)`.
///
/// # Safety
/// `reactor` is a live `Reactor`, `socket` stays live while it is registered, and
/// `data` holds `length` bytes.
#[cfg_attr(
    feature = "upstream-symbols",
    unsafe(export_name = "_ZN6swoole7Reactor6_writeEPS0_PNS_7network6SocketEPKvm")
)]
#[must_use]
pub unsafe extern "C-unwind" fn reactor_write(
    reactor: Running<'_>,
    socket: Socket<'static>,
    data: *const c_void,
    length: usize,
) -> isize {
    // SAFETY: as the caller promises.
    let data = unsafe { Drain::from_raw(data, length) };
    write(reactor, socket, WritePayload::Bytes(data))
}

/// `ssize_t Reactor::_writev(Reactor *, Socket *, const iovec *iov, size_t iovcnt)`:
/// not over TLS.
///
/// # Safety
/// As [`reactor_write`], with `iov` holding `count` entries.
#[cfg_attr(
    feature = "upstream-symbols",
    unsafe(export_name = "_ZN6swoole7Reactor7_writevEPS0_PNS_7network6SocketEPK5iovecm")
)]
pub unsafe extern "C-unwind" fn reactor_writev(
    reactor: Running<'_>,
    socket: Socket<'static>,
    iov: *const libc::iovec,
    count: usize,
) -> isize {
    if !socket.read(swoole_rs_reactor::SocketFields::ssl).is_null() {
        log::error_log(
            log::LOG_WARNING,
            SW_ERROR_OPERATION_NOT_SUPPORT,
            c"Reactor::_writev",
            format_args!("does not support SSL"),
        );
        return -1;
    }
    if count == 0 {
        return write(reactor, socket, WritePayload::Vectors(&[]));
    }
    // SAFETY: as the caller promises: `count` iovecs.
    let raw = unsafe { std::slice::from_raw_parts(iov, count) };
    if raw.iter().all(|part| !part.iov_base.is_null()) {
        // SAFETY: as the caller promises: iovecs over readable bytes, none at null,
        // which an `IoSlice` is laid out as.
        let vectors = unsafe { std::slice::from_raw_parts(iov.cast::<IoSlice<'_>>(), count) };
        return write(reactor, socket, WritePayload::Vectors(vectors));
    }
    // A `{NULL, 0}` part is empty; a null base with a length is refused.
    if raw
        .iter()
        .any(|part| part.iov_base.is_null() && part.iov_len != 0)
    {
        return -1;
    }
    let vectors: Vec<IoSlice<'_>> = raw
        .iter()
        .map(|part| {
            if part.iov_base.is_null() {
                return IoSlice::new(&[]);
            }
            // SAFETY: as the caller promises: `iov_len` readable bytes at a non-null base.
            IoSlice::new(unsafe { std::slice::from_raw_parts(part.iov_base.cast(), part.iov_len) })
        })
        .collect();
    write(reactor, socket, WritePayload::Vectors(&vectors))
}

impl FlushEffects for Flush<'_> {
    fn handle_send(&self) -> bool {
        socket_handle_send(self.socket)
    }

    fn handle_sendfile(&self) -> bool {
        socket_handle_sendfile(self.socket)
    }

    fn close(&self) -> c_int {
        // `reactor->close`, set at construction.
        let Some(close) = self.reactor.close() else {
            return -1;
        };
        // SAFETY: `reactor->close(reactor, socket)`.
        unsafe { close(self.reactor.as_ptr(), self.socket.as_ptr()) }
    }

    fn trigger_close(&self) -> c_int {
        let mut event = self.event.get();
        let result = trigger_close_event(self.reactor, &mut event);
        self.event.set(event);
        result
    }

    fn delete_timer(&self, timer: swoole_rs_timer::Held) {
        swoole_rs_timer::del_id(timer);
    }

    fn remove_write_event(&self) {
        // SAFETY: `reactor->remove_write_event(socket)` on the flush's live reactor.
        unsafe {
            reactor_interest(
                self.reactor.as_ptr(),
                self.socket.as_ptr(),
                Interest::RemoveWrite.code(),
                0,
            )
        };
    }

    fn wait_writable(&self) -> bool {
        let timeout = SOCKET_DEFAULTS.write.get();
        let timeout = (timeout * 1000.0).c_cast::<c_int>();
        self.socket.wait_event(timeout, SW_EVENT_WRITE)
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
    let result = swoole_rs_reactor::swr_v1_reactor_flush(socket, &flush);
    *event = lent.get();
    result
}

/// `void Reactor::drain_write_buffer(Socket *socket)`.
#[cfg_attr(
    feature = "upstream-symbols",
    unsafe(export_name = "_ZN6swoole7Reactor18drain_write_bufferEPNS_7network6SocketE")
)]
pub extern "C-unwind" fn reactor_drain_write_buffer(reactor: Running<'_>, record: Socket<'_>) {
    // SAFETY: a live socket, and the event lives for this call.
    let event = Cell::new(unsafe { Event::new(record.fd(), 0, 0, record.as_ptr().cast()) });
    let flush = Flush {
        reactor,
        socket: record,
        event: &event,
    };
    swoole_rs_reactor::swr_v1_reactor_drain(record, &flush);
}

/// `int Reactor::_close(Reactor *reactor, Socket *socket)`: frees the socket.
///
/// # Safety
/// `socket` is a live record nothing uses afterwards.
#[cfg_attr(
    feature = "upstream-symbols",
    unsafe(export_name = "_ZN6swoole7Reactor6_closeEPS0_PNS_7network6SocketE")
)]
pub unsafe extern "C-unwind" fn reactor_close(
    _reactor: *mut c_void,
    socket: *mut SocketRecord,
) -> c_int {
    // SAFETY: as the caller promises.
    unsafe { socket_free(socket) };
    0
}
