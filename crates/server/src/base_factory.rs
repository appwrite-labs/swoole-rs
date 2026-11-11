//! `BaseFactory` (`src/server/base.cc`): base and thread mode hand events to the worker
//! in this process, send replies, and close connections. Rust reads and updates the
//! server's connection and session records directly; native code keeps the message
//! bus, sockets, output buffers and callbacks, one op each.
use crate::ops::{
    LOG_BASE_ALLOC_CONNECTIONS, LOG_BASE_ALLOC_NUMS, LOG_BASE_ALLOC_PORT_NUMS, OP_ACCEPT_EVENT,
    OP_ALLOC_CONNECTION_NUMS, OP_ALLOC_CONNECTIONS, OP_ALLOC_PORT_CONNECTION_NUMS, OP_BASE_PASS,
    OP_BASE_REACTORS, OP_CLOSE_SOCKET, OP_FORWARD, OP_HAS_ON_CLOSE, OP_NEW_BASE_FACTORY,
    OP_ON_CLOSE, OP_ON_PACKET, OP_ON_RECEIVE, OP_OUT_EMPTY, OP_PORT_COUNT, OP_QUEUE_CLOSE,
    OP_SEND_HERE,
};
use crate::records::{ConnectionRecord, VERIFIED_TLS};
use crate::{Host, SW_OK};
use core::ffi::{c_char, c_int, c_long};
use core::sync::atomic::Ordering;
use swoole_rs_corebase::DataHead;
use swoole_rs_corebase::log;
use swoole_rs_process::worker_globals::with_globals;
use swoole_rs_process::worker_record::Worker;

/// `swoole::SendData`.
#[repr(C)]
pub(crate) struct SendData {
    pub info: DataHead,
    pub data: *const c_char,
}

/// `ServerEventType`.
pub(crate) const SEND_DATA: u8 = 2;
const SEND_FILE: u8 = 3;
const CLOSE_FORWARD: u8 = 7;
const RECV_DATA: u8 = 0;
const RECV_DGRAM: u8 = 1;
/// `SW_EVENT_DATA_PTR`: `MessageBus::pass`'s flags for a payload it points at.
const DATA_PTR: u8 = 1 << 1;
/// `SW_WORKER_BUSY`, `SW_WORKER_IDLE`.
const WORKER_BUSY: u8 = 1;
const WORKER_IDLE: u8 = 2;
/// `Server::CLOSE_RESET`, `CLOSE_ACTIVELY`.
const CLOSE_RESET: u64 = 1 << 1;
const CLOSE_ACTIVELY: u64 = 1 << 2;
/// `swLog_level`'s `SW_LOG_TRACE`, `SW_LOG_NOTICE`; the `SW_ERROR_SESSION_*` codes.
const LOG_TRACE: c_int = 1;
const LOG_NOTICE: c_int = 3;
const SESSION_CLOSING: c_int = 1003;
const SESSION_CLOSED: c_int = 1004;
const SESSION_NOT_EXIST: c_int = 1005;

/// `Server::is_stream_event`.
pub(crate) fn is_stream_event(kind: u8) -> bool {
    matches!(kind, 0 | 2..=5 | 11..=14)
}

fn address<T>(value: *const T) -> u64 {
    u64::try_from(value.expose_provenance()).unwrap_or(0)
}

/// `DataHead::server_fd` is `uint16_t`; native's assignment keeps the low bits.
pub(crate) fn low16(value: c_int) -> u16 {
    u16::try_from(value.cast_unsigned() & 0xffff).unwrap_or_default()
}

/// The connection lookup of `kind` (see `ServerState::connection`).
fn connection<H: Host>(h: &mut H, key: c_long, kind: u32) -> crate::Effect<*mut ConnectionRecord> {
    Ok(h.state()?.connection_ptr(key, kind))
}

/// Whether the server has allocated its session and connection tables.
pub(crate) fn allocated(state: &crate::ServerState) -> bool {
    let tables = state.tables_ref();
    !tables.session_list.is_null() && !tables.connection_list.is_null()
}

/// The session's reactor (its worker) and fd, or `None` before the tables.
fn session<H: Host>(h: &mut H, id: c_long) -> crate::Effect<Option<(u32, c_int)>> {
    let state = h.state()?;
    if !allocated(state) {
        return Ok(None);
    }
    Ok(state
        .session(id)
        .map(|slot| (slot.reactor_bits & 0xff, slot.fd)))
}

