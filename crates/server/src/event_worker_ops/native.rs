//! The event worker's ops on native objects (the reactor, the message bus, the pipes
//! and the connection's client certificate), and the reactor callbacks that run its
//! operations: what workers.cc's `WorkerContext::call` did.
use core::cell::Cell;
use core::ffi::{c_int, c_long, c_void};
use core::sync::atomic::Ordering;

use swoole_rs_buffer::string::CppString;
use swoole_rs_corebase::DataHead;
use swoole_rs_process::methods::{ExitStatus, swoole_get_worker_pid, swoole_get_worker_type};
use swoole_rs_process::worker_globals::with_globals;
use swoole_rs_process::worker_record::{Worker, WorkerRecord};
use swoole_rs_process::workers::ops::{
    OP_ARM_EXIT, OP_BUS_CLEAR, OP_BUS_EMPTY, OP_BUS_POP, OP_BUS_READ, OP_COMMAND_REQUEST,
    OP_CONN_UNREAD, OP_COUNT_COROUTINES, OP_DO_TASK, OP_DRAIN_WORKER_PIPES, OP_EVENT_BEGIN,
    OP_EVENT_END, OP_FREE_CLIENT_CERT, OP_LISTEN_PIPE, OP_ON_FINISH, OP_ON_PIPE_MESSAGE,
    OP_ON_WORKER_ERROR, OP_OTHER_PIPE_NONBLOCK, OP_PIPE_EVENT_WRITE, OP_PIPE_SEND_SYNC,
    OP_PORT_COUNT, OP_PORT_DEL, OP_PROTECT_SESSIONS, OP_REACTOR_DESTROYED, OP_REACTOR_ID,
    OP_REACTOR_IF_EXIT, OP_REACTOR_STOP, OP_REACTOR_TIMEOUT, OP_REACTOR_WAIT, OP_REACTOR_WAIT_EXIT,
    OP_RECV_CONN, OP_SEND_SHUTDOWN, OP_SET_ISOLATION, OP_SET_REACTOR_TIMEOUT, OP_STORE_CLIENT_CERT,
    OP_THREAD_SHUTDOWN, OP_UNLIMIT_PIPES, OP_UNREAD_PIPE_MASTER, OP_UNREAD_PIPE_WORKER,
    OP_UNREAD_THREAD_PIPE, RUN_ACCEPT_EVENT, RUN_CALL_WORKER_ERROR, RUN_CALL_WORKER_START,
    RUN_CALL_WORKER_STOP, RUN_CLEAN_CONNECTIONS, RUN_DRAIN_PIPES, RUN_KILL_WORKER,
    RUN_PIPE_RECEIVE, RUN_SEND_TO_REACTOR_THREAD, RUN_SIGNAL_INIT, RUN_START_EVENT_WORKER,
    RUN_STOP_ASYNC, RUN_TRY_EXIT, RUN_WORKER_SIGNAL, RUN_WORKER_START,
};

use super::{Context, SW_EVENT_WORKER, SW_WORKER_BUSY, SW_WORKER_IDLE};
use crate::callbacks::{CB_FINISH, CB_PACKET, CB_PIPE_MESSAGE, CB_RECEIVE, CB_WORKER_ERROR};
use crate::objects::{Bus, EVENT_READ, Reactor, set_nonblock, socket};
use crate::view;

use swoole_rs_coroutine::swoole_coroutine_count as coroutine_count;

/// `Reactor::PRIORITY_TRY_EXIT`, `PRIORITY_WORKER_CALLBACK`; `SW_FD_PIPE`.
const PRIORITY_TRY_EXIT: c_int = 4;
const PRIORITY_WORKER_CALLBACK: c_int = 6;
const SW_FD_PIPE: c_int = 3;
/// `SW_PIPE_MASTER`, `SW_SERVER_EVENT_SHUTDOWN`, `SW_TASK_WORKER`.
const PIPE_MASTER: c_int = 1;
const EVENT_SHUTDOWN: u8 = 16;
const SW_TASK_WORKER: c_int = 4;
use crate::records::{VERIFIED, VERIFIED_TLS};

