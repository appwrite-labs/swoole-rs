//! `Server::send` and `Server::send_to_connection`: the master's (or a base-mode
//! worker's) delivery of a send, a close or a receive pause/resume to a connection it
//! owns, over the connection, its socket and its output buffer.
//!
//! `--fix-halfclose` (M15) builds have the `halfclose` feature (upstream's
//! `SW_HALFCLOSE_DRAIN`): pause and resume then go through the reactor's read-event
//! helpers, a resume skips closing connections, and a queued close stops reading.
use crate::base_factory::{SendData, allocated};
use crate::callbacks::CallbackKind::{BufferEmpty, BufferFull};
use crate::reactor_handle::Reactor;
use crate::reactor_ops::out_buffer;
use crate::records::Lookup;
use crate::view::Server;
use crate::{Connection, Mode, ServerState};
use core::ffi::{CStr, c_int};
use std::io::IoSlice;
use swoole_rs_corebase::ServerEvent;
use swoole_rs_corebase::buffer::cpp::{
    new_buffer, swr_cpp_buffer_empty, swr_cpp_buffer_length, swr_cpp_buffer_typed_close,
};
use swoole_rs_corebase::constants::SW_SEND_BUFFER_SIZE;
use swoole_rs_corebase::log::LogLevel::{
    SW_LOG_NOTICE as LOG_NOTICE, SW_LOG_TRACE as LOG_TRACE, SW_LOG_WARNING as LOG_WARNING,
};
use swoole_rs_corebase::reactor::{ChangeOp, Drain, Interest, Socket, count_write, steady_ms};
use swoole_rs_corebase::reactor::{Events, SocketFields};
use swoole_rs_corebase::{CCast, DataHead, log, microtime};
use swoole_rs_error::ErrorCode as E;

fn error(level: swoole_rs_corebase::log::LogLevel, code: E, message: core::fmt::Arguments<'_>) {
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
    let info = || DataHead {
        fd: session_id,
        kind: ServerEvent::SendData.code(),
        len,
        ..DataHead::default()
    };
    let data = data.get(..len as usize).unwrap_or(data);
    let state = serv.state();
    let here = serv.config().mode_ != Mode::Process && sends_here(state, session_id);
    let sent = if here {
        let first = send_now(state, session_id, data);
        if let Some((sent, conn)) = first
            && usize::try_from(sent) == Ok(data.len())
        {
            // The session's own slot, which its lookup by id would find again.
            let port = state.port_by_server_fd(conn.server_fd());
            crate::shared::count_sent(state, port.and_then(|port| port.config().counters()), len);
            return true;
        }
        to_connection(serv, &info(), data, first.map(|(sent, _)| sent))
    } else {
        let mut send = SendData::new(info(), data);
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
    let worker = swoole_rs_corebase::process::methods::swoole_get_worker_id();
    !allocated(state)
        || state
            .session(session_id)
            .is_none_or(|session| session.reactor_bits() & 0xff == worker)
}

/// [`send`] of `head` then `body` as one `writev(2)`, so a large body is not copied
/// behind its head. `None`, with nothing sent, unless this worker writes straight to the
/// session's socket ([`write_now`]): the caller joins the two for [`send`]. What the
/// socket does not take is queued, as [`send`] queues it.
#[must_use]
pub fn send_parts(
    serv: Server<'_>,
    session_id: core::ffi::c_long,
    head: &[u8],
    body: &[u8],
) -> Option<bool> {
    let state = serv.state();
    if serv.config().mode_ == Mode::Process || !sends_here(state, session_id) {
        return None;
    }
    let parts = [IoSlice::new(head), IoSlice::new(body)];
    let write = |fd| swoole_rs_corebase::tls::upstream::writev(fd, &parts);
    // A failed write sent nothing: all of it is queued, where the reactor meets the error.
    let (conn, sent) = write_now(state, session_id, write)?;
    let sent = sent.unwrap_or(0);
    let total = head.len() + body.len();
    let done = if sent == total {
        conn.set_last_send_time(microtime());
        true
    } else {
        let head_left = head.get(sent..).unwrap_or_default();
        let body_left = body.get(sent.saturating_sub(head.len())..);
        [head_left, body_left.unwrap_or_default()]
            .into_iter()
            .filter(|left| !left.is_empty())
            .all(|left| {
                let info = DataHead {
                    fd: session_id,
                    kind: ServerEvent::SendData.code(),
                    len: left.len().c_cast::<u32>(),
                    ..DataHead::default()
                };
                to_connection(serv, &info, left, Some(0))
            })
    };
    if done {
        crate::shared::count_send(state, session_id, total.c_cast::<u32>());
    }
    Some(done)
}

/// A direct plaintext write on this worker's session. Read the descriptor and retry
/// policy together with the direct-send checks, then release the borrow before I/O.
#[inline(always)]
fn write_now(
    state: &ServerState,
    session_id: core::ffi::c_long,
    write: impl Fn(c_int) -> Option<usize>,
) -> Option<(Connection<'_>, Option<usize>)> {
    let conn = state.connection(session_id, Lookup::VerifiedTls)?;
    if conn.overflow() {
        return None;
    }
    let socket = conn.socket()?;
    let (fd, dont_restart) = socket.read(|s| {
        (s.direct_send && s.ssl().is_null() && s.output.out_buffer.is_none())
            .then_some((s.fd, s.dont_restart))
    })?;
    let sent = count_write(*socket, dont_restart, || write(fd), steady_ms);
    Some((conn, sent))
}

/// [`send`]'s direct write, made here when it needs nothing else ([`write_now`]).
/// `None` leaves the whole send to [`to_connection`]; otherwise `Socket::send`'s result
/// after EINTR retries, and the session's connection, whose `last_send_time` a complete
/// send has set.
fn send_now<'a>(
    state: &'a ServerState,
    session_id: core::ffi::c_long,
    data: &[u8],
) -> Option<(i64, Connection<'a>)> {
    let (conn, sent) = loop {
        let (conn, sent) = write_now(state, session_id, |fd| Drain::new(data).send(fd, 0))?;
        if sent.is_some() || std::io::Error::last_os_error().raw_os_error() != Some(libc::EINTR) {
            break (conn, sent);
        }
    };
    if sent == Some(data.len()) {
        conn.set_last_send_time(microtime());
    }
    Some((
        sent.map_or(-1, |sent| i64::try_from(sent).unwrap_or(-1)),
        conn,
    ))
}

