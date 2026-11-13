//! `Server::send` and `Server::send_to_connection`: the master's (or a base-mode
//! worker's) delivery of a send, a close or a receive pause/resume to a connection it
//! owns, over the connection, its socket and its output buffer.
//!
//! `--fix-halfclose` (M15) builds have the `halfclose` feature (upstream's
//! `SW_HALFCLOSE_DRAIN`): pause and resume then go through the reactor's read-event
//! helpers, a resume skips closing connections, and a queued close stops reading.
use crate::base_factory::{SEND_DATA, SendData, allocated};
use crate::callbacks::{CB_BUFFER_EMPTY, CB_BUFFER_FULL, CallbackSlot};
use crate::reactor_handle::Reactor;
use crate::reactor_ops::out_buffer;
use crate::records::Lookup;
use crate::view::Server;
use crate::{Connection, MODE_PROCESS, ServerState};
use core::ffi::{CStr, c_int, c_void};
use swoole_rs_buffer::cpp::{
    new_buffer, swr_cpp_buffer_append, swr_cpp_buffer_empty, swr_cpp_buffer_length,
    swr_cpp_buffer_typed_close,
};
use swoole_rs_corebase::constants::SW_SEND_BUFFER_SIZE;
use swoole_rs_corebase::{CCast, DataHead, log, microtime};
use swoole_rs_error::ErrorCode as E;
use swoole_rs_reactor::{Socket, socket_record_send, steady_ms};

/// `ServerEventType`.
const SEND_FILE: u8 = 3;
const CLOSE: u8 = 4;
const BUFFER_FULL: c_int = 13;
const PAUSE_RECV: u8 = 11;
const RESUME_RECV: u8 = 12;
/// `SW_EVENT_READ`, `SW_EVENT_WRITE`.
const EVENT_READ: c_int = swoole_rs_corebase::constants::EventType::SW_EVENT_READ as c_int;
const EVENT_WRITE: c_int = swoole_rs_corebase::constants::EventType::SW_EVENT_WRITE as c_int;
/// `Reactor::change`'s add, set and del; `Reactor::interest`'s add-read, add-write and
/// remove-read.
const ADD: u32 = 0;
const SET: u32 = 1;
const DEL: u32 = 2;
const ADD_READ: u32 = 2;
const ADD_WRITE: u32 = 3;
const REMOVE_READ: u32 = 4;
/// `SW_OK` and `SW_ERR`.
const SW_OK: c_int = swoole_rs_corebase::constants::ResultCode::SW_OK as c_int;
const SW_ERR: c_int = swoole_rs_corebase::constants::ResultCode::SW_ERR as c_int;
/// `SW_LOG_TRACE`, `SW_LOG_NOTICE` and `SW_LOG_WARNING`.
const LOG_TRACE: c_int = swoole_rs_corebase::log::LogLevel::SW_LOG_TRACE as c_int;
const LOG_NOTICE: c_int = swoole_rs_corebase::log::LogLevel::SW_LOG_NOTICE as c_int;
const LOG_WARNING: c_int = swoole_rs_corebase::log::LogLevel::SW_LOG_WARNING as c_int;

fn error(level: c_int, code: E, message: core::fmt::Arguments<'_>) {
    log::error_log(level, code as c_int, c"Server::send_to_connection", message);
}

/// `Server::send(session_id, data, length)`: through the factory, then the server's,
/// port's and worker's counts. A base or thread mode worker that owns the session
/// sends it here, as `BaseFactory::finish` would. The length narrows to `uint32_t`,
/// as the C++ callers' `ZSTR_LEN` did.
#[expect(
    clippy::must_use_candidate,
    reason = "callers ignore the result where upstream's do"
)]
pub fn send(serv: Server<'_>, session_id: core::ffi::c_long, data: &[u8]) -> bool {
    let len = data.len().c_cast::<u32>();
    let info = DataHead {
        fd: session_id,
        kind: SEND_DATA,
        len,
        ..DataHead::default()
    };
    let data = data.get(..len as usize).unwrap_or(data);
    let state = serv.state();
    let here = u64::from(serv.config().mode_) != MODE_PROCESS && sends_here(state, session_id);
    let sent = if here {
        match send_now(state, session_id, data) {
            Some(sent) if usize::try_from(sent) == Ok(data.len()) => true,
            first => to_connection(serv, &info, data, first) == SW_OK,
        }
    } else {
        let mut send = SendData {
            info,
            data: data.as_ptr().cast(),
        };
        crate::hybrid::hybrid().factory_finish(serv, &mut send)
    };
    if sent {
        crate::shared::count_send(state, session_id, len);
    }
    sent
}