/// The context's objects, each live when the op reading it runs.
#[expect(
    clippy::mut_from_ref,
    reason = "native records shared with C++, written by the worker that owns them as workers.cc did"
)]
impl Context {
    pub(super) fn state(&self) -> &crate::ServerState {
        self.server().state()
    }
    pub(super) fn config(&self) -> &crate::ServerConfig {
        self.state().config_ref()
    }
    pub(super) fn counters(&self) -> &crate::ServerCounters {
        self.server().counters()
    }
    pub(super) fn tables(&self) -> &crate::ServerTables {
        self.state().tables_ref()
    }
    pub(super) fn info(&self) -> &DataHead {
        // SAFETY: the ops reading it run with the event's live head.
        unsafe { &*self.info }
    }
    pub(super) fn worker(&self) -> &mut WorkerRecord {
        // SAFETY: the ops reading it run with the context's live worker.
        unsafe { &mut *self.worker }
    }
    /// `serv->get_worker(id)`.
    fn other(&self, id: u64) -> &mut WorkerRecord {
        let id = u16::try_from(id).unwrap_or(u16::MAX);
        let worker = self.server().worker(id);
        // SAFETY: its worker `id`, live.
        unsafe { &mut *worker }
    }
    pub(super) fn reactor(&self) -> Reactor {
        // SAFETY: the context's live reactor, on this thread.
        unsafe { Reactor::of(self.reactor) }
    }
    /// `serv->get_worker_message_bus()`.
    pub(super) fn bus(&self) -> Bus {
        crate::master_ops::worker_message_bus(self.state())
    }
    /// `serv->get_worker_pipe_*_in_message_bus(worker)`: in thread mode, the thread
    /// bus's own end of `pipe`.
    fn in_bus(&self, pipe: *mut c_void) -> *mut c_void {
        crate::objects::in_worker_bus(self.state(), self.config(), pipe)
    }
    /// `reactor->remove_read_event(pipe)` when `pipe` is still in the loop.
    fn unread(&self, pipe: *mut c_void) {
        if let Some(sock) = socket(pipe).filter(|s| s.read(|s| s.removed == 0)) {
            self.reactor().unread(sock);
        }
    }
    /// The connection the head's session names, verified (and TLS-ready when `tls`).
    fn session(&self, tls: bool) -> Option<&mut crate::ConnectionRecord> {
        let kind = if tls { VERIFIED_TLS } else { VERIFIED };
        self.state().connection(self.info().fd, kind)
    }
    fn callback(&self, index: u32, data: *mut c_void) {
        if let Some(slot) = self.state().callbacks.slot(index) {
            // SAFETY: the slot takes the server and the event data.
            unsafe { slot.call(self.serv, data) };
        }
    }
}

/// The status of a byte count, as C++'s `ssize_t` in a `uint64_t`.
fn sized(n: isize) -> u64 {
    (n as i64).cast_unsigned()
}

