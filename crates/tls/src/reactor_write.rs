// SPDX-License-Identifier: Apache-2.0
// Compatibility derived from Swoole v6.2.4 src/reactor/base.cc (Swoole contributors).
//! The reactor's write path from base.cc (`Reactor::_write`, `_writev`,
//! `_writable_callback` and `drain_write_buffer`), defined under their upstream
//! symbols over the Rust socket methods. `swoole_rs_reactor`'s
//! `swr_v1_reactor_write`, `_flush` and `_drain` decide; these are their effects.
use crate::upstream::{socket_free, socket_handle_send, socket_handle_sendfile, socket_send};
use std::ffi::{c_int, c_void};
use swoole_rs_corebase::log;
use swoole_rs_reactor::reactor_upstream::{prepared, reactor_interest, trigger_close_event};
use swoole_rs_reactor::socket_upstream::socket_set_fd_option;
use swoole_rs_reactor::{
    Event, FlushEffects, SOCKET_DEFAULTS, Socket, SocketRecord, WriteEffects, WritePayload,
};

const SW_ERROR_OPERATION_NOT_SUPPORT: c_int = 507;
const SW_ERROR_OUTPUT_BUFFER_OVERFLOW: c_int = 1009;
const SW_ERROR_PACKAGE_LENGTH_TOO_LARGE: c_int = 1201;
const SW_EVENT_WRITE: c_int = 1 << 10;
/// `Reactor::interest` kinds.
const ADD_WRITE: u32 = 3;
const REMOVE_WRITE: u32 = 5;

/// One write: the reactor, the socket and the payload. Only [`write`], whose caller
/// promises a live reactor, builds one, so it stays live while it does.
struct Write<'a> {
    reactor: *mut c_void,
    socket: Socket<'a>,
    payload: &'a WritePayload,
}

/// One flush or drain: the reactor, the socket and its event. Only the writable
/// callback and the drain, whose callers promise the reactor and event live, build one.
struct Flush<'a> {
    reactor: *mut c_void,
    socket: Socket<'a>,
    event: *mut Event,
}

impl WriteEffects for Write<'_> {
    fn set_nonblock(&self) {
        self.socket
            .edit(|record| socket_set_fd_option(record, 1, -1));
    }

    fn send(&self) -> isize {
        let payload = self.payload;
        let socket = self.socket;
        if !payload.data.is_null() {
            // SAFETY: `socket->send(buf, n, 0)` over the payload's bytes.
            return unsafe { socket_send(socket.as_ptr(), payload.data.cast(), payload.length, 0) };
        }
        let fd = socket.fd();
        let count = c_int::try_from(payload.count).unwrap_or(c_int::MAX);
        // SAFETY: `socket->writev(iov, iovcnt)` over the payload's vector.
        unsafe { libc::writev(fd, payload.iov, count) }
    }

    fn create_buffer(&self) {
        let socket = self.socket;
        let chunk_size = socket.read(|record| record.chunk_size);
        let buffer = swoole_rs_buffer::cpp::new_buffer(chunk_size);
        socket.edit(|record| record.output.out_buffer = buffer.cast());
    }

    fn watch_writable(&self) {
        if self.socket.read(|record| record.events) & SW_EVENT_WRITE == 0 {
            // SAFETY: `reactor->add_write_event(socket)` on the live reactor.
            unsafe { reactor_interest(self.reactor, self.socket.as_ptr(), ADD_WRITE, 0) };
        }
    }
}

/// `Reactor::write_func`.
///
/// # Safety
/// `reactor` is a live `Reactor` and `payload` valid.
unsafe fn write(reactor: *mut c_void, socket: Socket<'_>, payload: &WritePayload) -> isize {
    let ops = Write {
        reactor,
        socket,
        payload,
    };
    let default_size = SOCKET_DEFAULTS.buffer_size();
    // SAFETY: as the caller promises.
    let result =
        unsafe { swoole_rs_reactor::swr_v1_reactor_write(socket, default_size, payload, &ops) };
    let origin = c"Reactor::write_func";
    match result.outcome {
        0 => return result.value,
        1 => log::error_log(
            log::LOG_WARNING,
            SW_ERROR_PACKAGE_LENGTH_TOO_LARGE,
            origin,
            format_args!("data packet is too large, cannot exceed the socket buffer size"),
        ),
        2 => {
            let queued = result.value;
            let (fd, size) = socket.read(|record| (record.fd, record.output.buffer_size));
            log::error_log(
                log::LOG_WARNING,
                SW_ERROR_OUTPUT_BUFFER_OVERFLOW,
                origin,
                format_args!("socket#{fd} output buffer overflow: ({queued}/{size})"),
            );
        }
        _ => log::set_last_error(std::io::Error::last_os_error().raw_os_error().unwrap_or(0)),
    }
    -1
}

/// `ssize_t Reactor::_write(Reactor *, Socket *, const void *buf, size_t n)`.
///
/// # Safety
/// `reactor` is a live `Reactor`, `socket` a live record and `data` holds `length` bytes.
#[cfg_attr(
    feature = "upstream-symbols",
    unsafe(export_name = "_ZN6swoole7Reactor6_writeEPS0_PNS_7network6SocketEPKvm")
)]
pub unsafe extern "C-unwind" fn reactor_write(
    reactor: *mut c_void,
    socket: *mut SocketRecord,
    data: *const c_void,
    length: usize,
) -> isize {
    // SAFETY: as the caller promises.
    let Some(socket) = (unsafe { Socket::from_ptr(socket) }) else {
        return -1;
    };
    let payload = WritePayload {
        data: data.cast(),
        iov: std::ptr::null(),
        count: 0,
        length,
    };
    // SAFETY: as the caller promises.
    unsafe { write(reactor, socket, &payload) }
}