/// Whether `BaseFactory::finish` would send to `session_id` from this worker: it owns
/// the session, or there is none.
fn sends_here(state: &ServerState, session_id: core::ffi::c_long) -> bool {
    let worker = swoole_rs_process::methods::swoole_get_worker_id();
    !allocated(state)
        || state
            .session(session_id)
            .is_none_or(|session| session.reactor_bits() & 0xff == worker)
}

/// [`send`]'s direct write, made here when it needs nothing else: the verified
/// connection has no overflow and its socket sends directly with no output buffer yet.
/// `None` leaves the whole send to [`to_connection`]; otherwise `Socket::send`'s result
/// after EINTR retries, and a complete send has set the connection's `last_send_time`.
fn send_now(state: &ServerState, session_id: core::ffi::c_long, data: &[u8]) -> Option<i64> {
    let conn = state.connection(session_id, Lookup::VerifiedTls)?;
    let socket = conn.socket()?;
    if conn.overflow()
        || socket.read(|record| {
            !record.direct_send || !record.ssl().is_null() || !record.output.out_buffer.is_null()
        })
    {
        return None;
    }
    let sent = loop {
        let (at, len) = (data.as_ptr().cast(), data.len());
        // SAFETY: `data` holds `len` bytes.
        let sent = unsafe { socket_record_send(socket, at, len, 0, no_tls, steady_ms) };
        if sent >= 0 || std::io::Error::last_os_error().raw_os_error() != Some(libc::EINTR) {
            break sent;
        }
    };
    if sent.cast_unsigned() == data.len() {
        conn.set_last_send_time(microtime());
    }
    Some(i64::try_from(sent).unwrap_or(-1))
}

/// `send_now`'s TLS call, never made: it sends only on sockets without an SSL.
extern "C" fn no_tls(_: *mut c_void, _: *mut c_void, _: usize) -> isize {
    -1
}

/// `Server::send_to_connection(send)`: `SW_OK` or `SW_ERR`, and 0 (upstream's
/// `false`) where a send fails after the connection was found.
///
/// # Safety
/// `send` lends `info.len` readable bytes at `data` (a `SendfileTask` and its name
/// for a file), or none.
#[expect(
    clippy::must_use_candidate,
    reason = "callers ignore the result where upstream's do"
)]
pub unsafe fn send_to_connection(serv: Server<'_>, send: &SendData) -> c_int {
    let len = send.info.len as usize;
    let data = if len == 0 || send.data.is_null() {
        &[][..]
    } else {
        // SAFETY: as the caller guarantees.
        unsafe { core::slice::from_raw_parts(send.data.cast::<u8>(), len) }
    };
    to_connection(serv, &send.info, data, None)
}

