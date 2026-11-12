//! The reactor-thread entry points that were master.cc's: the reactor handlers
//! (`ReactorThread_on*`) and the `Server` and `ReactorThread` methods that build a
//! context and run one operation over it. With `upstream-symbols`, `dispatch_task`
//! keeps its upstream (Itanium) symbol, which crates/ports' protocols link. Every op
//! they make is Rust's.
use core::ffi::{c_int, c_long, c_void};

use swoole_rs_corebase::DataHead;
use swoole_rs_corebase::net_types::SocketAddress;
use swoole_rs_reactor::{Event, Running, SocketRecord};

use crate::ops::{RUN_PACKET_RECEIVED, RUN_PIPE_READ, RUN_RT_PIPE_WRITE, RUN_THREAD_SHUTDOWN};
use crate::reactor_handle::Reactor;
use crate::reactor_ops::{Facts, ReactorContext, ThreadRecord, facts};
use crate::view;

/// `swoole::RecvData`.
#[repr(C)]
pub struct RecvData {
    pub(crate) info: DataHead,
    pub(crate) data: *const c_void,
}

impl RecvData {
    /// The event's head.
    #[must_use]
    pub fn info(&self) -> &DataHead {
        &self.info
    }

    /// Where the event's payload starts: `info.len` bytes, or what the head's
    /// flags say it is.
    #[must_use]
    pub fn data(&self) -> *const c_void {
        self.data
    }

    /// The payload's `info.len` bytes; none for a null `data`.
    #[must_use]
    pub fn payload(&self) -> &[u8] {
        if self.data.is_null() {
            return &[];
        }
        // SAFETY: whoever lends a `RecvData` points `data` at `info.len` bytes live
        // for the borrow, and its fields are private to this crate.
        unsafe { core::slice::from_raw_parts(self.data.cast(), self.info.len as usize) }
    }

    /// An `onPacket` payload: the `DgramPacket` head's socket type and sender,
    /// and the datagram that follows it; `None` if the payload is too short.
    #[must_use]
    pub fn datagram(&self) -> Option<(c_int, SocketAddress, &[u8])> {
        let payload = self.payload();
        let head = payload.get(..size_of::<DgramPacket>())?;
        // SAFETY: `size_of::<DgramPacket>()` bytes, read unaligned; every field is
        // plain data.
        let head = unsafe { head.as_ptr().cast::<DgramPacket>().read_unaligned() };
        let start = size_of::<DgramPacket>();
        let data = payload.get(start..start + head.length as usize)?;
        Some((head.socket_type, head.socket_addr, data))
    }
}

/// `DgramPacket`'s head; the datagram follows.
#[repr(C)]
struct DgramPacket {
    socket_type: c_int,
    socket_addr: SocketAddress,
    length: u32,
}

/// `serv->get_thread(id)`.
pub(crate) fn thread(state: &crate::ServerState, id: usize) -> *mut c_void {
    let threads = state.tables_ref().reactor_threads;
    threads.wrapping_byte_add(id * size_of::<ThreadRecord>())
}

/// Runs `op` over `ctx`.
pub(crate) fn run(op: u32, arg: u64, ctx: &mut ReactorContext<'_>) -> i64 {
    let state = ctx.serv.state();
    crate::run_reactor_with(ctx, state, |h| crate::run(h, op, arg))
}

/// `operation` over `ctx` with the `which` facts, outside [`run`]'s match.
fn run_with<'s, 'c>(
    which: Facts,
    ctx: &'s mut ReactorContext<'c>,
    operation: impl FnOnce(&mut crate::Native<'s, 'c>, u64) -> crate::Effect<i64>,
) -> c_int {
    let state = ctx.serv.state();
    let facts = facts(&mut *ctx, state, which);
    int(crate::run_reactor_with(ctx, state, |h| operation(h, facts)))
}

#[expect(
    clippy::cast_possible_truncation,
    reason = "C++'s implicit int64_t to int"
)]
pub(crate) fn int(value: i64) -> c_int {
    value as c_int
}

/// `reactor->ptr`, a server reactor's server.
///
/// # Safety
/// `reactor` is a server's reactor, whose `ptr` is the live `Server` for `'a`.
pub(crate) unsafe fn server<'a>(reactor: Reactor) -> view::Server<'a> {
    // SAFETY: per the caller.
    unsafe { view::Server::from_raw(reactor.fields().ptr) }
}

/// A context over the server reactor at `reactor` and its server.
///
/// # Safety
/// `reactor` is a live server `Reactor`, used on its own thread, whose `ptr` is the
/// live `Server` for `'a`.
#[inline]
pub(crate) unsafe fn reactor_context<'a>(reactor: *mut c_void) -> ReactorContext<'a> {
    // SAFETY: per the caller.
    let reactor = unsafe { Reactor::of(reactor) };
    // SAFETY: per the caller.
    ReactorContext::new(unsafe { server(reactor) }, Some(reactor))
}