/// A live connection record for one step; no native op runs while it is borrowed.
///
/// # Safety
/// `conn` is non-null and in the server's connection table.
unsafe fn record<'a>(conn: *mut ConnectionRecord) -> &'a mut ConnectionRecord {
    // SAFETY: as the caller guarantees.
    unsafe { &mut *conn }
}

/// onClose's event for session `fd`: `reactor_id` is -1 when the server closed it.
fn close_event(c: &ConnectionRecord, fd: c_long) -> DataHead {
    DataHead {
        fd,
        reactor_id: if c.close_actively != 0 {
            -1
        } else {
            i16::from(c.reactor_id)
        },
        server_fd: low16(c.server_fd),
        ..DataHead::default()
    }
}

/// `ProcessFactory`'s onClose for session `fd`: `call` runs with the close event
/// while the connection `conn` gives is marked closing. onClose reaches the same
/// record by session (`exist()`, `getClientInfo()`, `close()`), so `conn` is asked
/// again after it rather than a reference held across it.
#[cfg_attr(
    not(any(test, feature = "upstream-symbols")),
    expect(dead_code, reason = "only process mode's factory ops call it")
)]
pub(crate) fn on_close<'a>(
    conn: impl Fn() -> Option<&'a mut ConnectionRecord>,
    fd: c_long,
    call: impl FnOnce(&mut DataHead),
) -> Option<()> {
    let c = conn()?;
    let mut info = close_event(c, fd);
    c.closing = 1;
    call(&mut info);
    conn()?.closing = 0;
    Some(())
}

/// `Server::create_base_factory`: one reactor per worker and per-worker counters.
pub(crate) fn create<H: Host>(h: &mut H) -> crate::Effect<i64> {
    h.get(OP_BASE_REACTORS)?;
    if !h.is(OP_ALLOC_CONNECTIONS)? {
        h.get(LOG_BASE_ALLOC_CONNECTIONS)?;
        return Ok(0);
    }
    if !h.is(OP_ALLOC_CONNECTION_NUMS)? {
        h.get(LOG_BASE_ALLOC_NUMS)?;
        return Ok(0);
    }
    for port in 0..h.get(OP_PORT_COUNT)? {
        if h.op(OP_ALLOC_PORT_CONNECTION_NUMS, port)? == 0 {
            h.get(LOG_BASE_ALLOC_PORT_NUMS)?;
            return Ok(0);
        }
    }
    h.get(OP_NEW_BASE_FACTORY)?;
    Ok(1)
}

/// `BaseFactory::dispatch`: a stream event needs an active connection the server has
/// not closed and goes out under its session id; the bus hands it to this worker.
pub(crate) fn dispatch<H: Host>(
    h: &mut H,
    task: *mut SendData,
    direct: bool,
) -> crate::Effect<bool> {
    // SAFETY: native code lends a live `SendData` for the call.
    let info = unsafe { &mut (*task).info };
    if is_stream_event(info.kind) {
        let conn = connection(h, info.fd, 0)?;
        // SAFETY: a non-null lookup is a connection record.
        if conn.is_null() || unsafe { record(conn) }.active == 0 {
            log::warning(
                c"BaseFactory::dispatch",
                format_args!(
                    "dispatch[type={}] failed, socket#{} is not active",
                    info.kind, info.fd
                ),
            );
            return Ok(false);
        }
        // SAFETY: as above.
        let c = unsafe { record(conn) };
        // server active close, discard data.
        if c.closed.load(Ordering::Relaxed) != 0 {
            log::warning(
                c"BaseFactory::dispatch",
                format_args!(
                    "dispatch[type={}] failed, socket#{} is closed by server",
                    info.kind, info.fd
                ),
            );
            return Ok(false);
        }
        info.fd = c.session_id;
        info.server_fd = low16(c.server_fd);
    }
    // Upstream marks a payload that is the whole receive buffer `SW_EVENT_DATA_POP_PTR`
    // here, but `MessageBus::pass` replaces the flags of every payload with
    // `SW_EVENT_DATA_PTR`, so no worker ever sees it.
    // SAFETY: as above; no native op has run since.
    if !direct || !receive_now(h, unsafe { &*task })? {
        h.op(OP_BASE_PASS, address(task))?;
    }
    Ok(true)
}