/// [`send_to_connection`] of the event `info` with its `data`, where `first` is what
/// [`send_now`] already sent, short of all of it.
fn to_connection(serv: Server<'_>, info: &DataHead, data: &[u8], first: Option<i64>) -> c_int {
    let state = serv.state();
    let (kind, session) = (info.kind, info.fd);
    let lookup = if kind == CLOSE {
        Lookup::Verified
    } else {
        Lookup::VerifiedTls
    };
    let found = state.connection(session, lookup);
    let Some((conn, sock)) = found.and_then(|conn| Some((conn, conn.socket()?))) else {
        let code = E::SW_ERROR_SESSION_NOT_EXIST;
        if kind == SEND_DATA {
            let length = info.len;
            let m = format_args!("send {length} byte failed, session#{session} does not exist");
            error(LOG_TRACE, code, m);
        } else {
            let m = format_args!("send event[{kind}] failed, session#{session} does not exist");
            error(LOG_TRACE, code, m);
        }
        return SW_ERR;
    };
    let reactor = Reactor::thread();
    let config = serv.config();
    debug_assert!(
        config.single_thread.get()
            || i64::from(conn.fd()) % i64::from(config.reactor_num.get())
                == i64::from(swoole_rs_corebase::thread_id()),
        "a connection is sent to on its own reactor thread"
    );
    let send_yield = config.send_yield.get();
    let yield_error = || log::set_last_error(E::SW_ERROR_OUTPUT_SEND_YIELD as c_int);
    if conn.overflow() && u64::from(config.mode_) != MODE_PROCESS {
        if send_yield {
            yield_error();
        } else {
            let m = format_args!("socket#{} output buffer overflow", conn.fd());
            error(LOG_WARNING, E::SW_ERROR_OUTPUT_BUFFER_OVERFLOW, m);
        }
        return SW_ERR;
    }
    if kind == CLOSE && (conn.close_reset() || conn.close_force() || conn.peer_closed()) {
        reactor.close(sock);
        return SW_OK;
    }
    let halfclose = cfg!(feature = "halfclose");
    let (removed, notified, events) = sock.read(|s| (s.removed, s.close_notified, s.events));
    let (reading, writing) = (events & EVENT_READ != 0, events & EVENT_WRITE != 0);
    if kind == PAUSE_RECV {
        if removed || !reading {
            return SW_OK;
        }
        return if halfclose {
            reactor.interest(sock, REMOVE_READ, 0)
        } else if writing {
            reactor.change(sock, SET, EVENT_WRITE)
        } else {
            reactor.change(sock, DEL, 0)
        };
    }
    if kind == RESUME_RECV {
        let skip = if halfclose {
            let closing = conn.closed().load(core::sync::atomic::Ordering::Relaxed) != 0;
            closing || conn.peer_closed() || conn.close_queued() || notified || reading
        } else {
            !removed || reading
        };
        if skip {
            return SW_OK;
        }
        return if halfclose {
            reactor.interest(sock, ADD_READ, 0)
        } else if writing {
            reactor.change(sock, SET, EVENT_READ | EVENT_WRITE)
        } else {
            reactor.change(sock, ADD, EVENT_READ)
        };
    }
    let mut data = data;
    let out_empty = sock.read(|s| out_buffer(s).is_none_or(|b| swr_cpp_buffer_empty(b)));
    if out_empty {
        if kind == CLOSE {
            reactor.close(sock);
            return SW_OK;
        }
        if kind != SEND_FILE
            && sock.read(|s| s.direct_send)
            && direct_send(conn, sock, &mut data, first)
        {
            return SW_OK;
        }
        if sock.read(|s| s.output.out_buffer.is_null()) {
            let buffer = new_buffer(SW_SEND_BUFFER_SIZE).cast();
            sock.edit(|s| s.output.out_buffer = buffer);
        }
    }
    let queued = || sock.read(|s| out_buffer(s).map_or(0, |b| swr_cpp_buffer_length(b)));
    if kind == CLOSE {
        sock.read(|s| out_buffer(s).map(|b| swr_cpp_buffer_typed_close(b)));
        conn.set_close_queued(true);
        if halfclose {
            reactor.interest(sock, REMOVE_READ, 0);
        }
    } else if kind == SEND_FILE {
        if send_file(sock, data) < 0 {
            return 0;
        }
    } else {
        if conn.peer_closed() {
            let m = format_args!("socket#{} is closed by client", conn.fd());
            error(LOG_NOTICE, E::SW_ERROR_SESSION_CLOSED_BY_CLIENT, m);
            return 0;
        }
        let callback = |index| {
            state
                .callbacks
                .slot(index)
                .is_some_and(CallbackSlot::is_set)
        };
        let (on_full, on_empty) = (callback(CB_BUFFER_FULL), callback(CB_BUFFER_EMPTY));
        if queued() >= sock.read(|s| s.output.buffer_size) {
            if send_yield {
                yield_error();
            } else {
                let m = format_args!("connection#{} output buffer overflow", conn.fd());
                error(LOG_WARNING, E::SW_ERROR_OUTPUT_BUFFER_OVERFLOW, m);
            }
            conn.set_overflow(true);
            if on_empty && !on_full {
                conn.set_high_watermark(true);
            }
        }
        let (at, len) = (data.as_ptr(), data.len().c_cast::<u32>());
        // SAFETY: `data` holds `len` bytes.
        sock.read(|s| out_buffer(s).map(|b| unsafe { swr_cpp_buffer_append(b, at, len) }));
        conn.set_send_queued_bytes(queued());
        let port = serv.port_by_server_fd(conn.server_fd());
        let high = port.map_or(u32::MAX, |port| port.config().buffer_high_watermark.get());
        if on_full && !conn.high_watermark() && queued() >= high {
            crate::server_methods::notify(serv, conn, BUFFER_FULL);
            conn.set_high_watermark(true);
        }
    }
    let port = serv.port_by_server_fd(conn.server_fd());
    let idle = port.map_or(0.0, |port| port.config().max_idle_time.get());
    if idle > 0.0 && sock.read(|s| s.output.send_timer.is_null()) {
        idle_timer(conn, sock, idle);
    }
    if sock.read(|s| s.events) & EVENT_WRITE == 0 {
        reactor.interest(sock, ADD_WRITE, 0);
    }
    SW_OK
}