/// `Server::send_to_connection(send)`: false for `SW_ERR`. Where a send fails after
/// the connection was found upstream returns `false`, which its `int` makes 0, the
/// same as `SW_OK`: true here too.
///
/// A file's payload is its `SendfileTask` and name.
#[expect(
    clippy::must_use_candidate,
    reason = "callers ignore the result where upstream's do"
)]
pub fn send_to_connection(serv: Server<'_>, send: &SendData) -> bool {
    to_connection(serv, send.info(), send.payload(), None)
}

/// [`send_to_connection`] of the event `info` with the bytes `data` it carries.
#[expect(
    clippy::must_use_candidate,
    reason = "callers ignore the result where upstream's do"
)]
pub fn send_event_to_connection(serv: Server<'_>, info: &DataHead, data: &[u8]) -> bool {
    to_connection(serv, info, data, None)
}

/// [`send_to_connection`] of the event `info` with its `data`, where `first` is what
/// [`send_now`] already sent, short of all of it.
fn to_connection(serv: Server<'_>, info: &DataHead, data: &[u8], first: Option<i64>) -> bool {
    let state = serv.state();
    let (kind, session) = (info.event(), info.fd);
    let close = kind == Some(ServerEvent::Close);
    let lookup = if close {
        Lookup::Verified
    } else {
        Lookup::VerifiedTls
    };
    let found = state.connection(session, lookup);
    let Some((conn, sock)) = found.and_then(|conn| Some((conn, conn.socket()?))) else {
        let code = E::SW_ERROR_SESSION_NOT_EXIST;
        if kind == Some(ServerEvent::SendData) {
            let length = info.len;
            let m = format_args!("send {length} byte failed, session#{session} does not exist");
            error(LOG_TRACE, code, m);
        } else {
            let kind = info.kind;
            let m = format_args!("send event[{kind}] failed, session#{session} does not exist");
            error(LOG_TRACE, code, m);
        }
        return false;
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
    if conn.overflow() && config.mode_ != Mode::Process {
        if send_yield {
            yield_error();
        } else {
            let m = format_args!("socket#{} output buffer overflow", conn.fd());
            error(LOG_WARNING, E::SW_ERROR_OUTPUT_BUFFER_OVERFLOW, m);
        }
        return false;
    }
    if close && (conn.close_reset() || conn.close_force() || conn.peer_closed()) {
        crate::conn_io::close_connection(serv, reactor, Some(sock));
        return true;
    }
    let halfclose = cfg!(feature = "halfclose");
    if matches!(kind, Some(ServerEvent::PauseRecv | ServerEvent::ResumeRecv)) {
        let (removed, notified, events) = sock.read(|s| (s.removed, s.close_notified, s.events));
        let (reading, writing) = (
            events.intersects(Events::READ),
            events.intersects(Events::WRITE),
        );
        if kind == Some(ServerEvent::PauseRecv) {
            if removed || !reading {
                return true;
            }
            return if halfclose {
                reactor.interest(sock, Interest::RemoveRead)
            } else if writing {
                reactor.change(sock, ChangeOp::Set, Events::WRITE)
            } else {
                reactor.del(*sock)
            };
        }

        let skip = if halfclose {
            let closing = conn.closed().load(core::sync::atomic::Ordering::Relaxed) != 0;
            closing || conn.peer_closed() || conn.close_queued() || notified || reading
        } else {
            !removed || reading
        };
        if skip {
            return true;
        }
        return if halfclose {
            reactor.interest(sock, Interest::AddRead)
        } else if writing {
            reactor.change(sock, ChangeOp::Set, Events::READ | Events::WRITE)
        } else {
            reactor.change(sock, ChangeOp::Add, Events::READ)
        };
    }
    let (mut data, file) = (data, kind == Some(ServerEvent::SendFile));
    let (out_empty, unbuffered, direct) = sock.read(output_state);
    if out_empty {
        if close {
            crate::conn_io::close_connection(serv, reactor, Some(sock));
            return true;
        }
        if !file && direct && direct_send(conn, *sock, &mut data, first) {
            return true;
        }
        if unbuffered {
            let buffer = Some(new_buffer(SW_SEND_BUFFER_SIZE));
            sock.edit(|s| s.output.out_buffer = buffer);
        }
    }
    let port = serv.port_by_server_fd(conn.server_fd());
    if close {
        sock.read(|s| out_buffer(s).map(swr_cpp_buffer_typed_close));
        conn.set_close_queued(true);
        if halfclose {
            reactor.interest(sock, Interest::RemoveRead);
        }
    } else if file {
        if !send_file(*sock, data) {
            return true;
        }
    } else {
        if conn.peer_closed() {
            let m = format_args!("socket#{} is closed by client", conn.fd());
            error(LOG_NOTICE, E::SW_ERROR_SESSION_CLOSED_BY_CLIENT, m);
            return true;
        }
        let callback = |index| state.callbacks.slot(index).is_set();
        let (on_full, on_empty) = (callback(BufferFull), callback(BufferEmpty));
        if sock.read(|s| queued(s) >= s.output.buffer_size) {
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
        // `Buffer::append`'s `uint32_t` size.
        let size = data.len().c_cast::<u32>() as usize;
        let bytes = sock.read(|s| append(s, data.get(..size).unwrap_or(data)));
        conn.set_send_queued_bytes(bytes);
        let high = port.map_or(u32::MAX, |port| port.config().buffer_high_watermark.get());
        if on_full && !conn.high_watermark() && bytes >= high {
            crate::server_methods::notify(serv, conn, ServerEvent::BufferFull);
            conn.set_high_watermark(true);
        }
    }
    let idle = port.map_or(0.0, |port| port.config().max_idle_time.get());
    if idle > 0.0 && sock.read(|s| s.output.send_timer.is_none()) {
        idle_timer(conn, *sock, idle);
    }
    if !sock.read(|s| s.events.intersects(Events::WRITE)) {
        reactor.interest(sock, Interest::AddWrite);
    }
    true
}

/// Whether the socket's output buffer is missing or empty, whether it is missing, and
/// whether the socket sends directly.
fn output_state(s: &SocketFields) -> (bool, bool, bool) {
    let buffer = out_buffer(s);
    let empty = buffer.is_none_or(swr_cpp_buffer_empty);
    (empty, buffer.is_none(), s.direct_send)
}

/// The bytes queued in the socket's output buffer.
fn queued(s: &SocketFields) -> u32 {
    out_buffer(s).map_or(0, swr_cpp_buffer_length)
}

/// Queues `data` in the socket's output buffer, if it has one: the bytes queued after.
fn append(s: &SocketFields, data: &[u8]) -> u32 {
    if let Some(buffer) = out_buffer(s) {
        buffer.append(data);
    }
    queued(s)
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
        let sent = swoole_rs_corebase::tls::upstream::send(sock, data, 0);
        if sent == Some(data.len()) {
            conn.set_last_send_time(microtime());
            return true;
        }
        if let Some(sent @ 1..) = sent {
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

/// `sock->sendfile_async(task->filename, task->offset, task->length)`: false when it
/// failed or the task lacks its head or name.
fn send_file(sock: Socket<'_>, task: &[u8]) -> bool {
    let word = |at: usize| <[u8; 8]>::try_from(task.get(at..at + 8)?).ok();
    let (Some(offset), Some(length)) = (word(0), word(8)) else {
        return false;
    };
    let Some(Ok(name)) = task.get(TASK_HEAD..).map(CStr::from_bytes_until_nul) else {
        return false;
    };
    let (offset, length) = (i64::from_ne_bytes(offset), usize::from_ne_bytes(length));
    swoole_rs_corebase::tls::upstream::socket_sendfile_async(sock, name, offset, length)
}

/// `swoole_timer_add(sec2msec(port->max_idle_time), true, ...)` for the socket's
/// sends.
fn idle_timer(conn: Connection<'_>, sock: Socket<'_>, idle: f64) {
    let timer = crate::connection::idle_timer(conn, idle);
    let now = steady_ms();
    sock.edit(|sock| {
        sock.timeouts.write = idle;
        sock.last_sent_time = now;
        sock.output.send_timer = timer;
    });
}
#[cfg(test)]
mod tests {
    use super::*;
    use crate::fixture::LiveServer;
    use crate::hybrid::tests::{Call, calls};

    const SW_OK: c_int = swoole_rs_corebase::constants::ResultCode::SW_OK as c_int;
    const SW_ERR: c_int = swoole_rs_corebase::constants::ResultCode::SW_ERR as c_int;
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
            let event = live.accept_event();
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
            *self.conn().socket().unwrap()
        }
        /// `send_to_connection` of an event of `kind` with `data` to `session`.
        fn send(&self, kind: ServerEvent, session: i64, data: &[u8]) -> c_int {
            let sent = send_to_connection(self.serv(), &event(kind, session, data));
            if sent { SW_OK } else { SW_ERR }
        }
        /// Whether the connection on `fd` was closed: its slot freed its socket.
        fn closed(&self, fd: c_int) -> bool {
            let state = self.live.state();
            let conn = state.connection(core::ffi::c_long::from(fd), Lookup::ByFd);
            conn.is_some_and(|conn| !conn.is_active() && conn.socket().is_none())
        }
        fn queued(&self) -> u32 {
            self.sock()
                .read(|s| out_buffer(s).map_or(0, swr_cpp_buffer_length))
        }
        fn read(&mut self, len: usize) -> Vec<u8> {
            let mut got = vec![0; len];
            self.client.read_exact(&mut got).unwrap();
            got
        }
    }

    fn event(kind: ServerEvent, session: i64, data: &[u8]) -> SendData<'_> {
        let info = DataHead {
            fd: session,
            kind: kind.code(),
            ..DataHead::default()
        };
        SendData::new(info, data)
    }

    #[test]
    fn a_missing_session_fails() {
        let a = Accepted::new();
        assert_eq!(a.send(ServerEvent::SendData, 9, b"data"), SW_ERR);
        assert_eq!(a.send(ServerEvent::Close, 9, b""), SW_ERR);
    }

    #[test]
    fn overflow_outside_process_mode_refuses() {
        let mut a = Accepted::new();
        a.conn().set_overflow(true);
        assert_eq!(
            a.send(ServerEvent::SendData, 1, b"data"),
            SW_OK,
            "process mode buffers it"
        );
        a.live.config_mut().mode_ = Mode::Base;
        assert_eq!(a.send(ServerEvent::SendData, 1, b"data"), SW_ERR);
        a.serv().config().send_yield.set(true);
        assert_eq!(a.send(ServerEvent::SendData, 1, b"data"), SW_ERR);
        assert_eq!(
            log::last_error(),
            E::SW_ERROR_OUTPUT_SEND_YIELD as c_int,
            "send_yield reports it instead"
        );
    }

    #[test]
    fn pause_and_resume_move_the_read_event() {
        let a = Accepted::new();
        let events = || {
            a.sock()
                .read(|s| (s.removed, s.events.intersects(Events::READ)))
        };
        assert_eq!(
            events(),
            (true, false),
            "the master leaves it to a reactor thread"
        );
        assert_eq!(a.send(ServerEvent::PauseRecv, 1, b""), SW_OK, "not reading");
        assert_eq!(a.send(ServerEvent::ResumeRecv, 1, b""), SW_OK);
        assert_eq!(events(), (false, true), "reading");
        assert_eq!(
            a.send(ServerEvent::ResumeRecv, 1, b""),
            SW_OK,
            "already reading"
        );
        assert_eq!(a.send(ServerEvent::PauseRecv, 1, b""), SW_OK);
        assert_eq!(events(), (true, false), "removed");
    }

    #[test]
    fn a_direct_send_reaches_the_peer() {
        let mut a = Accepted::new();
        assert_eq!(a.send(ServerEvent::SendData, 1, b"hello"), SW_OK);
        assert_eq!(a.read(5), b"hello");
        assert_eq!(a.queued(), 0, "nothing buffered");
        assert!(a.conn().last_send_time() > 0.0, "touched");
    }

    #[test]
    fn parts_go_out_in_one_write_where_the_worker_writes_to_the_socket() {
        let mut a = Accepted::new();
        assert_eq!(
            send_parts(a.serv(), 1, b"head", b"body"),
            None,
            "process mode sends through the factory"
        );
        a.live.config_mut().mode_ = Mode::Base;
        assert_eq!(send_parts(a.serv(), 1, b"head", b"body"), Some(true));
        assert_eq!(a.read(8), b"headbody");
        assert_eq!(a.queued(), 0, "nothing buffered");
        assert!(a.conn().last_send_time() > 0.0, "touched");
        a.sock().edit(|s| s.direct_send = false);
        assert_eq!(send_parts(a.serv(), 1, b"head", b"body"), None);
        assert!(a.client.set_nonblocking(true).is_ok());
        assert!((&a.client).read(&mut [0; 1]).is_err(), "nothing sent");
    }

    #[test]
    #[cfg_attr(miri, ignore = "building a 16 MiB body takes Miri over half an hour")]
    fn parts_the_socket_does_not_take_are_queued_in_order() {
        let mut a = Accepted::new();
        a.live.config_mut().mode_ = Mode::Base;
        // More than a loopback socket's buffers hold while the peer reads nothing.
        let body: Vec<u8> = (0..16usize << 20).map(|at| at.to_le_bytes()[0]).collect();
        assert_eq!(send_parts(a.serv(), 1, b"head", &body), Some(true));
        let queued = a.queued() as usize;
        assert!(queued > 0 && queued < body.len(), "the body's tail waits");
        assert_eq!(a.conn().send_queued_bytes() as usize, queued);
        assert!(
            a.sock().read(|s| s.events.intersects(Events::WRITE)),
            "waits to write"
        );
        let sent = 4 + body.len() - queued;
        let got = a.read(sent);
        assert_eq!(got.get(..4), Some(&b"head"[..]));
        assert_eq!(got.get(4..), body.get(..sent - 4), "the body's start, once");
        assert!(a.client.set_nonblocking(true).is_ok());
        assert!((&a.client).read(&mut [0; 1]).is_err(), "and no more");
    }

    #[test]
    fn without_direct_send_the_data_is_buffered_for_the_write_event() {
        let a = Accepted::new();
        a.sock().edit(|s| s.direct_send = false);
        assert_eq!(a.send(ServerEvent::SendData, 1, b"hello"), SW_OK);
        assert_eq!(a.queued(), 5);
        assert_eq!(a.conn().send_queued_bytes(), 5);
        assert!(
            a.sock().read(|s| s.events.intersects(Events::WRITE)),
            "waits to write"
        );
        a.conn().set_peer_closed(true);
        assert_eq!(
            a.send(ServerEvent::SendData, 1, b"more"),
            0,
            "closed by the client"
        );
        assert_eq!(a.queued(), 5);
    }

    #[test]
    fn a_send_made_first_is_not_repeated() {
        let mut a = Accepted::new();
        let data = b"0123456789";
        let info = *event(ServerEvent::SendData, 1, data).info();
        assert_eq!(
            send_now(a.live.state(), 1, data).map(|(sent, _)| sent),
            Some(10),
            "all of it went"
        );
        assert_eq!(a.read(10), data);
        // As if only four had gone.
        assert!(to_connection(a.serv(), &info, data, Some(4)));
        assert_eq!(a.queued(), 6, "the rest is buffered");
        let a = Accepted::new();
        assert!(to_connection(a.serv(), &info, data, Some(-1)));
        assert_eq!(a.queued(), 10, "would block: all of it is buffered");
        assert!(a.client.set_nonblocking(true).is_ok());
        assert!((&a.client).read(&mut [0; 1]).is_err(), "nothing sent");
    }

    #[test]
    fn a_full_buffer_overflows_and_the_watermark_notifies() {
        fn full(_: Server<'_>, _: &DataHead) {}
        let a = Accepted::new();
        a.sock().edit(|s| {
            s.direct_send = false;
            s.output.buffer_size = 4;
        });
        a.live.port().buffer_high_watermark.set(2);
        let callbacks = &a.live.state().callbacks;
        callbacks.set(crate::callbacks::Callback::BufferFull(full));
        calls();
        assert_eq!(a.send(ServerEvent::SendData, 1, b"abc"), SW_OK);
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
        assert_eq!(a.send(ServerEvent::SendData, 1, b"de"), SW_OK);
        assert_eq!(calls(), [], "already above the watermark");
        assert!(!a.conn().overflow());
        assert_eq!(
            a.send(ServerEvent::SendData, 1, b"f"),
            SW_OK,
            "buffered all the same"
        );
        assert!(a.conn().overflow(), "the buffer was full");
        assert_eq!(a.queued(), 6);
    }

    #[test]
    fn close_closes_now_or_queues_behind_the_output() {
        let a = Accepted::new();
        let fd = a.conn().fd();
        a.sock().edit(|s| s.direct_send = false);
        assert_eq!(a.send(ServerEvent::SendData, 1, b"hello"), SW_OK);
        assert_eq!(a.send(ServerEvent::Close, 1, b""), SW_OK);
        assert!(a.conn().close_queued(), "behind the output");
        assert!(!a.closed(fd), "not yet");

        let a = Accepted::new();
        let fd = a.conn().fd();
        assert_eq!(a.send(ServerEvent::Close, 1, b""), SW_OK);
        assert!(a.closed(fd), "closed now");
    }

    #[test]
    fn a_forced_close_skips_the_output() {
        let a = Accepted::new();
        let fd = a.conn().fd();
        a.sock().edit(|s| s.direct_send = false);
        assert_eq!(a.send(ServerEvent::SendData, 1, b"hello"), SW_OK);
        a.conn().set_close_force(true);
        assert_eq!(a.send(ServerEvent::Close, 1, b""), SW_OK);
        assert!(a.closed(fd), "closed with output still queued");
    }

    #[test]
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
        assert_eq!(a.send(ServerEvent::SendFile, 1, &file), SW_OK);
        assert!(
            a.sock().read(|s| s.events.intersects(Events::WRITE)),
            "waits to write"
        );
        assert_eq!(a.send(ServerEvent::SendFile, 1, &task(b"no-such-file")), 0);
    }

    #[test]
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
                ServerEvent::SendData.code(),
                16,
                0x0707_0707_0707_0707,
                0x0707_0707_0707_0707,
                [0; 8]
            )]
        );
        assert_eq!(counters(&a), 1, "counted");
        a.live.config_mut().mode_ = Mode::Base;
        assert!(send(a.serv(), 1, b"here"));
        assert_eq!(calls(), [], "no factory");
        assert_eq!(a.read(4), b"here");
        assert_eq!(counters(&a), 2);
        assert!(!send(a.serv(), 9, b"lost"), "no session");
        assert_eq!(counters(&a), 2);
    }
}