/// `worker_accept_event` of a base-mode `RECV_DATA` or `RECV_DGRAM` without the message
/// bus. When this worker has no request limit (and, for data, the session's verified
/// connection is open), Rust does what the event's `EVENT_BEGIN`, `RECV_CONN`, `DO_TASK`
/// and `EVENT_END` did and only `onReceive` or `onPacket` is native: it gets the bytes
/// `MessageBus::pass` and `get_packet` would have handed it, under `SW_EVENT_DATA_PTR`,
/// and nothing reads the bus after them in base mode. False leaves the event to
/// `OP_BASE_PASS`, untouched.
fn receive_now<H: Host>(h: &mut H, task: &SendData) -> crate::Effect<bool> {
    let dgram = task.info.kind == RECV_DGRAM;
    if task.info.kind != RECV_DATA && !dgram {
        return Ok(false);
    }
    let (worker, max_request) = with_globals(|g| {
        (
            g.worker.load(Ordering::Relaxed),
            g.max_request.load(Ordering::Relaxed),
        )
    });
    if worker.is_null() || max_request != 0 {
        return Ok(false);
    }
    let conn = if dgram {
        core::ptr::null_mut()
    } else {
        connection(h, task.info.fd, VERIFIED_TLS)?
    };
    // SAFETY: a non-null lookup is a connection record.
    if !dgram && (conn.is_null() || unsafe { record(conn) }.closed.load(Ordering::Relaxed) != 0) {
        return Ok(false);
    }
    // SAFETY: the current worker's record, which this worker loop owns.
    let worker = unsafe { Worker::new(worker) };
    worker.set_status(WORKER_BUSY);
    if !dgram {
        // SAFETY: as above.
        let c = unsafe { record(conn) };
        if task.info.len > 0 {
            c.recv_queued_bytes
                .fetch_sub(task.info.len, Ordering::SeqCst);
        }
        c.last_dispatch_time = task.info.time;
    }
    let mut recv = SendData {
        info: task.info,
        data: task.data,
    };
    if task.info.len > 0 {
        recv.info.flags = DATA_PTR;
    }
    let callback = if dgram { OP_ON_PACKET } else { OP_ON_RECEIVE };
    if h.op(callback, address(&raw const recv))? != 0 {
        worker.add_request_count();
        if let Some(gs) = crate::shared::server_counters(h.state()?) {
            gs.request_count.fetch_add(1, Ordering::SeqCst);
        }
    }
    worker.set_status(WORKER_IDLE);
    Ok(true)
}

/// `BaseFactory::notify`: only active stream connections the server has not closed.
pub(crate) fn notify<H: Host>(h: &mut H, info: *mut DataHead) -> crate::Effect<bool> {
    // SAFETY: native code lends a live `DataHead` for the call.
    let head = unsafe { &mut *info };
    let conn = connection(h, head.fd, 0)?;
    // SAFETY: a non-null lookup is a connection record.
    if conn.is_null() || unsafe { record(conn) }.active == 0 {
        log::warning(
            c"BaseFactory::notify",
            format_args!(
                "dispatch[type={}] failed, socket#{} is not active",
                head.kind, head.fd
            ),
        );
        return Ok(false);
    }
    // SAFETY: as above.
    let c = unsafe { record(conn) };
    if c.closed.load(Ordering::Relaxed) != 0 {
        log::warning(
            c"BaseFactory::notify",
            format_args!(
                "dispatch[type={}] failed, session#{} is closed by server",
                head.kind, c.session_id
            ),
        );
        return Ok(false);
    }
    head.fd = c.session_id;
    head.server_fd = low16(c.server_fd);
    head.flags = 0;
    h.op(OP_ACCEPT_EVENT, address(info))?;
    Ok(true)
}

/// `BaseFactory::forward_message`: to the worker whose reactor owns the session.
fn forward<H: Host>(h: &mut H, send: &SendData) -> crate::Effect<bool> {
    if h.op(OP_FORWARD, address(send))? != 0 {
        return Ok(true);
    }
    log::sys_warning(
        c"BaseFactory::forward_message",
        format_args!("failed to send {} bytes to pipe_master", send.info.len),
    );
    Ok(false)
}

fn worker_id<H: Host>(h: &mut H) -> crate::Effect<u32> {
    Ok(u32::try_from(h.worker_id()? & u64::from(u32::MAX)).unwrap_or_default())
}

