//! The reactor-thread entry points that were master.cc's: the reactor handlers
//! (`ReactorThread_on*`) and the `Server` and `ReactorThread` methods that build a
//! context and run one operation over it. With `upstream-symbols`, `dispatch_task`
//! keeps its upstream (Itanium) symbol, which crates/ports' protocols link. Every op
//! they make is Rust's.
use core::ffi::{c_int, c_long, c_void};
use core::ptr;

use swoole_rs_corebase::DataHead;
use swoole_rs_reactor::SocketRecord;

use crate::ops::{
    RUN_CLOSE_CONNECTION, RUN_INIT_REACTOR, RUN_JOIN_REACTOR_THREADS, RUN_PACKET_RECEIVED,
    RUN_PIPE_READ, RUN_REACTOR_THREAD_MAIN, RUN_RT_INIT, RUN_RT_PIPE_WRITE,
    RUN_START_REACTOR_THREADS, RUN_THREAD_SHUTDOWN,
};
use crate::reactor_ops::{Facts, ReactorContext, ThreadRecord, facts};
use crate::{unanswered, view};

/// `swoole::RecvData`.
#[repr(C)]
pub struct RecvData {
    pub(crate) info: DataHead,
    pub(crate) data: *const c_void,
}

/// `serv->get_thread(id)`.
pub(crate) fn thread(state: &crate::ServerState, id: usize) -> *mut c_void {
    let threads = state.tables_ref().reactor_threads;
    threads.wrapping_byte_add(id * size_of::<ThreadRecord>())
}

/// Runs `op` over `ctx`.
///
/// # Safety
/// `ctx.serv` is a live `Server` and the context's other objects are what `op` reads.
pub(crate) unsafe fn run(op: u32, arg: u64, ctx: &mut ReactorContext) -> i64 {
    // SAFETY: per the caller.
    let state = unsafe { view::Server::from_raw(ctx.serv) }.state();
    let context = ptr::from_mut(ctx).cast();
    // SAFETY: per the caller.
    unsafe { crate::run_reactor(op, arg, context, unanswered, state) }
}

/// `operation` over `ctx` with the `which` facts, outside [`run`]'s match.
///
/// # Safety
/// As [`run`].
unsafe fn run_with<'s>(
    which: Facts,
    ctx: &mut ReactorContext,
    operation: impl FnOnce(&mut crate::Native<'s, crate::ServerCall>, u64) -> crate::Effect<i64>,
) -> c_int {
    // SAFETY: per the caller.
    let state = unsafe { state(ctx.serv) };
    // SAFETY: per the caller.
    let facts = unsafe { facts(ctx, state, which) };
    // SAFETY: per the caller.
    int(unsafe { crate::run_reactor_with(ctx, state, |h| operation(h, facts)) })
}

#[expect(
    clippy::cast_possible_truncation,
    reason = "C++'s implicit int64_t to int"
)]
fn int(value: i64) -> c_int {
    value as c_int
}

/// The server's Rust-owned state.
///
/// # Safety
/// `serv` is a live `Server`.
unsafe fn state<'a>(serv: *mut c_void) -> &'a crate::ServerState {
    // SAFETY: per the caller, `serv` is a live `Server` for the call.
    unsafe { view::Server::from_raw(serv) }.state()
}

/// `reactor->ptr`, a reactor thread's server.
///
/// # Safety
/// `reactor` is a live `Reactor`.
unsafe fn server(reactor: *mut c_void) -> *mut c_void {
    // SAFETY: per the caller.
    unsafe { crate::reactor_handle::Reactor::of(reactor) }
        .fields()
        .ptr
}

/// A context over the reactor, its server and the event and its socket.
///
/// # Safety
/// `reactor` is a live server `Reactor` and `event` one of its live events.
unsafe fn event_context(reactor: *mut c_void, event: *mut c_void) -> ReactorContext {
    // SAFETY: per the caller.
    let mut ctx = ReactorContext::new(unsafe { server(reactor) }, reactor);
    ctx.event = event.cast();
    // SAFETY: per the caller.
    ctx.sock = unsafe { (*ctx.event).socket }.cast();
    ctx
}

/// `ReactorThread_onPacketReceived`: a UDP port's datagram.
///
/// # Safety
/// `reactor` is a live server `Reactor` and `event` one of its live events.
pub unsafe extern "C" fn on_packet_received(reactor: *mut c_void, event: *mut c_void) -> c_int {
    // SAFETY: per the caller.
    let mut ctx = ReactorContext::new(unsafe { server(reactor) }, reactor);
    ctx.event = event.cast();
    // SAFETY: per the caller.
    int(unsafe { run(RUN_PACKET_RECEIVED, 0, &mut ctx) })
}