/// The ops on native objects. The first ones run for each received event.
#[inline(always)]
pub(super) fn native(ctx: &mut Context, op: u32, arg: u64) -> Option<u64> {
    let len = || usize::try_from(arg).unwrap_or(usize::MAX);
    Some(match op {
        OP_EVENT_BEGIN => {
            ctx.worker().set_status(SW_WORKER_BUSY);
            u64::from(ctx.info().kind)
        }
        OP_EVENT_END => {
            ctx.worker().set_status(SW_WORKER_IDLE);
            u64::from(ctx.worker().exceeded_max_request())
        }
        OP_PIPE_EVENT_WRITE => {
            let event_write = swoole_rs_reactor::event_api::swoole_event_write;
            // SAFETY: the context's pipe and the event of `len` bytes it carries.
            sized(unsafe { event_write(ctx.socket.cast(), ctx.info.cast(), len()) })
        }
        OP_PIPE_SEND_SYNC => {
            let send_sync = swoole_rs_tls::upstream::socket_send_sync;
            // SAFETY: as above.
            sized(unsafe { send_sync(ctx.socket.cast(), ctx.info.cast(), len(), 0) })
        }
        OP_RECV_CONN => {
            let info = *ctx.info();
            let Some(conn) = ctx.session(true) else {
                return Some(0);
            };
            if info.len > 0 {
                let length = ctx.bus().packet().length;
                let length = u32::try_from(length).unwrap_or(u32::MAX);
                conn.recv_queued_bytes.fetch_sub(length, Ordering::Relaxed);
            }
            conn.last_dispatch_time = info.time;
            if conn.closed.load(Ordering::Relaxed) == 0 {
                1
            } else {
                2
            }
        }
        OP_DO_TASK => {
            let packet = ctx.bus().packet();
            let mut recv = crate::reactor_entry::RecvData {
                info: *ctx.info(),
                data: packet.data.cast(),
            };
            recv.info.len = u32::try_from(packet.length).unwrap_or(u32::MAX);
            let index = if arg == 0 { CB_RECEIVE } else { CB_PACKET };
            let slot = ctx.state().callbacks.slot(index)?;
            // SAFETY: `onReceive` and `onPacket` take the server and a `RecvData`.
            if unsafe { slot.receive(ctx.serv, (&raw mut recv).cast()) } == 0 {
                ctx.worker().add_request_count();
                ctx.counters().request_count.fetch_add(1, Ordering::Relaxed);
            }
            0
        }
        OP_STORE_CLIENT_CERT => {
            if let Some(conn) = ctx.session(false) {
                let packet = ctx.bus().packet();
                conn.ssl_client_cert = client_cert(packet.data.cast(), packet.length).cast();
                conn.ssl_client_cert_pid = swoole_get_worker_pid();
            }
            0
        }
        OP_FREE_CLIENT_CERT => {
            if let Some(conn) = ctx.session(false)
                && !conn.ssl_client_cert.is_null()
                && conn.ssl_client_cert_pid == swoole_get_worker_pid()
            {
                // SAFETY: this worker's certificate, which `client_cert` made.
                let mut cert = unsafe { Box::from_raw(conn.ssl_client_cert.cast::<CppString>()) };
                cert.dispose();
                conn.ssl_client_cert = core::ptr::null_mut();
            }
            0
        }
        OP_ON_FINISH => {
            ctx.callback(CB_FINISH, ctx.bus().buffer().cast());
            0
        }
        OP_ON_PIPE_MESSAGE => {
            ctx.callback(CB_PIPE_MESSAGE, ctx.bus().buffer().cast());
            0
        }
        OP_ON_WORKER_ERROR => {
            if let Some(slot) = ctx.state().callbacks.slot(CB_WORKER_ERROR) {
                // SAFETY: `onWorkerError` takes the server, the worker and its exit status.
                unsafe { slot.dispatch(ctx.serv, ctx.worker.cast(), ctx.status.cast_mut()) };
            }
            0
        }
        OP_COMMAND_REQUEST => {
            let pair = ctx
                .config()
                .pipe_command
                .cast::<swoole_rs_tls::socket_pair::SocketPair>();
            // SAFETY: the server's live command pipe, a `UnixSocket`.
            let pipe = unsafe { (*pair).worker_socket }.cast();
            let id = u16::try_from(ctx.worker().id()).unwrap_or(0);
            let bus = crate::master_ops::server_message_bus(ctx.state());
            let pipe = crate::objects::socket(pipe);
            crate::commands::call_command_handler(ctx.server(), bus, id, pipe);
            0
        }
        OP_THREAD_SHUTDOWN => {
            let reactor = Reactor::thread();
            let id = usize::from(reactor.fields().id);
            let threads = ctx
                .tables()
                .reactor_threads
                .cast::<crate::reactor_ops::ThreadRecord>();
            let thread = threads.wrapping_add(id).cast();
            // SAFETY: the reactor thread's live record and its reactor.
            unsafe { crate::reactor_entry::thread_shutdown(thread, reactor.ptr()) };
            0
        }
        OP_SET_ISOLATION => {
            ctx.state().set_isolation();
            0
        }
        OP_OTHER_PIPE_NONBLOCK => {
            if swoole_get_worker_type() == SW_EVENT_WORKER
                && let Some(pipe) = socket(ctx.other(arg).pipe_master)
            {
                set_nonblock(pipe);
            }
            0
        }
        OP_PROTECT_SESSIONS => {
            ctx.tables().session_list.protect(libc::PROT_READ);
            0
        }
        OP_BUS_EMPTY => u64::from(ctx.bus().is_empty()),
        OP_BUS_CLEAR => {
            ctx.bus().clear();
            0
        }
        OP_BUS_READ => sized(ctx.bus().read(socket(ctx.socket)?)),
        OP_BUS_POP => {
            ctx.bus().pop();
            0
        }
        OP_SEND_SHUTDOWN => {
            let head = DataHead {
                kind: EVENT_SHUTDOWN,
                ..DataHead::default()
            };
            let worker: *mut WorkerRecord = ctx.other(arg);
            let data = (&raw const head).cast();
            let size = size_of::<DataHead>();
            // SAFETY: the live server, one of its workers and the event.
            sized(unsafe {
                swoole_rs_process::methods::send_pipe_message(worker, data, size, PIPE_MASTER)
            })
        }
        _ => return reactor_op(ctx, op, arg),
    })
}