/// `BaseFactory::end`: forward the close to the session's worker, or mark the
/// connection, run `onClose`, and close the socket or queue the close behind its output
/// (with `--fix-halfclose`, native code then stops reading the socket).
pub(crate) fn end<H: Host>(h: &mut H, id: c_long, flags: u64) -> crate::Effect<bool> {
    let worker = worker_id(h)?;
    let Some((owner, _)) = session(h, id)?.filter(|&(_, fd)| fd != 0) else {
        log::error_log(
            LOG_TRACE,
            SESSION_NOT_EXIST,
            c"BaseFactory::end",
            format_args!("failed to close connection, session#{id} does not exist"),
        );
        return Ok(false);
    };
    if owner != worker {
        let send = SendData {
            info: DataHead {
                fd: id,
                kind: CLOSE_FORWARD,
                // `int16_t` from `swoole_get_worker_id()`.
                reactor_id: i16::from_ne_bytes(
                    u16::try_from(worker & 0xffff)
                        .unwrap_or_default()
                        .to_ne_bytes(),
                ),
                ..DataHead::default()
            },
            data: core::ptr::null(),
        };
        return forward(h, &send);
    }
    let conn = connection(h, id, 2)?;
    if conn.is_null() {
        log::set_last_error(SESSION_NOT_EXIST);
        return Ok(false);
    }
    // SAFETY: a non-null lookup is a connection record.
    let c = unsafe { record(conn) };
    if flags & CLOSE_RESET != 0 {
        c.close_reset = 1;
    }
    if flags & CLOSE_ACTIVELY != 0 {
        c.close_actively = 1;
    }
    if c.closing != 0 {
        log::error_log(
            LOG_NOTICE,
            SESSION_CLOSING,
            c"BaseFactory::end",
            format_args!("session#{id} is closing"),
        );
        return Ok(false);
    }
    if c.close_force == 0 && c.close_reset == 0 && c.closed.load(Ordering::Relaxed) != 0 {
        log::error_log(
            LOG_TRACE,
            SESSION_CLOSED,
            c"BaseFactory::end",
            format_args!("session#{id} is closed"),
        );
        return Ok(false);
    }
    c.closing = 1;
    // `if_do_close_callback`: once, by whoever sets `closed`. The slot's presence
    // is Rust's fact, so nothing else reaches the record before onClose.
    if h.is(OP_HAS_ON_CLOSE)?
        && c.closed
            .compare_exchange(0, 1, Ordering::SeqCst, Ordering::SeqCst)
            .is_ok()
    {
        let info = close_event(c, id);
        h.op(OP_ON_CLOSE, address(&raw const info))?;
    }
    // SAFETY: as above, re-derived: onClose, which reaches it by session, has returned.
    let c = unsafe { record(conn) };
    c.closing = 0;
    c.close_errno = 0;
    let socket = c.socket;
    if socket.is_null() {
        log::warning(
            c"BaseFactory::end",
            format_args!("session#{id}->socket is nullptr"),
        );
        return Ok(false);
    }
    let now = c.close_reset != 0 || c.peer_closed != 0 || c.close_force != 0;
    if h.op(OP_OUT_EMPTY, address(socket))? != 0 || now {
        return Ok(h.op(OP_CLOSE_SOCKET, address(socket))?.cast_signed() == SW_OK);
    }
    // SAFETY: as above.
    unsafe { record(conn) }.close_queued = 1;
    Ok(h.op(OP_QUEUE_CLOSE, address(socket))? != 0)
}

/// Whether `BaseFactory::finish` sends `send` from this worker: it owns the session.
pub(crate) fn sends_here<H: Host>(h: &mut H, send: &SendData) -> crate::Effect<bool> {
    let worker = worker_id(h)?;
    Ok(session(h, send.info.fd)?.is_none_or(|(owner, _)| owner == worker))
}

/// `BaseFactory::finish`: send here, or forward data and files to the session's worker.
pub(crate) fn finish<H: Host>(h: &mut H, data: *const SendData) -> crate::Effect<bool> {
    // SAFETY: native code lends a live `SendData` for the call.
    let send = unsafe { &*data };
    if sends_here(h, send)? {
        return Ok(h.signed(OP_SEND_HERE)? == SW_OK);
    }
    if send.info.kind == SEND_DATA || send.info.kind == SEND_FILE {
        return forward(h, send);
    }
    log::warning(
        c"BaseFactory::finish",
        format_args!("unknown event type[{}]", send.info.kind),
    );
    Ok(false)
}