/// `ReactorThread_onClose`: the peer closed.
///
/// # Safety
/// As [`on_packet_received`].
pub unsafe extern "C" fn on_close(reactor: *mut c_void, event: *mut c_void) -> c_int {
    // SAFETY: per the caller.
    let mut ctx = unsafe { event_context(reactor, event) };
    // SAFETY: per the caller.
    unsafe { run_with(Facts::PeerClose, &mut ctx, crate::conn_io::peer_close) }
}

/// `ReactorThread_onPipeRead`: a worker's message.
///
/// # Safety
/// As [`on_packet_received`], on a reactor thread's reactor.
pub unsafe extern "C" fn on_pipe_read(reactor: *mut c_void, event: *mut c_void) -> c_int {
    // SAFETY: per the caller.
    let mut ctx = ReactorContext::new(unsafe { server(reactor) }, reactor);
    ctx.event = event.cast();
    // SAFETY: per the caller.
    let id = usize::from(
        unsafe { crate::reactor_handle::Reactor::of(reactor) }
            .fields()
            .id,
    );
    // SAFETY: per the caller, a live server's.
    ctx.thread = thread(unsafe { state(ctx.serv) }, id);
    // SAFETY: per the caller.
    int(unsafe { run(RUN_PIPE_READ, 0, &mut ctx) })
}

/// `ReactorThread_onPipeWrite`: a worker pipe can be written.
///
/// # Safety
/// As [`on_packet_received`].
pub unsafe extern "C" fn on_pipe_write(reactor: *mut c_void, event: *mut c_void) -> c_int {
    // SAFETY: per the caller.
    let mut ctx = ReactorContext::new(unsafe { server(reactor) }, reactor);
    ctx.event = event.cast();
    // SAFETY: per the caller.
    int(unsafe { run(RUN_RT_PIPE_WRITE, 0, &mut ctx) })
}

/// `ReactorThread_onRead`: a connection is readable.
///
/// # Safety
/// As [`on_packet_received`].
pub unsafe extern "C" fn on_read(reactor: *mut c_void, event: *mut c_void) -> c_int {
    // SAFETY: per the caller.
    let mut ctx = unsafe { event_context(reactor, event) };
    // SAFETY: per the caller.
    let fd = c_long::from(unsafe { (*ctx.event).fd });
    // SAFETY: per the caller, a live server.
    ctx.conn = unsafe { view::Server::from_raw(ctx.serv) }
        .state()
        .connection_ptr(fd, 0);
    // SAFETY: per the caller.
    unsafe { run_with(Facts::Read, &mut ctx, crate::conn_io::read) }
}

/// `ReactorThread_onWrite`: a connection is writable.
///
/// # Safety
/// As [`on_packet_received`].
pub unsafe extern "C" fn on_write(reactor: *mut c_void, event: *mut c_void) -> c_int {
    // SAFETY: per the caller.
    let mut ctx = unsafe { event_context(reactor, event) };
    // SAFETY: per the caller.
    unsafe { run_with(Facts::Write, &mut ctx, crate::conn_io::write) }
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
    let mut ctx = ReactorContext::new(unsafe { server(reactor) }, reactor);
    ctx.sock = socket;
    // SAFETY: per the caller.
    int(unsafe { run(RUN_CLOSE_CONNECTION, 0, &mut ctx) })
}

/// `void ReactorThread::shutdown(Reactor *)`.
///
/// # Safety
/// `thread` is the live `ReactorThread` that `reactor` belongs to.
pub unsafe fn thread_shutdown(thread: *mut c_void, reactor: *mut c_void) {
    // SAFETY: per the caller.
    let mut ctx = ReactorContext::new(unsafe { server(reactor) }, reactor);
    ctx.thread = thread;
    // SAFETY: per the caller.
    unsafe { run(RUN_THREAD_SHUTDOWN, 0, &mut ctx) };
}

/// `int ReactorThread::init(Server *, Reactor *, uint16_t)`.
///
/// # Safety
/// `thread` is `serv`'s live `ReactorThread` and `reactor` its new reactor.
pub unsafe fn thread_init(
    thread: *mut c_void,
    serv: *mut c_void,
    reactor: *mut c_void,
    reactor_id: u16,
) -> c_int {
    let mut ctx = ReactorContext::new(serv, reactor);
    ctx.thread = thread;
    // SAFETY: per the caller.
    int(unsafe { run(RUN_RT_INIT, u64::from(reactor_id), &mut ctx) })
}