/// A context over the reactor, `reactor->ptr` (a reactor thread's server) and the event.
fn context<'a>(reactor: Running<'_>, event: &'a mut Event) -> ReactorContext<'a> {
    let reactor = Reactor::running(reactor);
    // SAFETY: the handlers below are registered only on server reactors.
    let mut ctx = ReactorContext::new(unsafe { server(reactor) }, Some(reactor));
    ctx.event = Some(event);
    ctx
}

/// [`context`] with the event's socket.
fn event_context<'a>(reactor: Running<'_>, event: &'a mut Event) -> ReactorContext<'a> {
    let sock = event.socket_ptr().cast();
    // SAFETY: null, or the event's socket, live while the event is.
    let sock = unsafe { swoole_rs_reactor::Socket::from_ptr(sock) };
    let mut ctx = context(reactor, event);
    ctx.sock = sock;
    ctx
}

// The handlers below are registered only on server reactors, whose `ptr` is the live
// `Server`.

/// `ReactorThread_onPacketReceived`: a UDP port's datagram.
pub extern "C" fn on_packet_received(reactor: Running<'_>, event: &mut Event) -> c_int {
    int(run(
        RUN_PACKET_RECEIVED,
        0,
        &mut event_context(reactor, event),
    ))
}

/// `ReactorThread_onClose`: the peer closed.
pub extern "C" fn on_close(reactor: Running<'_>, event: &mut Event) -> c_int {
    let mut ctx = event_context(reactor, event);
    run_with(Facts::PeerClose, &mut ctx, crate::conn_io::peer_close)
}

/// `ReactorThread_onPipeRead`: a worker's message.
pub extern "C" fn on_pipe_read(reactor: Running<'_>, event: &mut Event) -> c_int {
    let mut ctx = context(reactor, event);
    let id = usize::from(Reactor::running(reactor).fields().id);
    ctx.thread = thread(ctx.serv.state(), id);
    int(run(RUN_PIPE_READ, 0, &mut ctx))
}

/// `ReactorThread_onPipeWrite`: a worker pipe can be written.
pub extern "C" fn on_pipe_write(reactor: Running<'_>, event: &mut Event) -> c_int {
    int(run(RUN_RT_PIPE_WRITE, 0, &mut context(reactor, event)))
}

/// `ReactorThread_onRead`: a connection is readable.
pub extern "C" fn on_read(reactor: Running<'_>, event: &mut Event) -> c_int {
    let fd = c_long::from(event.fd());
    let mut ctx = event_context(reactor, event);
    ctx.conn = ctx.serv.state().connection_ptr(fd, 0);
    run_with(Facts::Read, &mut ctx, crate::conn_io::read)
}

/// `ReactorThread_onWrite`: a connection is writable.
pub extern "C" fn on_write(reactor: Running<'_>, event: &mut Event) -> c_int {
    let mut ctx = event_context(reactor, event);
    run_with(Facts::Write, &mut ctx, crate::conn_io::write)
}

/// `static int Server::close_connection(Reactor *, Socket *)`.
///
/// # Safety
/// `reactor` is a live server `Reactor` and `socket` a live session socket of it.
pub unsafe extern "C-unwind" fn close_connection(
    reactor: *mut c_void,
    socket: *mut SocketRecord,
) -> c_int {
    // SAFETY: per the caller.
    let mut ctx = unsafe { reactor_context(reactor) };
    // SAFETY: per the caller.
    ctx.sock = unsafe { swoole_rs_reactor::Socket::from_ptr(socket) };
    run_with(Facts::Close, &mut ctx, crate::conn_io::close_connection)
}

/// `void ReactorThread::shutdown(Reactor *)`.
///
/// # Safety
/// `thread` is the live `ReactorThread` that `reactor` belongs to.
pub unsafe fn thread_shutdown(thread: *mut c_void, reactor: *mut c_void) {
    // SAFETY: per the caller: the thread's reactor holds its live server.
    let mut ctx = unsafe { reactor_context(reactor) };
    ctx.thread = thread;
    run(RUN_THREAD_SHUTDOWN, 0, &mut ctx);
}

/// `void Server::init_reactor(Reactor *)`, over one of the server's reactors.
#[cfg_attr(
    test,
    expect(dead_code, reason = "its callers are the hybrid's, outside tests")
)]
pub(crate) fn init_reactor(serv: view::Server<'_>, reactor: Reactor) {
    run(
        crate::ops::RUN_INIT_REACTOR,
        0,
        &mut ReactorContext::new(serv, Some(reactor)),
    );
}