/// The base-factory operations of `crate::run`.
pub(crate) fn run<H: Host>(h: &mut H, operation: u32, arg: u64) -> crate::Effect<Option<i64>> {
    use crate::ops::{
        RUN_BASE_CREATE, RUN_BASE_DISPATCH, RUN_BASE_END, RUN_BASE_FINISH, RUN_BASE_NOTIFY,
    };
    let pointer = |arg: u64| usize::try_from(arg).unwrap_or(0);
    Ok(Some(match operation {
        RUN_BASE_CREATE => create(h)?,
        RUN_BASE_DISPATCH => i64::from(dispatch(
            h,
            core::ptr::with_exposed_provenance_mut(pointer(arg)),
            false,
        )?),
        RUN_BASE_NOTIFY => i64::from(notify(
            h,
            core::ptr::with_exposed_provenance_mut(pointer(arg)),
        )?),
        RUN_BASE_END => i64::from(end(
            h,
            c_long::try_from(arg & 0xffff_ffff_ffff).unwrap_or_default(),
            arg >> 48,
        )?),
        RUN_BASE_FINISH => i64::from(finish(h, core::ptr::with_exposed_provenance(pointer(arg)))?),
        _ => return Ok(None),
    }))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ops::OP_WORKER_ID;
    use crate::records::{ServerTables, SessionRecord};
    use crate::script::Script;
    use core::sync::atomic::AtomicU32;
    use swoole_rs_process::worker_globals::set_current_worker;

    /// One session (id 1, fd 2, reactor/worker 0) on connection 2; worker 0 runs.
    struct Tables {
        sessions: Vec<SessionRecord>,
        connections: Vec<ConnectionRecord>,
    }

    impl Tables {
        fn new() -> Self {
            let mut t = Self {
                sessions: (0..2)
                    .map(|id| SessionRecord {
                        id,
                        fd: 0,
                        reactor_bits: 0,
                    })
                    .collect(),
                // SAFETY: the record is plain data; zero is a valid closed record.
                connections: (0..3).map(|_| unsafe { core::mem::zeroed() }).collect(),
            };
            t.sessions[1].fd = 2;
            let c = &mut t.connections[2];
            c.active = 1;
            c.fd = 2;
            c.session_id = 1;
            c.server_fd = 7;
            c.reactor_id = 3;
            c.socket = core::ptr::without_provenance_mut(0x50);
            t
        }

        fn script(&mut self, facts: &[(u32, u64)]) -> Script {
            let mut s = Script::with(facts);
            s.state = crate::ServerState::with_tables(ServerTables {
                session_list: crate::records::tests::naming(&mut self.sessions),
                connection_list: crate::records::tests::naming(&mut self.connections),
                max_connection: 2,
                ..ServerTables::default()
            });
            s
        }

        fn conn(&mut self) -> &mut ConnectionRecord {
            &mut self.connections[2]
        }
    }

    fn send(fd: c_long, kind: u8, len: u32) -> SendData {
        let mut s = SendData {
            info: DataHead::default(),
            data: core::ptr::null(),
        };
        s.info.fd = fd;
        s.info.kind = kind;
        s.info.len = len;
        s
    }

    #[test]
    fn create_fails_on_a_port() {
        let mut s = Script::with(&[
            (OP_ALLOC_CONNECTIONS, 1),
            (OP_ALLOC_CONNECTION_NUMS, 1),
            (OP_PORT_COUNT, 2),
        ])
        .queue(OP_ALLOC_PORT_CONNECTION_NUMS, &[1, 0]);
        assert_eq!(create(&mut s).unwrap(), 0, "second port failed");
        assert_eq!(s.called(LOG_BASE_ALLOC_PORT_NUMS), 1, "warned");
        assert_eq!(s.called(OP_NEW_BASE_FACTORY), 0);
    }

    #[test]
    fn dispatch_converts_stream_events_to_their_session() {
        let mut t = Tables::new();
        let mut task = send(2, 0, 5);
        let mut s = t.script(&[]);
        assert!(dispatch(&mut s, &raw mut task, true).unwrap());
        assert_eq!((task.info.fd, task.info.server_fd), (1, 7), "session id");
        assert_eq!(task.info.flags, 0);
        assert_eq!(s.called(OP_BASE_PASS), 1, "no worker here: the bus");

        let mut empty = send(2, 5, 0);
        let mut s = t.script(&[]);
        assert!(dispatch(&mut s, &raw mut empty, true).unwrap());
        assert_eq!((empty.info.flags, s.called(OP_BASE_PASS)), (0, 1));

        let mut packet = send(9, 1, 5);
        let mut s = t.script(&[]);
        assert!(
            dispatch(&mut s, &raw mut packet, true).unwrap(),
            "datagrams need no connection"
        );
        assert_eq!((packet.info.fd, s.called(OP_ON_RECEIVE)), (9, 0));

        t.conn().closed = AtomicU32::new(1);
        let mut s = t.script(&[]);
        let mut task = send(2, 0, 5);
        assert!(
            !dispatch(&mut s, &raw mut task, true).unwrap(),
            "closed by server"
        );
        t.conn().active = 0;
        assert!(!dispatch(&mut s, &raw mut task, true).unwrap(), "inactive");
        let mut far = send(3, 4, 0);
        assert!(
            !dispatch(&mut s, &raw mut far, true).unwrap(),
            "past max_connection"
        );
        assert_eq!(task.info.fd, 2, "unconverted");
        assert_eq!(s.called(OP_BASE_PASS), 0);
    }

    #[test]
    fn base_mode_receives_without_the_bus() {
        use swoole_rs_process::worker_record::WorkerRecord;
        let mut t = Tables::new();
        t.conn().recv_queued_bytes = AtomicU32::new(5);
        // SAFETY: a worker record is plain data; zero is a fresh one.
        let mut worker: WorkerRecord = unsafe { core::mem::zeroed() };
        set_current_worker(&raw mut worker);
        let mut task = send(2, RECV_DATA, 5);
        task.info.time = 1.5;
        let mut s = t.script(&[(OP_ON_RECEIVE, 1)]);
        assert!(dispatch(&mut s, &raw mut task, true).unwrap());
        assert_eq!((s.called(OP_ON_RECEIVE), s.called(OP_BASE_PASS)), (1, 0));
        // SAFETY: as above.
        assert_eq!(
            unsafe { Worker::new(&raw const worker).status() },
            WORKER_IDLE
        );
        let c = t.conn();
        assert_eq!(c.recv_queued_bytes.load(Ordering::Relaxed), 0, "unqueued");
        assert!((c.last_dispatch_time - 1.5).abs() < f64::EPSILON);

        // A datagram has no connection to check or update.
        let mut packet = send(9, RECV_DGRAM, 5);
        let mut s = t.script(&[(OP_ON_PACKET, 1)]);
        assert!(dispatch(&mut s, &raw mut packet, true).unwrap());
        assert_eq!((s.called(OP_ON_PACKET), s.called(OP_BASE_PASS)), (1, 0));
        assert_eq!(packet.info.fd, 9, "unconverted");

        // A request limit, or a factory dispatch, takes the bus.
        with_globals(|g| g.max_request.store(1, Ordering::Relaxed));
        let mut task = send(2, RECV_DATA, 5);
        let mut s = t.script(&[]);
        assert!(dispatch(&mut s, &raw mut task, true).unwrap());
        let mut again = send(2, RECV_DATA, 5);
        assert!(dispatch(&mut s, &raw mut again, false).unwrap());
        assert_eq!((s.called(OP_ON_RECEIVE), s.called(OP_BASE_PASS)), (0, 2));
        with_globals(|g| g.max_request.store(0, Ordering::Relaxed));
        set_current_worker(core::ptr::null_mut());
    }

    #[test]
    fn notify_converts_to_the_session() {
        let mut t = Tables::new();
        let mut head = send(2, 5, 0).info;
        head.flags = 9;
        let mut s = t.script(&[]);
        assert!(notify(&mut s, &raw mut head).unwrap());
        assert_eq!((head.fd, head.server_fd, head.flags), (1, 7, 0));
        assert_eq!(s.args(OP_ACCEPT_EVENT), vec![address(&raw const head)]);
        t.conn().closed = AtomicU32::new(1);
        let mut s = t.script(&[]);
        let mut head = send(2, 5, 0).info;
        assert!(!notify(&mut s, &raw mut head).unwrap(), "closed by server");
    }

    #[test]
    fn end_forwards_foreign_sessions() {
        let mut t = Tables::new();
        let mut s = t.script(&[(OP_WORKER_ID, 1), (OP_FORWARD, 1)]);
        assert!(end(&mut s, 1, CLOSE_ACTIVELY).unwrap(), "forwarded");
        assert_eq!(s.called(OP_FORWARD), 1);
        assert_eq!(t.conn().close_actively, 0, "the owner marks it");
        let mut s = t.script(&[]);
        assert!(!end(&mut s, 0, 0).unwrap(), "no session");
    }

    #[test]
    fn end_closes_once_with_the_callback() {
        let mut t = Tables::new();
        let mut s = t.script(&[(OP_HAS_ON_CLOSE, 1), (OP_OUT_EMPTY, 1)]);
        assert!(end(&mut s, 1, CLOSE_ACTIVELY).unwrap());
        assert_eq!((s.called(OP_ON_CLOSE), s.called(OP_CLOSE_SOCKET)), (1, 1));
        let c = t.conn();
        assert_eq!(
            (
                c.close_actively,
                c.closing,
                c.closed.load(Ordering::Relaxed)
            ),
            (1, 0, 1)
        );
        // Closed now: a second close stops unless forced or reset.
        let mut s = t.script(&[(OP_HAS_ON_CLOSE, 1)]);
        assert!(!end(&mut s, 1, 0).unwrap(), "already closed");
        let mut s = t.script(&[(OP_HAS_ON_CLOSE, 1)]);
        assert!(end(&mut s, 1, CLOSE_RESET).unwrap(), "reset closes now");
        assert_eq!(
            (s.called(OP_ON_CLOSE), s.called(OP_CLOSE_SOCKET)),
            (0, 1),
            "callback once"
        );
    }

    #[test]
    fn end_queues_the_close_behind_output() {
        let mut t = Tables::new();
        let mut s = t.script(&[(OP_QUEUE_CLOSE, 1)]);
        assert!(end(&mut s, 1, 0).unwrap());
        assert_eq!((s.called(OP_QUEUE_CLOSE), t.conn().close_queued), (1, 1));
        let mut s = t.script(&[]);
        t.conn().close_queued = 0;
        assert!(!end(&mut s, 1, 0).unwrap(), "the read event stayed");
        assert_eq!(t.conn().close_queued, 1, "queued all the same");
        t.conn().closing = 1;
        let mut s = t.script(&[]);
        assert!(!end(&mut s, 1, 0).unwrap(), "closing");
        t.conn().closing = 0;
        t.conn().close_force = 1;
        t.conn().socket = core::ptr::null_mut();
        assert!(!end(&mut s, 1, 0).unwrap(), "no socket");
        t.conn().active = 0;
        assert!(!end(&mut s, 1, 0).unwrap(), "no connection");
    }

    #[test]
    fn finish_sends_here_or_forwards() {
        let mut t = Tables::new();
        let data = send(1, SEND_DATA, 3);
        let mut s = t.script(&[(OP_SEND_HERE, 0)]);
        assert!(finish(&mut s, &raw const data).unwrap(), "sent here");
        let mut s = t.script(&[(OP_WORKER_ID, 1), (OP_FORWARD, 1)]);
        assert!(finish(&mut s, &raw const data).unwrap(), "forwarded");
        let mut s = t.script(&[(OP_WORKER_ID, 1)]);
        let close = send(1, 4, 0);
        assert!(!finish(&mut s, &raw const close).unwrap(), "unknown event");
        assert_eq!(s.called(OP_FORWARD), 0);
    }

    /// onClose sees its connection closing through the session lookup
    /// `$server->exist()` and `close()` use, and writes through its own
    /// reference to the slot; the caller's write after it is still valid
    /// (`cargo miri test` checks the aliasing).
    #[test]
    fn on_close_reaches_the_record_by_session() {
        let mut t = Tables::new();
        t.conn().close_actively = 1;
        let s = t.script(&[]);
        // As process mode's factory holds the slot `OP_SESSION_CONN` looked up.
        let conn = s.state.connection_ptr(1, crate::records::VERIFIED);
        let mut seen = None;
        let call = |info: &mut DataHead| {
            let c = s.state.connection(1, crate::records::BY_SESSION).unwrap();
            seen = Some((info.fd, info.reactor_id, info.server_fd, c.closing));
            c.close_errno = 104;
        };
        on_close(|| unsafe { conn.as_mut() }, 1, call).unwrap();
        assert_eq!(seen, Some((1, -1, 7, 1)), "closing during onClose");
        assert_eq!((t.conn().closing, t.conn().close_errno), (0, 104));
    }
}