/// The ops on the reactor.
fn reactor_op(ctx: &Context, op: u32, arg: u64) -> Option<u64> {
    Some(match op {
        OP_REACTOR_STOP => {
            ctx.reactor().fields().running = false;
            0
        }
        OP_REACTOR_WAIT_EXIT => u64::from(ctx.reactor().fields().wait_exit),
        OP_REACTOR_ID => u64::from(ctx.reactor().fields().id),
        OP_REACTOR_TIMEOUT => i64::from(ctx.reactor().fields().timeout_msec).cast_unsigned(),
        OP_SET_REACTOR_TIMEOUT => {
            ctx.reactor().fields().timeout_msec = i32::try_from(arg.cast_signed()).unwrap_or(-1);
            0
        }
        OP_REACTOR_IF_EXIT => u64::from(ctx.reactor().if_exit()),
        OP_REACTOR_WAIT => i64::from(Reactor::thread().wait()).cast_unsigned(),
        OP_REACTOR_DESTROYED => {
            Reactor::thread().fields().destroyed = true;
            0
        }
        OP_UNREAD_PIPE_WORKER => {
            ctx.unread(ctx.in_bus(ctx.worker().pipe_worker));
            0
        }
        OP_UNREAD_PIPE_MASTER => {
            ctx.unread(ctx.worker().pipe_master);
            0
        }
        OP_UNREAD_THREAD_PIPE => {
            ctx.unread(ctx.in_bus(ctx.other(arg).pipe_master));
            0
        }
        OP_CONN_UNREAD => {
            // SAFETY: the connection OP_CONN_AT selected.
            let sock = socket(unsafe { (*ctx.conn).socket })?;
            ctx.reactor().unread(sock);
            0
        }
        OP_PORT_COUNT => ctx.ports().len() as u64,
        OP_PORT_DEL => {
            let port = *ctx.ports().get(usize::try_from(arg).ok()?)?;
            port.with_socket(|sock| ctx.reactor().del(sock))?;
            0
        }
        OP_ARM_EXIT => {
            ctx.reactor().fields().wait_exit = true;
            swoole_rs_reactor::swr_v1_event_set_end_callback(PRIORITY_TRY_EXIT, try_to_exit);
            let now = crate::shared::unix_now();
            with_globals(|globals| globals.exit_time.store(now, Ordering::Relaxed));
            0
        }
        OP_DRAIN_WORKER_PIPES => {
            let worker = ctx.other(arg);
            for pipe in [worker.pipe_worker, worker.pipe_master] {
                if let Some(pipe) = socket(pipe) {
                    Reactor::thread().drain(pipe);
                }
            }
            0
        }
        OP_UNLIMIT_PIPES => {
            let worker = ctx.other(arg);
            for pipe in [worker.pipe_master, worker.pipe_worker] {
                if let Some(pipe) = socket(pipe) {
                    pipe.edit(|pipe| pipe.output.buffer_size = u32::MAX);
                }
            }
            0
        }
        OP_LISTEN_PIPE => {
            let pipe = socket(ctx.worker().pipe_worker)?;
            let reactor = Reactor::thread();
            set_nonblock(pipe);
            reactor.fields().ptr = ctx.serv;
            reactor.add(pipe, EVENT_READ);
            reactor.set_handler(SW_FD_PIPE, EVENT_READ, on_pipe_receive);
            0
        }
        OP_COUNT_COROUTINES => {
            COUNTED.set(ctx.worker);
            swoole_rs_reactor::swr_v1_event_set_end_callback(
                PRIORITY_WORKER_CALLBACK,
                count_coroutines,
            );
            0
        }
        _ => return None,
    })
}

