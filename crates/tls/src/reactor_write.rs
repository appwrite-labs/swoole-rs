// SPDX-License-Identifier: Apache-2.0
// Compatibility derived from Swoole v6.2.4 src/reactor/base.cc (Swoole contributors).
//! The reactor's write path from base.cc (`Reactor::_write`, `_writev`,
//! `_writable_callback` and `drain_write_buffer`), defined under their upstream
//! symbols over the Rust socket methods. `swoole_rs_reactor`'s
//! `swr_v1_reactor_write`, `_flush` and `_drain` decide; these are their effects.
use crate::upstream::{
    buffer_check, out_queue, socket_free, socket_handle_send, socket_handle_sendfile, socket_send,
};
use std::ffi::{c_int, c_void};
use swoole_rs_buffer::Buffer;
use swoole_rs_corebase::log;
use swoole_rs_reactor::reactor_upstream::{prepared, reactor_interest, trigger_close_event};
use swoole_rs_reactor::socket_upstream::{socket_set_fd_option, socket_wait_event};
use swoole_rs_reactor::{
    Event, FlushEffects, SocketOutput, SocketRecord, WriteEffects, WritePayload,
    swr_v1_socket_defaults,
};

const SW_ERROR_OPERATION_NOT_SUPPORT: c_int = 507;
const SW_ERROR_OUTPUT_BUFFER_OVERFLOW: c_int = 1009;
const SW_ERROR_PACKAGE_LENGTH_TOO_LARGE: c_int = 1201;
const SW_EVENT_WRITE: c_int = 1 << 10;
/// `Reactor::interest` kinds.
const ADD_WRITE: u32 = 3;
const REMOVE_WRITE: u32 = 5;

/// One write: the reactor, the socket and the payload. Only [`write`], whose caller
/// promises a live reactor and socket, builds one, so both stay live while it does.
struct Write<'a> {
    reactor: *mut c_void,
    socket: *mut SocketRecord,
    payload: &'a WritePayload,
}

/// One flush or drain: the reactor, the socket and its event. Only the writable
/// callback and the drain, whose callers promise all three live, build one.
struct Flush {
    reactor: *mut c_void,
    socket: *mut SocketRecord,
    event: *mut Event,
}

/// The socket's output block and queue (null without an output buffer).
///
/// # Safety
/// `socket` is a live record.
unsafe fn output(socket: *mut SocketRecord) -> (*mut SocketOutput, *mut Buffer) {
    // SAFETY: as the caller promises.
    let record = unsafe { &mut *socket };
    let queue = if record.output.out_buffer.is_null() {
        std::ptr::null_mut()
    } else {
        // SAFETY: the socket's output buffer.
        unsafe { out_queue(socket) }
    };
    (&raw mut record.output, queue)
}

impl WriteEffects for Write<'_> {
    fn set_nonblock(&self) {
        // SAFETY: the write's live socket.
        socket_set_fd_option(unsafe { &mut *self.socket }, 1, -1);
    }

    fn send(&self) -> isize {
        let payload = self.payload;
        if !payload.data.is_null() {
            // SAFETY: `socket->send(buf, n, 0)` over the payload's bytes.
            return unsafe { socket_send(self.socket, payload.data.cast(), payload.length, 0) };
        }
        // SAFETY: the write's live socket.
        let fd = unsafe { (*self.socket).fd };
        let count = c_int::try_from(payload.count).unwrap_or(c_int::MAX);
        // SAFETY: `socket->writev(iov, iovcnt)` over the payload's vector.
        unsafe { libc::writev(fd, payload.iov, count) }
    }

    fn create_buffer(&self) -> *mut Buffer {
        let socket = self.socket;
        // SAFETY: the write's live socket.
        let chunk_size = unsafe { (*socket).chunk_size };
        let buffer = swoole_rs_buffer::cpp::new_buffer(chunk_size);
        // SAFETY: `socket->out_buffer = new Buffer(socket->chunk_size)`.
        unsafe { (*socket).output.out_buffer = buffer.cast() };
        // SAFETY: the buffer just made.
        unsafe { out_queue(socket) }
    }

    fn watch_writable(&self) {
        // SAFETY: the write's live socket.
        let events = unsafe { (*self.socket).events };
        if events & SW_EVENT_WRITE == 0 {
            // SAFETY: `reactor->add_write_event(socket)` on the live reactor.
            unsafe { reactor_interest(self.reactor, self.socket, ADD_WRITE, 0) };
        }
    }
}

