//! `BaseFactory` (`src/server/base.cc`): base and thread mode hand events to the worker
//! in this process, send replies, and close connections. The message bus and the
//! worker's event loop are the hybrid's ([`crate::hybrid`]).
#![cfg_attr(
    not(any(test, feature = "upstream-symbols")),
    expect(
        dead_code,
        reason = "only the hybrid's factory runs a base-mode server"
    )
)]
use crate::callbacks::{CallbackKind, EventSlot, ReceiveSlot};
use crate::hybrid::hybrid;
use crate::reactor_entry::RecvData;
use crate::reactor_handle::Reactor;
use crate::reactor_ops::out_buffer;
use crate::records::Lookup;
use crate::view;
use core::ffi::{c_char, c_int, c_long, c_void};
use core::marker::PhantomData;
use core::sync::atomic::Ordering;
use swoole_rs_buffer::cpp::{swr_cpp_buffer_empty, swr_cpp_buffer_typed_close};
use swoole_rs_corebase::DataHead;
use swoole_rs_corebase::ServerEvent;
use swoole_rs_corebase::{CCast, log};
use swoole_rs_memory::members::Shm;
use swoole_rs_process::methods::swoole_get_worker_id;
use swoole_rs_process::worker_globals::with_globals;
use swoole_rs_process::worker_record::{Worker, WorkerStatus};

/// `swoole::SendData`: an event's head and the payload it lends for `'a`.
///
/// The fields are private to this module so that the pair keeps one invariant, which
/// the safe readers ([`SendData::payload`], [`crate::send::send_to_connection`]) rely
/// on: `data` is null, or points at `info.len` bytes that are readable, and not written,
/// for `'a`. Only the constructors set `data` and `info.len`; the setters change the
/// rest of the head, and only the bus's writes ([`SendData::send_raw`]) lower `len`.
#[repr(C)]
#[derive(Clone, Copy)]
pub struct SendData<'a> {
    info: DataHead,
    data: *const c_char,
    lent: PhantomData<&'a [u8]>,
}

impl<'a> SendData<'a> {
    /// An event with no payload.
    #[must_use]
    pub const fn empty(info: DataHead) -> Self {
        Self {
            info,
            data: core::ptr::null(),
            lent: PhantomData,
        }
    }

    /// `info` and the bytes of `data`, whose length becomes `info.len` (at most
    /// `u32::MAX` of them).
    #[must_use]
    pub fn new(mut info: DataHead, data: &'a [u8]) -> Self {
        info.len = u32::try_from(data.len()).unwrap_or(u32::MAX);
        Self {
            info,
            data: data.as_ptr().cast(),
            lent: PhantomData,
        }
    }

    /// `info` and the payload at `data`.
    ///
    /// # Safety
    /// `data` is null, or points at `info.len` bytes that are readable, and not
    /// written, for `'a`.
    #[must_use]
    pub const unsafe fn from_raw(info: DataHead, data: *const c_char) -> Self {
        Self {
            info,
            data,
            lent: PhantomData,
        }
    }

    /// The event's head.
    #[must_use]
    pub fn info(&self) -> &DataHead {
        &self.info
    }

    /// Where the payload starts, or null without one.
    #[must_use]
    pub fn data(&self) -> *const c_char {
        self.data
    }