/// `void Server::init_reactor(Reactor *)`.
///
/// # Safety
/// `serv` is a live `Server` and `reactor` a live reactor of it.
pub unsafe fn init_reactor(serv: *mut c_void, reactor: *mut c_void) {
    // SAFETY: per the caller.
    unsafe { run(RUN_INIT_REACTOR, 0, &mut ReactorContext::new(serv, reactor)) };
}

/// `int Server::start_reactor_threads()`.
///
/// # Safety
/// `serv` is a live `Server`, in its master.
pub unsafe extern "C-unwind" fn start_reactor_threads(serv: *mut c_void) -> c_int {
    let mut ctx = ReactorContext::new(serv, ptr::null_mut());
    // SAFETY: per the caller.
    int(unsafe { run(RUN_START_REACTOR_THREADS, 0, &mut ctx) })
}

/// `static void Server::reactor_thread_main_loop(Server *, int)`.
///
/// # Safety
/// `serv` is a live `Server`, on the reactor thread `reactor_id`.
pub unsafe fn reactor_thread_main_loop(serv: *mut c_void, reactor_id: c_int) {
    let mut ctx = ReactorContext::new(serv, ptr::null_mut());
    let id = i64::from(reactor_id).cast_unsigned();
    // SAFETY: per the caller.
    unsafe { run(RUN_REACTOR_THREAD_MAIN, id, &mut ctx) };
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
    socket: *mut SocketRecord,
    rdata: *const RecvData,
) -> c_int {
    use core::sync::atomic::Ordering::SeqCst;
    // SAFETY: per the caller; a port's protocol keeps its server.
    let serv = unsafe { (*proto).private_data_2 };
    // SAFETY: per the caller, a live server's.
    let state = unsafe { state(serv) };
    // SAFETY: per the caller, a live session socket.
    let Some(socket) = (unsafe { swoole_rs_reactor::Socket::from_ptr(socket) }) else {
        return -1;
    };
    // A session socket's object is its live connection.
    let conn = socket
        .read(|s| s.object)
        .cast::<crate::records::ConnectionRecord>();
    // SAFETY: per the caller.
    let rdata = unsafe { &*rdata };
    let len = rdata.info.len;
    let mut task = {
        // SAFETY: the live connection, borrowed only for this block.
        let c = unsafe { &*conn };
        if len > 0 {
            c.recv_queued_bytes.fetch_add(len, SeqCst);
        }
        crate::base_factory::SendData {
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
        }
    };
    let mode = u64::from(state.config_ref().mode_);
    let dispatched = if mode == crate::MODE_PROCESS {
        crate::factory::dispatch(state, (&raw mut task).cast())
    } else {
        let mut ctx = ReactorContext::new(serv, ptr::null_mut());
        ctx.sock = socket.as_ptr();
        ctx.conn = conn;
        let task = &raw mut task;
        let base = mode == crate::MODE_BASE;
        let run = |h: &mut _| crate::base_factory::dispatch(h, task, base).map(i64::from);
        // SAFETY: per the caller; the task outlives the run.
        unsafe { crate::run_reactor_with(&mut ctx, state, run) == 1 }
    };
    // SAFETY: the live connection; the dispatch has returned.
    let c = unsafe { &*conn };
    if !dispatched {
        if len > 0 {
            c.recv_queued_bytes.fetch_sub(len, SeqCst);
        }
        return -1;
    }
    if mode == crate::MODE_PROCESS {
        let thread = thread(state, usize::from(c.reactor_id)).cast::<ThreadRecord>();
        // SAFETY: the connection's live reactor thread.
        unsafe { (*thread).dispatch_count += 1 };
    }
    if let Some(gs) = crate::shared::server_counters(state) {
        gs.dispatch_count.fetch_add(1, SeqCst);
    }
    let port = crate::reactor_ops::port_by_fd(state, c.fd);
    // SAFETY: the connection's live port, whose counters live as long as it.
    if let Some(gs) = unsafe { crate::port_config::of(port) }.counters() {
        gs.dispatch_count.fetch_add(1, SeqCst);
    }
    0
}

/// `void Server::join_reactor_thread()`.
///
/// # Safety
/// `serv` is a live `Server`, in its master.
pub unsafe extern "C-unwind" fn join_reactor_thread(serv: *mut c_void) {
    let mut ctx = ReactorContext::new(serv, ptr::null_mut());
    // SAFETY: per the caller.
    unsafe { run(RUN_JOIN_REACTOR_THREADS, 0, &mut ctx) };
}