/// `Reactor::write_func`.
///
/// # Safety
/// `reactor` is a live `Reactor`, `socket` a live record and `payload` valid.
unsafe fn write(reactor: *mut c_void, socket: *mut SocketRecord, payload: &WritePayload) -> isize {
    let ops = Write {
        reactor,
        socket,
        payload,
    };
    // SAFETY: as the caller promises.
    let (output, queue) = unsafe { output(socket) };
    // SAFETY: `Socket::default_buffer_size`, which only the main thread sets.
    let default_size = unsafe { swr_v1_socket_defaults.buffer_size };
    // SAFETY: the socket's flag.
    let nobufs = unsafe { &raw mut (*socket).kernel_nobufs };
    // SAFETY: the socket's output block, flag and queue, the payload and the ops.
    let result = unsafe {
        swoole_rs_reactor::swr_v1_reactor_write(output, nobufs, queue, default_size, payload, &ops)
    };
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
            // SAFETY: the overflowed socket has a queue.
            let queue = unsafe { out_queue(socket) };
            let (mut queued, mut count) = (0u32, 0usize);
            // SAFETY: the live queue; both outputs are local.
            unsafe {
                swoole_rs_buffer::swr_v1_buffer_stats(queue, &raw mut queued, &raw mut count)
            };
            // SAFETY: the live socket.
            let record = unsafe { &*socket };
            let (fd, size) = (record.fd, record.output.buffer_size);
            log::error_log(
                log::LOG_WARNING,
                SW_ERROR_OUTPUT_BUFFER_OVERFLOW,
                origin,
                format_args!("socket#{fd} output buffer overflow: ({queued}/{size})"),
            );
        }
        3 => log::set_last_error(std::io::Error::last_os_error().raw_os_error().unwrap_or(0)),
        _ => buffer_check(result.status),
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
    if !unsafe { (*socket).ssl }.is_null() {
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

impl FlushEffects for Flush {
    fn front_kind(&self) -> c_int {
        // SAFETY: the flush's socket has queued output.
        let queue = unsafe { out_queue(self.socket) };
        let mut front = swoole_rs_buffer::TypedFront::default();
        // SAFETY: the live queue and a local view.
        buffer_check(unsafe { swoole_rs_buffer::swr_v1_buffer_typed_front(queue, &raw mut front) });
        front.kind.cast_signed()
    }

    fn handle_send(&self) -> c_int {
        // SAFETY: the flush's live socket.
        unsafe { socket_handle_send(self.socket) }
    }

    fn handle_sendfile(&self) -> c_int {
        // SAFETY: the flush's live socket.
        unsafe { socket_handle_sendfile(self.socket) }
    }

    fn close(&self) -> c_int {
        // SAFETY: the flush's live reactor.
        let fields = unsafe { prepared(self.reactor) }.fields;
        // SAFETY: `reactor->close`, set at construction.
        let Some(close) = (unsafe { (*fields).close }) else {
            return -1;
        };
        // SAFETY: `reactor->close(reactor, socket)`.
        unsafe { close(self.reactor, self.socket) }
    }

    fn trigger_close(&self) -> c_int {
        // SAFETY: the flush's live reactor and event.
        unsafe { trigger_close_event(self.reactor, self.event.cast()) }
    }

    fn delete_timer(&self) {
        // SAFETY: the flush's live socket.
        let timer = unsafe { (*self.socket).output.send_timer };
        // SAFETY: `swoole_timer_del(socket->send_timer)`.
        unsafe { swoole_rs_timer::swoole_timer_del(timer.cast()) };
    }

    fn remove_write_event(&self) {
        // SAFETY: `reactor->remove_write_event(socket)` on the flush's live reactor.
        unsafe { reactor_interest(self.reactor, self.socket, REMOVE_WRITE, 0) };
    }

    fn wait_writable(&self) -> c_int {
        // SAFETY: `Socket::default_write_timeout`, which only the main thread sets.
        let timeout = unsafe { swr_v1_socket_defaults.timeouts.write };
        #[expect(
            clippy::cast_possible_truncation,
            reason = "upstream's static_cast<int>(sec2msec(...))"
        )]
        let timeout = (timeout * 1000.0) as c_int;
        // SAFETY: the flush's live socket.
        socket_wait_event(unsafe { &*self.socket }, timeout, SW_EVENT_WRITE)
    }

    fn removed(&self) -> bool {
        // SAFETY: the flush's live socket.
        unsafe { (*self.socket).removed != 0 }
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
    let socket = unsafe { (*event).socket }.cast::<SocketRecord>();
    let flush = Flush {
        reactor,
        socket,
        event,
    };
    // SAFETY: as the caller promises.
    let (output, queue) = unsafe { output(socket) };
    // SAFETY: the socket's output block and queue.
    unsafe { swoole_rs_reactor::swr_v1_reactor_flush(output, queue, &flush) }
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
    let mut event = Event {
        // SAFETY: as the caller promises.
        fd: unsafe { (*socket).fd },
        reactor_id: 0,
        fd_type: 0,
        socket: socket.cast(),
    };
    let flush = Flush {
        reactor,
        socket,
        event: &raw mut event,
    };
    // SAFETY: as the caller promises.
    let (output, queue) = unsafe { output(socket) };
    // SAFETY: the socket's output block and queue.
    unsafe { swoole_rs_reactor::swr_v1_reactor_drain(output, queue, &flush) };
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