/// `int Server::start_reactor_threads()`, in the server's master.
#[must_use]
pub fn start_reactor_threads(serv: view::Server<'_>) -> c_int {
    let mut ctx = ReactorContext::new(serv, None);
    int(run(crate::ops::RUN_START_REACTOR_THREADS, 0, &mut ctx))
}

/// `static void Server::reactor_thread_main_loop(Server *, int)`, on the reactor
/// thread `reactor_id`.
pub fn reactor_thread_main_loop(serv: view::Server<'_>, reactor_id: c_int) {
    let mut ctx = ReactorContext::new(serv, None);
    let id = i64::from(reactor_id).cast_unsigned();
    run(crate::ops::RUN_REACTOR_THREAD_MAIN, id, &mut ctx);
}

/// `static int Server::dispatch_task(const Protocol *, Socket *, const RecvData *)`:
/// a data frame to the workers.
///
/// # Safety
/// `proto` is a live server port's protocol, `socket` a live session socket and
/// `rdata` the frame.
#[cfg(all(feature = "upstream-symbols", not(test)))]
#[cfg_attr(
    feature = "upstream-symbols",
    unsafe(
        export_name = "_ZN6swoole6Server13dispatch_taskEPKNS_8ProtocolEPNS_7network6SocketEPKNS_8RecvDataE"
    )
)]
pub unsafe extern "C-unwind" fn dispatch_task(
    proto: *const swoole_rs_corebase::net_types::Protocol,
    socket: *mut c_void,
    rdata: *const c_void,
) -> c_int {
    use core::sync::atomic::Ordering::SeqCst;
    // SAFETY: per the caller; a port's protocol keeps its server.
    let serv = unsafe { (*proto).private_data_2 };
    // SAFETY: per the caller, a live server's.
    let server = unsafe { view::Server::from_raw(serv) };
    let state = server.state();
    // SAFETY: per the caller, a live session socket.
    let Some(socket) = (unsafe { swoole_rs_reactor::Socket::from_ptr(socket.cast()) }) else {
        return -1;
    };
    // A session socket's object is its live connection.
    let conn = socket
        .read(|s| s.object)
        .cast::<crate::records::ConnectionRecord>();
    // SAFETY: per the caller.
    let rdata = unsafe { &*rdata.cast::<RecvData>() };
    let len = rdata.info.len;
    // The slot is read only before the dispatch, as upstream does: base and thread mode
    // run the callback inline, and a close there hands the slot to the accept that reuses
    // the fd (`OP_CC_FINISH`'s Release of `closed`), which no later read is ordered before.
    let (port, reactor_id, mut task) = {
        // SAFETY: the live connection, borrowed only for this block.
        let c = unsafe { &*conn };
        if len > 0 {
            c.recv_queued_bytes.fetch_add(len, SeqCst);
        }
        let task = crate::base_factory::SendData {
            info: DataHead {
                server_fd: crate::base_factory::low16(c.server_fd),
                reactor_id: c.reactor_id.into(),
                ext_flags: rdata.info.ext_flags,
                kind: 0, // SW_SERVER_EVENT_RECV_DATA
                time: c.last_recv_time,
                fd: c.fd.into(),
                len,
                ..DataHead::default()
            },
            data: rdata.data.cast(),
        };
        (server.port_of_fd(c.fd), c.reactor_id, task)
    };
    let mode = u64::from(state.config_ref().mode_);
    let dispatched = if mode == crate::MODE_PROCESS {
        crate::factory::dispatch(state, (&raw mut task).cast())
    } else {
        let mut ctx = ReactorContext::new(server, None);
        ctx.sock = Some(socket);
        ctx.conn = conn;
        let task = &raw mut task;
        let base = mode == crate::MODE_BASE;
        let run = |h: &mut _| crate::base_factory::dispatch(h, task, base).map(i64::from);
        crate::run_reactor_with(&mut ctx, state, run) == 1
    };
    if !dispatched {
        if len > 0 {
            // SAFETY: the connection slot, which lives as long as the server.
            unsafe { &*conn }.recv_queued_bytes.fetch_sub(len, SeqCst);
        }
        return -1;
    }
    if mode == crate::MODE_PROCESS {
        let thread = thread(state, usize::from(reactor_id)).cast::<ThreadRecord>();
        // SAFETY: the connection's live reactor thread.
        unsafe { (*thread).dispatch_count += 1 };
    }
    if let Some(gs) = crate::shared::server_counters(state) {
        gs.dispatch_count.fetch_add(1, SeqCst);
    }
    if let Some(gs) = port.and_then(|port| port.counters()) {
        gs.dispatch_count.fetch_add(1, SeqCst);
    }
    0
}

/// `void Server::join_reactor_thread()`, in the server's master.
pub fn join_reactor_thread(serv: view::Server<'_>) {
    let mut ctx = ReactorContext::new(serv, None);
    run(crate::ops::RUN_JOIN_REACTOR_THREADS, 0, &mut ctx);
}