    /// The payload's `info.len` bytes; none for a null `data`.
    #[must_use]
    pub fn payload(&self) -> &'a [u8] {
        let len = self.info.len as usize;
        if len == 0 || self.data.is_null() {
            return &[];
        }
        // SAFETY: the fields' invariant: `info.len` bytes at `data`, readable and
        // unwritten for `'a`.
        unsafe { core::slice::from_raw_parts(self.data.cast(), len) }
    }

    /// The event as the `RecvData` a callback reads: the same head and payload.
    pub(crate) fn as_recv(&self) -> &RecvData {
        // SAFETY: `RecvData` is `repr(C)` with the same two leading fields; its
        // payload invariant is this one, for a borrow no longer than `'a`.
        unsafe { &*core::ptr::from_ref(self).cast::<RecvData>() }
    }

    /// The head's fd.
    pub(crate) fn set_fd(&mut self, fd: c_long) {
        self.info.fd = fd;
    }

    /// The head's payload length.
    ///
    /// # Safety
    /// `data` points at `len` bytes that are readable, and not written, for `'a`.
    pub(crate) unsafe fn set_len(&mut self, len: u32) {
        self.info.len = len;
    }

    /// The head's session, and the listening fd it arrived on.
    pub(crate) fn set_session(&mut self, fd: c_long, server_fd: u16) {
        self.info.fd = fd;
        self.info.server_fd = server_fd;
    }

    /// The head's reactor and server fd, as a reply's route names them.
    pub(crate) fn set_route(&mut self, reactor_id: i16, server_fd: u16) {
        self.info.reactor_id = reactor_id;
        self.info.server_fd = server_fd;
    }

    /// The head's flags.
    pub(crate) fn set_flags(&mut self, flags: u8) {
        self.info.flags = flags;
    }

    /// [`swoole_rs_bus::Bus::send_raw`] of the head and payload to `socket`.
    ///
    /// # Safety
    /// `socket` outlives any registration the write makes, as `send_raw` requires.
    #[cfg_attr(
        not(all(feature = "upstream-symbols", not(test))),
        expect(dead_code, reason = "only the hybrid's factories write to a pipe")
    )]
    pub(crate) unsafe fn send_raw(
        &mut self,
        bus: &swoole_rs_bus::Bus,
        socket: swoole_rs_reactor::Socket<'_>,
    ) -> bool {
        // SAFETY: `data` covers `info.len` bytes or is null (the fields' invariant;
        // the write only ever lowers `len`), and `socket` is as the caller promises.
        unsafe { bus.send_raw(socket, &mut self.info, self.data.cast()) }
    }
}

/// `SW_EVENT_DATA_PTR`: `MessageBus::pass`'s flags for a payload it points at.
const DATA_PTR: u8 = 1 << 1;
/// `SW_EVENT_DATA_POP_PTR`: the payload is the whole receive buffer, which the worker
/// may take.
pub(crate) const POP_PTR: u8 = 1 << 6;
/// `Server::CLOSE_RESET`, `CLOSE_ACTIVELY`.
const CLOSE_RESET: c_int = 1 << 1;
const CLOSE_ACTIVELY: c_int = 1 << 2;
/// `swLog_level`'s `SW_LOG_TRACE`, `SW_LOG_NOTICE`; the `SW_ERROR_SESSION_*` codes.
const LOG_TRACE: c_int = swoole_rs_corebase::log::LogLevel::SW_LOG_TRACE as c_int;
const LOG_NOTICE: c_int = swoole_rs_corebase::log::LogLevel::SW_LOG_NOTICE as c_int;
const SESSION_CLOSING: c_int = 1003;
const SESSION_CLOSED: c_int = 1004;
const SESSION_NOT_EXIST: c_int = 1005;

/// `DataHead::server_fd` is `uint16_t`; native's assignment keeps the low bits.
pub(crate) fn low16(value: c_int) -> u16 {
    u16::try_from(value.cast_unsigned() & 0xffff).unwrap_or_default()
}

/// Whether the server has allocated its session and connection tables.
pub(crate) fn allocated(state: &crate::ServerState) -> bool {
    let tables = state.tables_ref();
    !tables.session_list.is_null() && !tables.connection_list.is_null()
}