/// Writes the bytes straight to the socket: true when all of them went, else the rest
/// is left in `data` for the buffer. After `send_now`'s attempt only its progress is
/// recorded.
fn direct_send(
    conn: Connection<'_>,
    sock: Socket<'_>,
    data: &mut &[u8],
    first: Option<i64>,
) -> bool {
    if let Some(sent) = first {
        advance(data, sent);
        return false;
    }
    loop {
        let sent = swoole_rs_tls::upstream::send(sock, data, 0);
        if sent >= 0 && sent.cast_unsigned() == data.len() {
            conn.set_last_send_time(microtime());
            return true;
        }
        if sent > 0 {
            advance(data, i64::try_from(sent).unwrap_or(0));
            return false;
        }
        if std::io::Error::last_os_error().raw_os_error() != Some(libc::EINTR) {
            return false;
        }
    }
}

/// Moves past the `sent` bytes of `data`, if any.
fn advance(data: &mut &[u8], sent: i64) {
    let step = usize::try_from(sent).unwrap_or(0).min(data.len());
    *data = data.get(step..).unwrap_or_default();
}

/// `SendfileTask`'s head: its `off_t offset` and `size_t length`; the name follows.
const TASK_HEAD: usize = 16;

/// `sock->sendfile_async(task->filename, task->offset, task->length)`, or -1 for a
/// task without its head or name.
fn send_file(sock: Socket<'_>, task: &[u8]) -> c_int {
    let word = |at: usize| <[u8; 8]>::try_from(task.get(at..at + 8)?).ok();
    let (Some(offset), Some(length)) = (word(0), word(8)) else {
        return -1;
    };
    let Some(Ok(name)) = task.get(TASK_HEAD..).map(CStr::from_bytes_until_nul) else {
        return -1;
    };
    let (offset, length) = (i64::from_ne_bytes(offset), usize::from_ne_bytes(length));
    swoole_rs_tls::upstream::socket_sendfile_async(sock, name, offset, length)
}

/// `swoole_timer_add(sec2msec(port->max_idle_time), true, ...)` for the socket's
/// sends.
fn idle_timer(conn: Connection<'_>, sock: Socket<'_>, idle: f64) {
    let msec = (idle * 1000.0).c_cast::<libc::c_long>();
    let tick = crate::connection::idle_tick;
    // SAFETY: `tick` takes the live connection, which deletes the timer before it goes.
    let node =
        unsafe { swoole_rs_timer::swr_v1_timer_add_plain(msec, true, tick, conn.as_ptr().cast()) };
    let now = steady_ms();
    sock.edit(|sock| {
        sock.timeouts.write = idle;
        sock.last_sent_time = now;
        sock.output.send_timer = node;
    });
}
#[cfg(test)]
mod tests {
    use super::*;
    use crate::fixture::LiveServer;
    use crate::hybrid::tests::{Call, calls};
    use std::io::Read;