impl Context {
    /// `serv->ports`.
    fn ports(&self) -> &[view::Port<'_>] {
        self.server().ports()
    }
}

/// `new String(data, length)`: the connection's client certificate.
fn client_cert(data: *const u8, length: usize) -> *mut CppString {
    let mut cert = Box::new(CppString::EMPTY);
    cert.alloc(length, None);
    // SAFETY: the bus packet's `length` readable bytes.
    unsafe { swoole_rs_buffer::string::swr_string_append(&mut cert, data, length) };
    Box::into_raw(cert)
}

thread_local! {
    /// The worker whose coroutines `count_coroutines` counts: this thread's event worker.
    static COUNTED: Cell<*mut WorkerRecord> = const { Cell::new(core::ptr::null_mut()) };
}

/// `worker->coroutine_num = Coroutine::count()`, after each round of the loop.
extern "C-unwind" fn count_coroutines() {
    // SAFETY: the worker OP_COUNT_COROUTINES named, live while its loop runs.
    if let Some(worker) = unsafe { COUNTED.get().as_mut() } {
        worker.coroutine_num = coroutine_count();
    }
}

/// The context over this thread's worker and `reactor`, whose `ptr` is the server
/// (or, in a task worker, its pool, whose `ptr` is).
fn worker_context(reactor: Reactor) -> Context {
    let mut serv = reactor.fields().ptr;
    if swoole_get_worker_type() == SW_TASK_WORKER {
        // SAFETY: a task worker's reactor holds its live pool.
        serv = unsafe { (*serv.cast::<swoole_rs_process::process_pool::PoolRecord>()).ptr };
    }
    context(serv, reactor)
}

/// The context over `serv`, this thread's worker and `reactor`.
fn context(serv: *mut c_void, reactor: Reactor) -> Context {
    Context {
        serv,
        worker: swoole_rs_process::worker_globals::current_worker(),
        info: core::ptr::null_mut(),
        reactor: reactor.ptr(),
        conn: core::ptr::null_mut(),
        status: core::ptr::null(),
        socket: core::ptr::null_mut(),
    }
}

/// `Worker_reactor_try_to_exit`: the loop's end callback while the worker exits.
extern "C-unwind" fn try_to_exit() {
    let mut ctx = worker_context(Reactor::thread());
    run(&mut ctx, RUN_TRY_EXIT, 0);
}

/// `Worker_onPipeReceive`: a message from a reactor thread or the master (process mode).
extern "C" fn on_pipe_receive(
    reactor: swoole_rs_reactor::Running<'_>,
    event: &mut swoole_rs_reactor::Event,
) -> c_int {
    let mut ctx = worker_context(Reactor::running(reactor));
    ctx.info = ctx.bus().buffer();
    ctx.socket = event.socket_ptr();
    run(&mut ctx, RUN_PIPE_RECEIVE, 0);
    0
}

/// Runs `operation` with `arg` over `ctx`, whose objects are live.
fn run(ctx: &mut Context, operation: u32, arg: u64) -> i64 {
    swoole_rs_process::workers::run_event_worker(ctx, operation, arg)
}

impl Context {
    /// The context over `serv`, this thread's worker and reactor.
    fn of(serv: view::Server<'_>) -> Self {
        context(serv.raw(), Reactor::thread())
    }
    /// The context over `serv`, `worker` and this thread's reactor.
    fn with(serv: view::Server<'_>, worker: Worker<'_>) -> Self {
        Self {
            worker: worker.raw(),
            ..Self::of(serv)
        }
    }
}

/// `Server::worker_signal_init()`.
pub fn worker_signal_init(serv: view::Server<'_>) {
    run(&mut Context::of(serv), RUN_SIGNAL_INIT, 0);
}