/// `ssize_t Reactor::_writev(Reactor *, Socket *, const iovec *iov, size_t iovcnt)`:
/// not over TLS.
///
/// # Safety
/// `reactor` is a live `Reactor`, `socket` a live record and `iov` holds `count` entries.
#[cfg_attr(
    feature = "upstream-symbols",
    unsafe(export_name = "_ZN6swoole7Reactor7_writevEPS0_PNS_7network6SocketEPK5iovecm")
)]
pub unsafe extern "C-unwind" fn reactor_writev(
    reactor: *mut c_void,
    socket: *mut SocketRecord,
    iov: *const libc::iovec,
    count: usize,
) -> isize {
    // SAFETY: as the caller promises.
    let Some(socket) = (unsafe { Socket::from_ptr(socket) }) else {
        return -1;
    };
    if !socket.read(swoole_rs_reactor::SocketFields::ssl).is_null() {
        log::error_log(
            log::LOG_WARNING,
            SW_ERROR_OPERATION_NOT_SUPPORT,
            c"Reactor::_writev",
            format_args!("does not support SSL"),
        );
        return -1;
    }
    let length = if count == 0 {
        0
    } else {
        // SAFETY: as the caller promises.
        unsafe { std::slice::from_raw_parts(iov, count) }
            .iter()
            .map(|entry| entry.iov_len)
            .sum()
    };
    let payload = WritePayload {
        data: std::ptr::null(),
        iov,
        count,
        length,
    };
    // SAFETY: as the caller promises.
    unsafe { write(reactor, socket, &payload) }
}

impl FlushEffects for Flush<'_> {
    fn handle_send(&self) -> c_int {
        socket_handle_send(self.socket)
    }

    fn handle_sendfile(&self) -> c_int {
        socket_handle_sendfile(self.socket)
    }

    fn close(&self) -> c_int {
        // SAFETY: the flush's live reactor.
        let fields = unsafe { prepared(self.reactor) }.fields;
        // SAFETY: `reactor->close`, set at construction.
        let Some(close) = (unsafe { (*fields).close }) else {
            return -1;
        };
        // SAFETY: `reactor->close(reactor, socket)`.
        unsafe { close(self.reactor, self.socket.as_ptr()) }
    }

    fn trigger_close(&self) -> c_int {
        // SAFETY: the flush's live reactor and event.
        unsafe { trigger_close_event(self.reactor, self.event.cast()) }
    }

    fn delete_timer(&self) {
        let timer = self.socket.read(|record| record.output.send_timer);
        // SAFETY: `swoole_timer_del(socket->send_timer)`.
        unsafe { swoole_rs_timer::swoole_timer_del(timer.cast()) };
    }

    fn remove_write_event(&self) {
        // SAFETY: `reactor->remove_write_event(socket)` on the flush's live reactor.
        unsafe { reactor_interest(self.reactor, self.socket.as_ptr(), REMOVE_WRITE, 0) };
    }

    fn wait_writable(&self) -> c_int {
        let timeout = SOCKET_DEFAULTS.write.get();
        #[expect(
            clippy::cast_possible_truncation,
            reason = "upstream's static_cast<int>(sec2msec(...))"
        )]
        let timeout = (timeout * 1000.0) as c_int;
        self.socket.wait_event(timeout, SW_EVENT_WRITE)
    }
}

/// `int Reactor::_writable_callback(Reactor *reactor, Event *ev)`.
///
/// # Safety
/// `reactor` is a live `Reactor` and `event` a live event naming a live socket.
#[cfg_attr(
    feature = "upstream-symbols",
    unsafe(export_name = "_ZN6swoole7Reactor18_writable_callbackEPS0_PNS_5EventE")
)]
pub unsafe extern "C-unwind" fn reactor_writable_callback(
    reactor: *mut c_void,
    event: *mut Event,
) -> c_int {
    // SAFETY: as the caller promises.
    let socket = unsafe { (*event).socket_ptr() }.cast::<SocketRecord>();
    // SAFETY: as the caller promises.
    let Some(socket) = (unsafe { Socket::from_ptr(socket) }) else {
        return -1;
    };
    let flush = Flush {
        reactor,
        socket,
        event,
    };
    swoole_rs_reactor::swr_v1_reactor_flush(socket, &flush)
}

/// `void Reactor::drain_write_buffer(Socket *socket)`.
///
/// # Safety
/// `reactor` is a live `Reactor` and `socket` a live record.
#[cfg_attr(
    feature = "upstream-symbols",
    unsafe(export_name = "_ZN6swoole7Reactor18drain_write_bufferEPNS_7network6SocketE")
)]
pub unsafe extern "C-unwind" fn reactor_drain_write_buffer(
    reactor: *mut c_void,
    socket: *mut SocketRecord,
) {
    // SAFETY: as the caller promises.
    let Some(record) = (unsafe { Socket::from_ptr(socket) }) else {
        return;
    };
    // SAFETY: `record` proved the socket live, and the event lives for this call.
    let mut event = unsafe { Event::new(record.fd(), 0, 0, socket.cast()) };
    let flush = Flush {
        reactor,
        socket: record,
        event: &raw mut event,
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