    /// The live server with one accepted connection, session 1, and its client.
    struct Accepted {
        live: LiveServer,
        client: std::net::TcpStream,
    }

    impl Accepted {
        fn new() -> Self {
            let live = LiveServer::new();
            let client = live.connect();
            // SAFETY: the fixture's listener, live for the event.
            let event = unsafe {
                swoole_rs_reactor::Event::new(live.listener_fd, 0, 1, live.listener().cast())
            };
            crate::connection::accept(live.server(), live.reactor(), &event);
            client
                .set_read_timeout(Some(std::time::Duration::from_secs(5)))
                .unwrap();
            Self { live, client }
        }
        fn serv(&self) -> Server<'_> {
            self.live.server()
        }
        fn conn(&self) -> Connection<'_> {
            let state = self.live.state();
            state.connection(1, Lookup::Verified).unwrap()
        }
        fn sock(&self) -> Socket<'_> {
            self.conn().socket().unwrap()
        }
        /// `send_to_connection` of an event of `kind` with `data` to `session`.
        fn send(&self, kind: u8, session: i64, data: &[u8]) -> c_int {
            let send = event(kind, session, data);
            // SAFETY: `data` is live for the call.
            unsafe { send_to_connection(self.serv(), &send) }
        }
        /// The reactor records its closes, as the server's would close them.
        fn closes(&self) {
            self.live.reactor().handle().set_close(Some(record_close));
            closed();
        }
        fn queued(&self) -> u32 {
            self.sock()
                .read(|s| out_buffer(s).map_or(0, |b| swr_cpp_buffer_length(b)))
        }
        fn read(&mut self, len: usize) -> Vec<u8> {
            let mut got = vec![0; len];
            self.client.read_exact(&mut got).unwrap();
            got
        }
    }

    std::thread_local! {
        static CLOSED: core::cell::RefCell<Vec<c_int>> = const { core::cell::RefCell::new(Vec::new()) };
    }

    /// The fds the reactor's close was given since the last call.
    fn closed() -> Vec<c_int> {
        CLOSED.with_borrow_mut(core::mem::take)
    }

    unsafe extern "C-unwind" fn record_close(
        _: *mut c_void,
        socket: *mut swoole_rs_reactor::SocketRecord,
    ) -> c_int {
        // SAFETY: the reactor's live socket.
        let fd = unsafe { Socket::from_ptr(socket) }.map_or(-1, Socket::fd);
        CLOSED.with_borrow_mut(|closed| closed.push(fd));
        0
    }

    fn event(kind: u8, session: i64, data: &[u8]) -> SendData {
        SendData {
            info: DataHead {
                fd: session,
                kind,
                len: data.len().c_cast::<u32>(),
                ..DataHead::default()
            },
            data: data.as_ptr().cast(),
        }
    }

    #[test]
    #[cfg_attr(miri, ignore = "Miri has no epoll for the live reactor")]
    fn a_missing_session_fails() {
        let a = Accepted::new();
        assert_eq!(a.send(SEND_DATA, 9, b"data"), SW_ERR);
        assert_eq!(a.send(CLOSE, 9, b""), SW_ERR);
    }

    #[test]
    #[cfg_attr(miri, ignore = "Miri has no epoll for the live reactor")]
    fn overflow_outside_process_mode_refuses() {
        let a = Accepted::new();
        a.conn().set_overflow(true);
        assert_eq!(
            a.send(SEND_DATA, 1, b"data"),
            SW_OK,
            "process mode buffers it"
        );
        // SAFETY: the fixture's config, which nothing else borrows here.
        let config = unsafe { a.serv().config_mut() };
        config.mode_ = crate::MODE_BASE.c_cast();
        assert_eq!(a.send(SEND_DATA, 1, b"data"), SW_ERR);
        config.send_yield.set(true);
        assert_eq!(a.send(SEND_DATA, 1, b"data"), SW_ERR);
        assert_eq!(
            log::last_error(),
            E::SW_ERROR_OUTPUT_SEND_YIELD as c_int,
            "send_yield reports it instead"
        );
    }

    #[test]
    #[cfg_attr(miri, ignore = "Miri has no epoll for the live reactor")]
    fn pause_and_resume_move_the_read_event() {
        let a = Accepted::new();
        let events = || a.sock().read(|s| (s.removed, s.events & EVENT_READ != 0));
        assert_eq!(
            events(),
            (true, false),
            "the master leaves it to a reactor thread"
        );
        assert_eq!(a.send(PAUSE_RECV, 1, b""), SW_OK, "not reading");
        assert_eq!(a.send(RESUME_RECV, 1, b""), SW_OK);
        assert_eq!(events(), (false, true), "reading");
        assert_eq!(a.send(RESUME_RECV, 1, b""), SW_OK, "already reading");
        assert_eq!(a.send(PAUSE_RECV, 1, b""), SW_OK);
        assert_eq!(events(), (true, false), "removed");
    }

    #[test]
    #[cfg_attr(miri, ignore = "Miri has no epoll for the live reactor")]
    fn a_direct_send_reaches_the_peer() {
        let mut a = Accepted::new();
        assert_eq!(a.send(SEND_DATA, 1, b"hello"), SW_OK);
        assert_eq!(a.read(5), b"hello");
        assert_eq!(a.queued(), 0, "nothing buffered");
        assert!(a.conn().last_send_time() > 0.0, "touched");
    }

    #[test]
    #[cfg_attr(miri, ignore = "Miri has no epoll for the live reactor")]
    fn without_direct_send_the_data_is_buffered_for_the_write_event() {
        let a = Accepted::new();
        a.sock().edit(|s| s.direct_send = false);
        assert_eq!(a.send(SEND_DATA, 1, b"hello"), SW_OK);
        assert_eq!(a.queued(), 5);
        assert_eq!(a.conn().send_queued_bytes(), 5);
        assert!(
            a.sock().read(|s| s.events & EVENT_WRITE != 0),
            "waits to write"
        );
        a.conn().set_peer_closed(true);
        assert_eq!(a.send(SEND_DATA, 1, b"more"), 0, "closed by the client");
        assert_eq!(a.queued(), 5);
    }

    #[test]
    #[cfg_attr(miri, ignore = "Miri has no epoll for the live reactor")]
    fn a_send_made_first_is_not_repeated() {
        let mut a = Accepted::new();
        let data = b"0123456789";
        let info = event(SEND_DATA, 1, data).info;
        assert_eq!(
            send_now(a.live.state(), 1, data),
            Some(10),
            "all of it went"
        );
        assert_eq!(a.read(10), data);
        // As if only four had gone.
        assert_eq!(to_connection(a.serv(), &info, data, Some(4)), SW_OK);
        assert_eq!(a.queued(), 6, "the rest is buffered");
        let a = Accepted::new();
        assert_eq!(to_connection(a.serv(), &info, data, Some(-1)), SW_OK);
        assert_eq!(a.queued(), 10, "would block: all of it is buffered");
        assert!(a.client.set_nonblocking(true).is_ok());
        assert!((&a.client).read(&mut [0; 1]).is_err(), "nothing sent");
    }

    #[test]
    #[cfg_attr(miri, ignore = "Miri has no epoll for the live reactor")]
    fn a_full_buffer_overflows_and_the_watermark_notifies() {
        extern "C" fn full(_: crate::callbacks::Ctx, _: Server<'_>, _: &DataHead) {}
        let a = Accepted::new();
        a.sock().edit(|s| {
            s.direct_send = false;
            s.output.buffer_size = 4;
        });
        a.live.port().buffer_high_watermark.set(2);
        let callbacks = &a.live.state().callbacks;
        callbacks.set(crate::callbacks::Callback::BufferFull(full));
        calls();
        assert_eq!(a.send(SEND_DATA, 1, b"abc"), SW_OK);
        let fd = a.conn().fd();
        assert_eq!(
            calls(),
            [Call::Notify(
                fd.into(),
                0,
                13,
                a.conn().server_fd().c_cast()
            )],
            "past the high watermark"
        );
        assert!(a.conn().high_watermark());
        assert_eq!(a.send(SEND_DATA, 1, b"de"), SW_OK);
        assert_eq!(calls(), [], "already above the watermark");
        assert!(!a.conn().overflow());
        assert_eq!(a.send(SEND_DATA, 1, b"f"), SW_OK, "buffered all the same");
        assert!(a.conn().overflow(), "the buffer was full");
        assert_eq!(a.queued(), 6);
    }

    #[test]
    #[cfg_attr(miri, ignore = "Miri has no epoll for the live reactor")]
    fn close_closes_now_or_queues_behind_the_output() {
        let a = Accepted::new();
        a.closes();
        a.sock().edit(|s| s.direct_send = false);
        assert_eq!(a.send(SEND_DATA, 1, b"hello"), SW_OK);
        assert_eq!(a.send(CLOSE, 1, b""), SW_OK);
        assert!(a.conn().close_queued(), "behind the output");
        assert_eq!(closed(), [], "not yet");

        let a = Accepted::new();
        a.closes();
        assert_eq!(a.send(CLOSE, 1, b""), SW_OK);
        assert_eq!(closed(), [a.conn().fd()], "closed now");
    }

    #[test]
    #[cfg_attr(miri, ignore = "Miri has no epoll for the live reactor")]
    fn a_forced_close_skips_the_output() {
        let a = Accepted::new();
        a.closes();
        a.sock().edit(|s| s.direct_send = false);
        assert_eq!(a.send(SEND_DATA, 1, b"hello"), SW_OK);
        a.conn().set_close_force(true);
        assert_eq!(a.send(CLOSE, 1, b""), SW_OK);
        assert_eq!(closed(), [a.conn().fd()], "closed with output still queued");
    }

    #[test]
    #[cfg_attr(miri, ignore = "Miri has no epoll for the live reactor")]
    fn files_are_queued_never_sent_directly() {
        let task = |name: &[u8]| {
            let mut task = 0i64.to_ne_bytes().to_vec();
            task.extend(3usize.to_ne_bytes());
            task.extend(name);
            task.push(0);
            task
        };
        let a = Accepted::new();
        let file = task(b"Cargo.toml");
        assert_eq!(a.send(SEND_FILE, 1, &file), SW_OK);
        assert!(
            a.sock().read(|s| s.events & EVENT_WRITE != 0),
            "waits to write"
        );
        assert_eq!(a.send(SEND_FILE, 1, &task(b"no-such-file")), 0);
    }

    #[test]
    #[cfg_attr(miri, ignore = "Miri has no epoll for the live reactor")]
    fn send_goes_through_the_factory_in_process_mode_and_here_otherwise() {
        let mut a = Accepted::new();
        let counters = |a: &Accepted| {
            let count = &a.live.counters().response_count;
            count.load(core::sync::atomic::Ordering::SeqCst)
        };
        calls();
        let data = [7u8; 16];
        assert!(send(a.serv(), 1, &data));
        assert_eq!(
            calls(),
            [Call::Finish(
                1,
                SEND_DATA,
                16,
                0x0707_0707_0707_0707,
                0x0707_0707_0707_0707,
                [0; 8]
            )]
        );
        assert_eq!(counters(&a), 1, "counted");
        // SAFETY: the fixture's config, which nothing else borrows here.
        unsafe { a.serv().config_mut() }.mode_ = crate::MODE_BASE.c_cast();
        assert!(send(a.serv(), 1, b"here"));
        assert_eq!(calls(), [], "no factory");
        assert_eq!(a.read(4), b"here");
        assert_eq!(counters(&a), 2);
        assert!(!send(a.serv(), 9, b"lost"), "no session");
        assert_eq!(counters(&a), 2);
    }
}