/// `Server::worker_signal_handler(signo)`, over `SwooleG.server`.
pub(crate) extern "C" fn worker_signal_handler(signo: c_int) {
    let mut ctx = context(swoole_rs_corebase::swr_v1_core_server(), Reactor::thread());
    run(
        &mut ctx,
        RUN_WORKER_SIGNAL,
        i64::from(signo).cast_unsigned(),
    );
}

/// `Server::worker_accept_event(info)`: the event whose head is `info` (its data, if
/// any, in the worker message bus).
pub(crate) fn accept_event(serv: view::Server<'_>, info: &mut DataHead) {
    let mut ctx = Context::of(serv);
    ctx.info = info;
    run(&mut ctx, RUN_ACCEPT_EVENT, 0);
}

/// `Server::worker_start_callback(worker)`.
pub(crate) fn worker_start_callback(serv: view::Server<'_>, worker: Worker<'_>) {
    run(&mut Context::with(serv, worker), RUN_WORKER_START, 0);
}

/// `Server::call_worker_start_callback(worker)`.
pub(crate) fn call_worker_start_callback(serv: view::Server<'_>, worker: Worker<'_>) {
    run(&mut Context::with(serv, worker), RUN_CALL_WORKER_START, 0);
}

/// `Server::call_worker_stop_callback(worker)`.
pub(crate) fn call_worker_stop_callback(serv: view::Server<'_>, worker: Worker<'_>) {
    run(&mut Context::with(serv, worker), RUN_CALL_WORKER_STOP, 0);
}

/// `Server::call_worker_error_callback(worker, status)`.
pub(crate) fn call_worker_error_callback(
    serv: view::Server<'_>,
    worker: Worker<'_>,
    status: &ExitStatus,
) {
    let mut ctx = Context::with(serv, worker);
    ctx.status = core::ptr::from_ref(status).cast();
    run(&mut ctx, RUN_CALL_WORKER_ERROR, 0);
}

/// `Server::kill_worker(worker_id)`: -1 for this thread's worker.
#[must_use]
pub fn kill_worker(serv: view::Server<'_>, worker_id: c_int) -> bool {
    let id = i64::from(worker_id).cast_unsigned();
    run(&mut Context::of(serv), RUN_KILL_WORKER, id) != 0
}

/// `Server::stop_async_worker(worker)`.
pub fn stop_async_worker(serv: view::Server<'_>, worker: Worker<'_>) {
    run(&mut Context::with(serv, worker), RUN_STOP_ASYNC, 0);
}

/// `Server::drain_worker_pipe()`.
pub fn drain_worker_pipe(serv: view::Server<'_>) {
    run(&mut Context::of(serv), RUN_DRAIN_PIPES, 0);
}

/// `Server::clean_worker_connections(worker)`.
pub fn clean_worker_connections(serv: view::Server<'_>, worker: Worker<'_>) {
    run(&mut Context::with(serv, worker), RUN_CLEAN_CONNECTIONS, 0);
}

/// `Server::start_event_worker(worker)`.
pub(crate) fn start_event_worker(serv: view::Server<'_>, worker: Worker<'_>) -> c_int {
    let code = run(&mut Context::with(serv, worker), RUN_START_EVENT_WORKER, 0);
    c_int::try_from(code).unwrap_or(-1)
}

/// `Server::send_to_reactor_thread(head, sizeof(*head), head->fd)`: the event `head`
/// to its session's reactor pipe.
pub(crate) fn send_to_reactor_thread(serv: view::Server<'_>, head: &DataHead) -> isize {
    let mut ctx = Context::of(serv);
    ctx.info = core::ptr::from_ref(head).cast_mut();
    // `serv->get_reactor_pipe_socket(session_id, info->reactor_id)`.
    let pipes = c_long::from(ctx.tables().reactor_pipe_num);
    let id = c_long::from(head.reactor_id)
        + (c_long::from(head.fd) % pipes) * c_long::from(ctx.config().reactor_num);
    ctx.socket = ctx.other(id.cast_unsigned()).pipe_worker;
    let size = size_of::<DataHead>() as u64;
    isize::try_from(run(&mut ctx, RUN_SEND_TO_REACTOR_THREAD, size)).unwrap_or(-1)
}