/// onClose's event for session `fd`: `reactor_id` is -1 when the server closed it.
fn close_event(c: crate::Connection<'_>, fd: c_long) -> DataHead {
    DataHead {
        fd,
        reactor_id: if c.close_actively() {
            -1
        } else {
            i16::from(c.reactor_id())
        },
        server_fd: low16(c.server_fd()),
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
    conn: impl Fn() -> Option<crate::Connection<'a>>,
    fd: c_long,
    call: impl FnOnce(&mut DataHead),
) -> Option<()> {
    let c = conn()?;
    let mut info = close_event(c, fd);
    c.set_closing(true);
    call(&mut info);
    conn()?.set_closing(false);
    Some(())
}

/// `Server::create_base_factory`: one reactor per worker, this process's connection
/// list and per-worker connection counts, then the factory; null when an allocation
/// fails. It runs in `Server::create`, before any other thread or process exists.
pub(crate) fn create(serv: view::Server<'_>) -> *mut c_void {
    /// `sizeof(sw_atomic_t)`.
    const ATOMIC_SIZE: usize = 4;
    let state = serv.state();
    let config = state.config_ref();
    let workers = config.worker_num.get();
    config.reactor_num.set(workers.c_cast::<u16>());
    let origin = c"Server::create_base_factory";
    // Private, as `sw_calloc`'s memory was: each base worker indexes its own fds.
    let max = state.tables_ref().max_connection as usize;
    let connections = Shm::calloc_private(max, size_of::<crate::ConnectionRecord>());
    if connections.is_null() {
        let size = max * size_of::<crate::ConnectionRecord>();
        log::sys_warning(origin, format_args!("calloc[2]({size}) failed"));
        return core::ptr::null_mut();
    }
    // SAFETY: as above; the write is the statement's only access to the tables.
    unsafe { &mut *state.tables() }.connection_list = connections;
    let counts = |what: &str| {
        let cells = Shm::calloc(workers as usize, ATOMIC_SIZE);
        if cells.is_null() {
            let size = workers as usize * ATOMIC_SIZE;
            let m = format_args!("sw_shm_calloc({size}) for {what}->connection_nums failed");
            log::sys_warning(origin, m);
        }
        (!cells.is_null()).then_some(cells)
    };
    // SAFETY: `slot` is the `connection_nums` of the server's counters, which
    // `swr_v1_server_new` mapped, or of a port's, in the `port_gs_list` `Server::create`
    // mapped; no other process shares them yet.
    let store = |slot: *mut Shm, cells| unsafe { slot.write(cells) };
    let Some(cells) = counts("gs") else {
        return core::ptr::null_mut();
    };
    let gs = config.gs.as_ptr();
    store(
        gs.wrapping_byte_add(core::mem::offset_of!(
            crate::shared::ServerCounters,
            connection_nums
        ))
        .cast(),
        cells,
    );
    for port in state.tables_ref().ports() {
        let gs = port.config().gs.get();
        if gs.is_null() {
            continue;
        }
        let Some(cells) = counts("port") else {
            return core::ptr::null_mut();
        };
        store(
            gs.wrapping_byte_add(core::mem::offset_of!(
                crate::shared::PortCounters,
                connection_nums
            ))
            .cast(),
            cells,
        );
    }
    hybrid().factory_new(serv)
}

/// `BaseFactory::dispatch`: a stream event needs an active connection the server has
/// not closed and goes out under its session id; the bus hands it to this worker.
/// `direct` (base mode's reactor) lets this worker take it without the bus.
#[inline(always)]
pub(crate) fn dispatch(serv: view::Server<'_>, task: &mut SendData, direct: bool) -> bool {
    let info = &mut task.info;
    if info.event().is_some_and(ServerEvent::is_stream) {
        let conn = serv.state().connection(info.fd, Lookup::ByFd);
        let Some(c) = conn.filter(|c| c.is_active()) else {
            log::warning(
                c"BaseFactory::dispatch",
                format_args!(
                    "dispatch[type={}] failed, socket#{} is not active",
                    info.kind, info.fd
                ),
            );
            return false;
        };
        // server active close, discard data.
        if c.closed().load(Ordering::Relaxed) != 0 {
            log::warning(
                c"BaseFactory::dispatch",
                format_args!(
                    "dispatch[type={}] failed, socket#{} is closed by server",
                    info.kind, info.fd
                ),
            );
            return false;
        }
        info.fd = c.session_id();
        info.server_fd = low16(c.server_fd());
    }
    // `MessageBus::pass` replaces the flags of every payload with `SW_EVENT_DATA_PTR`,
    // so `SW_EVENT_DATA_POP_PTR` reaches a worker only through `receive_now`.
    if !direct || !receive_now(serv, task) {
        hybrid().base_pass(serv, task);
    }
    true
}

/// `worker_accept_event` of a base-mode `RECV_DATA` or `RECV_DGRAM` without the message
/// bus. When this worker has no request limit (and, for data, the session's verified
/// connection is open), Rust does what the event's `EVENT_BEGIN`, `RECV_CONN`, `DO_TASK`
/// and `EVENT_END` did and only `onReceive` or `onPacket` is native: it gets the bytes
/// `MessageBus::pass` and `get_packet` would have handed it, under `SW_EVENT_DATA_PTR`,
/// and nothing reads the bus after them in base mode. False leaves the event to the bus,
/// untouched.
#[inline(always)]
fn receive_now(serv: view::Server<'_>, task: &SendData) -> bool {
    let dgram = task.info.event() == Some(ServerEvent::RecvDgram);
    if !dgram && task.info.event() != Some(ServerEvent::RecvData) {
        return false;
    }
    let (worker, max_request) = with_globals(|g| {
        (
            g.worker.load(Ordering::Relaxed),
            g.max_request.load(Ordering::Relaxed),
        )
    });
    if worker.is_null() || max_request != 0 {
        return false;
    }
    let state = serv.state();
    let conn = if dgram {
        None
    } else {
        let conn = state.connection(task.info.fd, Lookup::VerifiedTls);
        let Some(c) = conn.filter(|c| c.closed().load(Ordering::Relaxed) == 0) else {
            return false;
        };
        Some(c)
    };
    // SAFETY: the current worker's record, which this worker loop owns.
    let worker = unsafe { Worker::new(worker) };
    worker.set_status(WorkerStatus::Busy);
    if let Some(c) = conn {
        if task.info.len > 0 {
            c.recv_queued_bytes()
                .fetch_sub(task.info.len, Ordering::SeqCst);
        }
        c.set_last_dispatch_time(task.info.time);
    }
    let mut recv = RecvData {
        info: task.info,
        data: task.data.cast(),
    };
    if task.info.len > 0 {
        recv.info.flags = DATA_PTR | (task.info.flags & POP_PTR);
    }
    let slot = if dgram {
        ReceiveSlot::Packet
    } else {
        ReceiveSlot::Receive
    };
    if state.callbacks.receive(slot, serv, &recv) == 0 {
        worker.add_request_count();
        if let Some(gs) = crate::shared::server_counters(state) {
            gs.request_count.fetch_add(1, Ordering::SeqCst);
        }
    }
    worker.set_status(WorkerStatus::Idle);
    true
}

/// `BaseFactory::notify`: only active stream connections the server has not closed.
pub(crate) fn notify(serv: view::Server<'_>, head: &mut DataHead) -> bool {
    let conn = serv.state().connection(head.fd, Lookup::ByFd);
    let Some(c) = conn.filter(|c| c.is_active()) else {
        log::warning(
            c"BaseFactory::notify",
            format_args!(
                "dispatch[type={}] failed, socket#{} is not active",
                head.kind, head.fd
            ),
        );
        return false;
    };
    if c.closed().load(Ordering::Relaxed) != 0 {
        log::warning(
            c"BaseFactory::notify",
            format_args!(
                "dispatch[type={}] failed, session#{} is closed by server",
                head.kind,
                c.session_id()
            ),
        );
        return false;
    }
    head.fd = c.session_id();
    head.server_fd = low16(c.server_fd());
    head.flags = 0;
    hybrid().accept_event(serv, head);
    true
}

/// `BaseFactory::forward_message`: to the worker whose reactor owns the session.
fn forward(serv: view::Server<'_>, send: &SendData) -> bool {
    if hybrid().forward(serv, send) {
        return true;
    }
    log::sys_warning(
        c"BaseFactory::forward_message",
        format_args!("failed to send {} bytes to pipe_master", send.info.len),
    );
    false
}

/// The session's reactor (its worker) and fd, or `None` before the tables.
fn session(state: &crate::ServerState, id: c_long) -> Option<(u32, c_int)> {
    if !allocated(state) {
        return None;
    }
    let slot = state.session(id)?;
    Some((slot.reactor_bits() & 0xff, slot.fd()))
}

/// `BaseFactory::end`: forward the close to the session's worker, or mark the
/// connection, run `onClose`, and close the socket or queue the close behind its output
/// (with `--fix-halfclose`, it then stops reading the socket).
pub(crate) fn end(serv: view::Server<'_>, id: c_long, flags: c_int) -> bool {
    let state = serv.state();
    let worker = swoole_get_worker_id();
    let Some((owner, _)) = session(state, id).filter(|&(_, fd)| fd != 0) else {
        log::error_log(
            LOG_TRACE,
            SESSION_NOT_EXIST,
            c"BaseFactory::end",
            format_args!("failed to close connection, session#{id} does not exist"),
        );
        return false;
    };
    if owner != worker {
        let send = SendData::empty(DataHead {
            fd: id,
            kind: ServerEvent::CloseForward.code(),
            // `int16_t` from `swoole_get_worker_id()`.
            reactor_id: i16::from_ne_bytes(
                u16::try_from(worker & 0xffff)
                    .unwrap_or_default()
                    .to_ne_bytes(),
            ),
            ..DataHead::default()
        });
        return forward(serv, &send);
    }
    // The handle reads and writes one field at a time through the table's own
    // pointer, so onClose, which reaches the record by session, leaves it valid.
    let Some(c) = state.connection(id, Lookup::Verified) else {
        log::set_last_error(SESSION_NOT_EXIST);
        return false;
    };
    if flags & CLOSE_RESET != 0 {
        c.set_close_reset(true);
    }
    if flags & CLOSE_ACTIVELY != 0 {
        c.set_close_actively(true);
    }
    if c.closing() {
        log::error_log(
            LOG_NOTICE,
            SESSION_CLOSING,
            c"BaseFactory::end",
            format_args!("session#{id} is closing"),
        );
        return false;
    }
    if !c.close_force() && !c.close_reset() && c.closed().load(Ordering::Relaxed) != 0 {
        log::error_log(
            LOG_TRACE,
            SESSION_CLOSED,
            c"BaseFactory::end",
            format_args!("session#{id} is closed"),
        );
        return false;
    }
    c.set_closing(true);
    // `if_do_close_callback`: once, by whoever sets `closed`.
    let on_close = state.callbacks.slot(CallbackKind::Close);
    if on_close.is_set()
        && c.closed()
            .compare_exchange(0, 1, Ordering::SeqCst, Ordering::SeqCst)
            .is_ok()
    {
        let info = close_event(c, id);
        state.callbacks.event(EventSlot::Close, serv, &info);
    }
    c.set_closing(false);
    c.set_close_errno(0);
    let Some(socket) = c.socket() else {
        log::warning(
            c"BaseFactory::end",
            format_args!("session#{id}->socket is nullptr"),
        );
        return false;
    };
    let now = c.close_reset() || c.peer_closed() || c.close_force();
    let empty = socket.read(|s| out_buffer(s).is_none_or(|b| swr_cpp_buffer_empty(b)));
    let reactor = Reactor::thread();
    if empty || now {
        return crate::conn_io::close_connection(serv, reactor, Some(socket)) == 0;
    }
    c.set_close_queued(true);
    socket.read(|s| out_buffer(s).map(|b| swr_cpp_buffer_typed_close(b)));
    // Only the half-close drain stops reading (`SW_HALFCLOSE_DRAIN`); upstream reads on.
    !cfg!(feature = "halfclose")
        || reactor.interest(socket, swoole_rs_reactor::Interest::RemoveRead, 0) == 0
}

/// `BaseFactory::finish`: send here, or forward data and files to the session's worker.
pub(crate) fn finish(serv: view::Server<'_>, send: &SendData) -> bool {
    let worker = swoole_get_worker_id();
    let here = session(serv.state(), send.info.fd).is_none_or(|(owner, _)| owner == worker);
    if here {
        return crate::send::send_to_connection(serv, send) == 0;
    }
    if matches!(
        send.info.event(),
        Some(ServerEvent::SendData | ServerEvent::SendFile)
    ) {
        return forward(serv, send);
    }
    log::warning(
        c"BaseFactory::finish",
        format_args!("unknown event type[{}]", send.info.kind),
    );
    false
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::callbacks::{Callback, Ctx};
    use crate::conn_io::tests::{accepted, closed, queue, session};
    use crate::hybrid::tests::{Call, calls};
    use std::cell::RefCell;
    use swoole_rs_process::worker_globals::set_current_worker;

    /// An event a callback heard: kind (0 receive, 1 packet, 4 close), fd, reactor,
    /// flags and length.
    type Heard = (u8, c_long, i16, u8, u32);

    thread_local! {
        /// The events this thread's callbacks heard.
        static HEARD: RefCell<Vec<Heard>> = const { RefCell::new(Vec::new()) };
    }

    fn heard() -> Vec<Heard> {
        HEARD.with_borrow_mut(core::mem::take)
    }

    extern "C" fn on_receive(_: Ctx, _: view::Server<'_>, data: &RecvData) -> c_int {
        let i = data.info;
        HEARD.with_borrow_mut(|h| h.push((i.kind, i.fd, i.reactor_id, i.flags, i.len)));
        0
    }

    extern "C" fn closing(_: Ctx, _: view::Server<'_>, i: &DataHead) {
        HEARD.with_borrow_mut(|h| h.push((4, i.fd, i.reactor_id, i.flags, i.len)));
    }

    fn send(fd: c_long, kind: ServerEvent, len: u32) -> SendData<'static> {
        SendData::empty(DataHead {
            fd,
            kind: kind.code(),
            len,
            ..DataHead::default()
        })
    }

    #[test]
    #[cfg_attr(miri, ignore = "Miri has no epoll for the live reactor")]
    fn dispatch_converts_stream_events_to_their_session() {
        let (live, _clients) = accepted();
        let (serv, conn) = (live.server(), session(&live, 1));
        let listen = low16(live.listener_fd);
        let mut task = send(c_long::from(conn.fd()), ServerEvent::RecvData, 5);
        assert!(dispatch(serv, &mut task, true), "no worker here");
        assert_eq!(
            (task.info.fd, task.info.server_fd),
            (1, listen),
            "session id"
        );
        let mut packet = send(9, ServerEvent::RecvDgram, 5);
        assert!(
            dispatch(serv, &mut packet, false),
            "datagrams need no connection"
        );
        assert_eq!(
            calls(),
            [
                Call::BasePass(1, ServerEvent::RecvData.code(), 5),
                Call::BasePass(9, ServerEvent::RecvDgram.code(), 5)
            ],
            "the bus"
        );

        conn.closed().store(1, Ordering::Relaxed);
        let mut task = send(c_long::from(conn.fd()), ServerEvent::RecvData, 5);
        assert!(!dispatch(serv, &mut task, true), "closed by server");
        conn.active().store(0, Ordering::Relaxed);
        assert!(!dispatch(serv, &mut task, true), "inactive");
        let mut far = send(1 << 20, ServerEvent::Close, 0);
        assert!(!dispatch(serv, &mut far, true), "past max_connection");
        assert_eq!(task.info.fd, c_long::from(conn.fd()), "unconverted");
        assert_eq!(calls(), []);
    }

    #[test]
    #[cfg_attr(miri, ignore = "Miri has no epoll for the live reactor")]
    fn base_mode_receives_without_the_bus() {
        use swoole_rs_process::worker_record::WorkerRecord;
        let (live, _clients) = accepted();
        let (serv, state, conn) = (live.server(), live.state(), session(&live, 1));
        state.callbacks.set(Callback::Receive(on_receive));
        state.callbacks.set(Callback::Packet(on_receive));
        conn.recv_queued_bytes().store(5, Ordering::Relaxed);
        // SAFETY: a worker record is plain data; zero is a fresh one.
        let mut worker: WorkerRecord = unsafe { core::mem::zeroed() };
        set_current_worker(&raw mut worker);
        let mut task = send(c_long::from(conn.fd()), ServerEvent::RecvData, 5);
        task.info.time = 1.5;
        assert!(dispatch(serv, &mut task, true));
        // SAFETY: as above.
        assert!(unsafe { Worker::new(&raw const worker).is_idle() });
        assert_eq!(
            conn.recv_queued_bytes().load(Ordering::Relaxed),
            0,
            "unqueued"
        );
        assert!((conn.last_dispatch_time() - 1.5).abs() < f64::EPSILON);

        // A datagram has no connection to check or update.
        let mut packet = send(9, ServerEvent::RecvDgram, 5);
        assert!(dispatch(serv, &mut packet, true));
        // The worker hears that the payload is the reader's whole buffer.
        let mut whole = send(c_long::from(conn.fd()), ServerEvent::RecvData, 5);
        whole.set_flags(POP_PTR);
        assert!(dispatch(serv, &mut whole, true));
        assert_eq!(
            heard(),
            [
                (ServerEvent::RecvData.code(), 1, 0, DATA_PTR, 5),
                (ServerEvent::RecvDgram.code(), 9, 0, DATA_PTR, 5),
                (ServerEvent::RecvData.code(), 1, 0, DATA_PTR | POP_PTR, 5)
            ]
        );

        // A request limit, or a factory dispatch, takes the bus.
        with_globals(|g| g.max_request.store(1, Ordering::Relaxed));
        let mut task = send(c_long::from(conn.fd()), ServerEvent::RecvData, 5);
        assert!(dispatch(serv, &mut task, true));
        with_globals(|g| g.max_request.store(0, Ordering::Relaxed));
        let mut again = send(c_long::from(conn.fd()), ServerEvent::RecvData, 5);
        assert!(dispatch(serv, &mut again, false));
        set_current_worker(core::ptr::null_mut());
        assert_eq!(heard(), []);
        assert_eq!(
            calls(),
            [Call::BasePass(1, ServerEvent::RecvData.code(), 5); 2]
        );
    }

    #[test]
    #[cfg_attr(miri, ignore = "Miri has no epoll for the live reactor")]
    fn notify_converts_to_the_session() {
        let (live, _clients) = accepted();
        let (serv, conn) = (live.server(), session(&live, 1));
        let mut head = send(c_long::from(conn.fd()), ServerEvent::Connect, 0).info;
        head.flags = 9;
        assert!(notify(serv, &mut head));
        assert_eq!((head.fd, head.flags), (1, 0));
        let listen = low16(live.listener_fd);
        assert_eq!(calls(), [Call::AcceptEvent(1, 5, listen)]);
        conn.closed().store(1, Ordering::Relaxed);
        let mut head = send(c_long::from(conn.fd()), ServerEvent::Connect, 0).info;
        assert!(!notify(serv, &mut head), "closed by server");
        assert_eq!(calls(), []);
    }

    #[test]
    #[cfg_attr(miri, ignore = "Miri has no epoll for the live reactor")]
    fn end_forwards_foreign_sessions() {
        let (live, _clients) = accepted();
        let (serv, conn) = (live.server(), session(&live, 1));
        live.state().session(1).unwrap().set_reactor_bits(1);
        assert!(end(serv, 1, CLOSE_ACTIVELY), "forwarded");
        assert_eq!(
            calls(),
            [Call::Forward(1, ServerEvent::CloseForward.code(), 0)]
        );
        assert!(!conn.close_actively(), "the owner marks it");
        assert!(!end(serv, 99, 0), "no session");
    }

    #[test]
    #[cfg_attr(miri, ignore = "Miri has no epoll for the live reactor")]
    fn end_closes_once_with_the_callback() {
        let (live, _clients) = accepted();
        let serv = live.server();
        live.state().callbacks.set(Callback::Close(closing));
        let one = session(&live, 1);
        assert!(end(serv, 1, CLOSE_ACTIVELY));
        assert!(closed(one));
        assert_eq!(heard(), [(4, 1, -1, 0, 0)], "the server closed it");
        assert!(!end(serv, 1, 0), "gone");

        // Closed but not yet released: a second close stops unless forced or reset.
        let two = session(&live, 2);
        two.closed().store(1, Ordering::Relaxed);
        assert!(!end(serv, 2, 0), "already closed");
        assert!(end(serv, 2, CLOSE_RESET), "reset closes now");
        assert!(closed(two));
        assert_eq!(heard(), [], "the callback ran once");
        assert_eq!(calls(), []);
    }

    #[test]
    #[cfg_attr(miri, ignore = "Miri has no epoll for the live reactor")]
    fn end_queues_the_close_behind_output() {
        let (live, _clients) = accepted();
        let (serv, conn) = (live.server(), session(&live, 1));
        queue(conn, b"kept");
        assert!(end(serv, 1, 0));
        assert!(conn.close_queued() && conn.is_active(), "queued");
        conn.set_closing(true);
        assert!(!end(serv, 1, 0), "closing");
        conn.set_closing(false);
        conn.closed().store(0, Ordering::Relaxed);
        let sock = conn.socket().unwrap();
        conn.set_socket(None);
        conn.set_close_force(true);
        assert!(!end(serv, 1, 0), "no socket");
        conn.set_socket(Some(sock));
        conn.active().store(0, Ordering::Relaxed);
        assert!(!end(serv, 1, 0), "no connection");
        conn.active().store(1, Ordering::Relaxed);
    }

    #[test]
    #[cfg_attr(miri, ignore = "Miri has no epoll for the live reactor")]
    fn finish_sends_here_or_forwards() {
        use std::io::Read;
        let (live, [mut client, _]) = accepted();
        let serv = live.server();
        session(&live, 1);
        let data = SendData::new(send(1, ServerEvent::SendData, 3).info, b"abc");
        assert!(finish(serv, &data), "sent here");
        let mut got = [0; 3];
        client.read_exact(&mut got).unwrap();
        assert_eq!(&got, b"abc");
        live.state().session(1).unwrap().set_reactor_bits(1);
        assert!(finish(serv, &data), "forwarded");
        assert!(
            !finish(serv, &send(1, ServerEvent::Close, 0)),
            "unknown event"
        );
        assert_eq!(calls(), [Call::Forward(1, ServerEvent::SendData.code(), 0)]);
    }

    /// onClose sees its connection closing through the session lookup
    /// `$server->exist()` and `close()` use, and writes through its own
    /// reference to the slot; the caller's write after it is still valid.
    #[test]
    #[cfg_attr(miri, ignore = "Miri has no epoll for the live reactor")]
    fn on_close_reaches_the_record_by_session() {
        let (live, _clients) = accepted();
        let (state, conn) = (live.state(), session(&live, 1));
        conn.set_close_actively(true);
        let mut seen = None;
        let call = |info: &mut DataHead| {
            let c = state.connection(1, Lookup::BySession).unwrap();
            seen = Some((info.fd, info.reactor_id, c.closing()));
            c.set_close_errno(104);
        };
        on_close(|| state.connection(1, Lookup::Verified), 1, call).unwrap();
        assert_eq!(seen, Some((1, -1, true)), "closing during onClose");
        assert_eq!((conn.closing(), conn.close_errno()), (false, 104));
    }
}
