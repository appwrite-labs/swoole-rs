//! The public `Server` methods of `src/server/master.cc`, and the `BaseFactory`
//! methods that were `src/server/base.cc`, that are one operation over a fresh
//! context, defined under their upstream (Itanium) symbols when the hybrid links
//! this crate (`upstream-symbols`). Each method fills the context the way its C++
//! forward did and runs over it in Rust.

use core::ffi::{c_char, c_int, c_long, c_void};
use core::ptr;

use crate::base_factory::{SEND_DATA, SendData};
use crate::ops::{OP_FACTORY_END, OP_FACTORY_NOTIFY, OP_PIPE_TO_WORKER};
use crate::ops::{
    RUN_ACCEPT, RUN_CREATE, RUN_DESTROY, RUN_INCOMING, RUN_SIGNAL, RUN_START, RUN_START_CHECK,
    RUN_START_MASTER_THREAD, RUN_STORE_LISTEN_SOCKET,
};
use crate::ops::{
    RUN_ADD_CONNECTION, RUN_ADD_PORT, RUN_BASE_CREATE, RUN_FEEDBACK, RUN_IDLE_TASK_WORKERS,
    RUN_IDLE_WORKERS, RUN_KERNEL_NOBUFS, RUN_NATIVE_OP, RUN_SC_SEND, RUN_SCHEDULE_WORKER,
    RUN_SENDFILE, RUN_SENDWAIT, RUN_SET_MAX_CONNECTION, RUN_SHUTDOWN, RUN_ST_ABORT,
    RUN_ST_ADD_COMMAND, RUN_ST_CLEAR_TIMER, RUN_ST_COMMAND, RUN_ST_DOCUMENT_ROOT, RUN_ST_SEND,
    RUN_ST_STORE_PIPE_FD, RUN_SYSTEMD_SOCKETS, RUN_TASKING,
};
use crate::records::ConnectionRecord;
use crate::view;
use swoole_rs_corebase::{DataHead, hook, log};

/// What an operation acts on besides the server.
#[repr(C)]
pub struct NativeContext {
    pub(crate) serv: *mut c_void,
    pub(crate) reactor: *mut c_void,
    pub(crate) event: *mut c_void,
    pub(crate) port: *mut c_void,
    pub(crate) sock: *mut c_void,
    pub(crate) conn: *mut c_void,
    pub(crate) server_fd: c_int,
    pub(crate) reactor_id: u8,
    pub(crate) send: *mut c_void,
    pub(crate) data: *const c_void,
    pub(crate) length: u32,
    pub(crate) session_id: c_long,
    pub(crate) buffer: *mut c_char,
    pub(crate) offset: i64,
    pub(crate) socket_type: c_int,
    pub(crate) host: *const c_char,
    pub(crate) number: c_int,
    pub(crate) object: *mut c_void,
}

impl NativeContext {
    fn new(serv: *mut c_void) -> Self {
        Self {
            serv,
            reactor: ptr::null_mut(),
            event: ptr::null_mut(),
            port: ptr::null_mut(),
            sock: ptr::null_mut(),
            conn: ptr::null_mut(),
            server_fd: 0,
            reactor_id: 0,
            send: ptr::null_mut(),
            data: ptr::null(),
            length: 0,
            session_id: 0,
            buffer: ptr::null_mut(),
            offset: 0,
            socket_type: 0,
            host: ptr::null(),
            number: 0,
            object: ptr::null_mut(),
        }
    }
}

/// Runs `op` over `ctx`: straight into Rust, or the tests' fake runner.
///
/// # Safety
/// `ctx.serv` is a live `Server`, and the other fields are what `op` reads.
unsafe fn run(op: u32, arg: u64, ctx: &mut NativeContext) -> i64 {
    #[cfg(not(test))]
    let run = crate::run_method;
    #[cfg(test)]
    let run = tests::fake;
    // SAFETY: as the caller guarantees.
    unsafe { run(op, arg, ctx) }
}

/// Runs `op` with `arg` over a context that lends `serv` and nothing else.
fn run_on(serv: view::Server<'_>, op: u32, arg: u64) -> i64 {
    // SAFETY: a live server, and no other object for the operation to read.
    unsafe { run(op, arg, &mut NativeContext::new(serv.raw())) }
}

/// Each method that is one operation over the server alone: its symbol when the C++ test
/// fixtures call it (then it takes the raw `Server *`), Rust name, return type and
/// operation. `bool` and `int` returns narrow like C++.
macro_rules! plain_methods {
    ($($symbol:literal fn $name:ident() -> $r:ty = $op:ident;)*) => {$(
        #[doc = concat!("`Server::", stringify!($name), "`.")]
        ///
        /// # Safety
        /// `serv` is a live `Server`.
        #[cfg_attr(feature = "upstream-symbols", unsafe(export_name = $symbol))]
        pub unsafe extern "C-unwind" fn $name(serv: *mut c_void) -> $r {
            // SAFETY: as the caller guarantees.
            Narrow::narrow(run_on(unsafe { view::Server::from_raw(serv) }, $op, 0))
        }
    )*};
    ($(fn $name:ident() -> $r:ty = $op:ident;)*) => {$(
        #[doc = concat!("`Server::", stringify!($name), "`.")]
        pub fn $name(serv: view::Server<'_>) -> $r {
            Narrow::narrow(run_on(serv, $op, 0))
        }
    )*};
}

/// A method's C++ return type, from the runner's `i64`.
trait Narrow {
    fn narrow(value: i64) -> Self;
}

impl Narrow for () {
    fn narrow(_: i64) {}
}

impl Narrow for bool {
    fn narrow(value: i64) -> bool {
        value != 0
    }
}

impl Narrow for c_int {
    #[expect(
        clippy::cast_possible_truncation,
        reason = "the C++ method's (int) conversion of the runner's result"
    )]
    fn narrow(value: i64) -> c_int {
        value as c_int
    }
}

impl Narrow for u32 {
    #[expect(
        clippy::cast_possible_truncation,
        clippy::cast_sign_loss,
        reason = "the C++ method's (uint32_t) conversion of the runner's result"
    )]
    fn narrow(value: i64) -> u32 {
        value as u32
    }
}

plain_methods! {
    "_ZN6swoole6Server8shutdownEv" fn shutdown() -> bool = RUN_SHUTDOWN;
    "_ZNK6swoole6Server19get_idle_worker_numEv" fn get_idle_worker_num() -> u32 = RUN_IDLE_WORKERS;
    "_ZN6swoole6Server5startEv" fn start() -> c_int = RUN_START;
}

plain_methods! {
    fn clear_timer() -> () = RUN_ST_CLEAR_TIMER;
    fn add_systemd_socket() -> c_int = RUN_SYSTEMD_SOCKETS;
    fn get_idle_task_worker_num() -> c_int = RUN_IDLE_TASK_WORKERS;
    fn get_tasking_num() -> c_int = RUN_TASKING;
    fn start_check() -> c_int = RUN_START_CHECK;
    fn store_listen_socket() -> () = RUN_STORE_LISTEN_SOCKET;
}

/// `Server::master_signal_handler(signo)`: the signal over `SwooleG.server`.
///
/// # Safety
/// Runs as a signal handler while `SwooleG.server` is live.
pub unsafe extern "C-unwind" fn master_signal_handler(signo: c_int) {
    let mut ctx = NativeContext::new(swoole_rs_corebase::swr_v1_core_server());
    // SAFETY: as the caller guarantees.
    unsafe { run(RUN_SIGNAL, i64::from(signo).cast_unsigned(), &mut ctx) };
}

/// `SW_GLOBAL_HOOK_BEFORE_SERVER_CREATE`.
const HOOK_BEFORE_SERVER_CREATE: c_int = 10;

/// `Server::create()`: `SW_ERR` once created, else the hook and then startup's create.
///
/// # Safety
/// `serv` is a live `Server`.
#[cfg_attr(
    feature = "upstream-symbols",
    unsafe(export_name = "_ZN6swoole6Server6createEv")
)]
pub unsafe extern "C-unwind" fn create(serv: *mut c_void) -> c_int {
    // SAFETY: per the caller, `serv` is a live `Server` for the call.
    let serv = unsafe { view::Server::from_raw(serv) };
    if !serv.config().factory_.is_null() {
        return -1;
    }
    if hook::swoole_isset_hook(HOOK_BEFORE_SERVER_CREATE) {
        // SAFETY: the hook's callbacks take the server.
        unsafe { hook::swoole_call_hook(HOOK_BEFORE_SERVER_CREATE, serv.raw()) };
    }
    c_int::narrow(run_on(serv, RUN_CREATE, 0))
}

/// `Server::destroy()`: nothing before the server is created.
///
/// # Safety
/// `serv` is a live `Server`.
#[cfg_attr(
    feature = "upstream-symbols",
    unsafe(export_name = "_ZN6swoole6Server7destroyEv")
)]
pub unsafe extern "C-unwind" fn destroy(serv: *mut c_void) {
    // SAFETY: per the caller, `serv` is a live `Server` for the call.
    let serv = unsafe { view::Server::from_raw(serv) };
    if !serv.config().factory_.is_null() {
        run_on(serv, RUN_DESTROY, 0);
    }
}

/// The module whose context each `Server` and `ThreadFactory` method of
/// crates/process's entry.rs runs its operation over, as workers.cc's
/// `workers_enter` built it.
#[cfg(all(feature = "upstream-symbols", not(test)))]
pub(crate) fn workers_enter(operation: u32) -> swoole_rs_process::workers::entry::WorkersEnter {
    use swoole_rs_process::workers::ops::{
        RUN_ACCEPT_EVENT, RUN_CALL_WORKER_ERROR, RUN_CALL_WORKER_START, RUN_CALL_WORKER_STOP,
        RUN_CLEAN_CONNECTIONS, RUN_CREATE_THREAD_FACTORY, RUN_DESTROY_THREAD_FACTORY,
        RUN_DRAIN_PIPES, RUN_FINISH, RUN_INIT_TASK_WORKERS, RUN_KILL_WORKER,
        RUN_REACTOR_PROCESS_MAIN, RUN_REACTOR_PROCESS_START, RUN_READ_WORKER_MESSAGE, RUN_RELOAD,
        RUN_RESTART_WORKER, RUN_SEND_TO_REACTOR_THREAD, RUN_SIGNAL_INIT, RUN_START_EVENT_WORKER,
        RUN_START_MANAGER, RUN_START_WORKER_THREADS, RUN_STOP_ASYNC, RUN_STOP_WORKER_THREADS,
        RUN_THREAD_FACTORY_START, RUN_THREAD_ID, RUN_THREAD_MANAGER_WAIT, RUN_THREAD_RELOAD,
        RUN_THREAD_TERMINATE_MANAGER, RUN_WORKER_SIGNAL, RUN_WORKER_START,
    };
    match operation {
        RUN_SIGNAL_INIT
        | RUN_WORKER_SIGNAL
        | RUN_ACCEPT_EVENT
        | RUN_SEND_TO_REACTOR_THREAD
        | RUN_CALL_WORKER_ERROR
        | RUN_KILL_WORKER
        | RUN_DRAIN_PIPES
        | RUN_WORKER_START
        | RUN_CALL_WORKER_START
        | RUN_CALL_WORKER_STOP
        | RUN_STOP_ASYNC
        | RUN_CLEAN_CONNECTIONS
        | RUN_START_EVENT_WORKER => crate::event_worker_ops::enter,
        RUN_INIT_TASK_WORKERS | RUN_FINISH => crate::task_worker_ops::enter,
        RUN_REACTOR_PROCESS_START | RUN_REACTOR_PROCESS_MAIN => crate::reactor_process_ops::enter,
        RUN_START_MANAGER | RUN_RELOAD | RUN_RESTART_WORKER | RUN_READ_WORKER_MESSAGE => {
            crate::manager_ops::enter
        }
        RUN_CREATE_THREAD_FACTORY
        | RUN_DESTROY_THREAD_FACTORY
        | RUN_THREAD_FACTORY_START
        | RUN_THREAD_MANAGER_WAIT
        | RUN_THREAD_RELOAD
        | RUN_THREAD_TERMINATE_MANAGER
        | RUN_THREAD_ID
        | RUN_START_WORKER_THREADS
        | RUN_STOP_WORKER_THREADS => crate::thread_mode_ops::enter,
        _ => std::process::abort(),
    }
}

/// `Server::start_master_thread(reactor)`.
///
/// # Safety
/// `serv` is a live `Server` and `reactor` the master's reactor.
#[cfg_attr(
    feature = "upstream-symbols",
    unsafe(export_name = "_ZN6swoole6Server19start_master_threadEPNS_7ReactorE")
)]
pub unsafe extern "C-unwind" fn start_master_thread(
    serv: *mut c_void,
    reactor: *mut c_void,
) -> c_int {
    let mut ctx = NativeContext::new(serv);
    ctx.reactor = reactor;
    // SAFETY: as the caller guarantees.
    c_int::narrow(unsafe { run(RUN_START_MASTER_THREAD, 0, &mut ctx) })
}

/// `Server::init_pipe_sockets(mb)`, once the server has started.
///
/// # Safety
/// `mb` is a live `MessageBus`.
#[cfg(all(feature = "upstream-symbols", not(test)))]
pub(crate) unsafe fn init_pipe_sockets(serv: view::Server<'_>, mb: *mut c_void) {
    if !serv.is_started() {
        // upstream's `assert(is_started())`.
        std::process::abort();
    }
    let mut ctx = NativeContext::new(serv.raw());
    ctx.object = mb;
    // SAFETY: as the caller guarantees.
    unsafe { run(crate::ops::RUN_ST_INIT_PIPE_SOCKETS, 0, &mut ctx) };
}

/// `Server::connection_incoming(reactor, conn)`, over the port `conn` arrived on.
///
/// # Safety
/// `serv` is a live `Server`, `reactor` its reactor and `conn` one of its connections.
pub unsafe extern "C-unwind" fn connection_incoming(
    serv: *mut c_void,
    reactor: *mut c_void,
    conn: *mut ConnectionRecord,
) -> c_int {
    // SAFETY: per the caller, `serv` is a live `Server` for the call.
    let serv = unsafe { view::Server::from_raw(serv) };
    // SAFETY: as the caller guarantees.
    let record = unsafe { &*conn };
    let mut ctx = NativeContext::new(serv.raw());
    ctx.reactor = reactor;
    ctx.port = serv.port_by_server_fd(record.server_fd);
    ctx.sock = record.socket;
    ctx.conn = conn.cast();
    // SAFETY: as the caller guarantees.
    c_int::narrow(unsafe { run(RUN_INCOMING, 0, &mut ctx) })
}

/// The accept-retry timer's callback: re-enables accepting on the server.
///
/// # Safety
/// `serv` is a live `Server`, on the thread that armed the timer.
#[cfg(all(feature = "upstream-symbols", not(test)))]
pub(crate) unsafe extern "C" fn enable_accept(serv: *mut c_void) {
    // SAFETY: as the caller guarantees.
    unsafe {
        run(
            crate::ops::RUN_ENABLE_ACCEPT,
            0,
            &mut NativeContext::new(serv),
        )
    };
}

/// `Server::accept_connection(reactor, event)`: the master reactor's read handler
/// for listening sockets, over the server the reactor carries.
///
/// # Safety
/// `reactor` is the server's live reactor and `event` a read event on a listening socket.
#[cfg_attr(
    feature = "upstream-symbols",
    unsafe(export_name = "_ZN6swoole6Server17accept_connectionEPNS_7ReactorEPNS_5EventE")
)]
pub unsafe extern "C" fn accept_connection(reactor: *mut c_void, event: *mut c_void) -> c_int {
    // SAFETY: as the caller guarantees.
    let serv = unsafe { crate::reactor_handle::Reactor::of(reactor) }
        .fields()
        .ptr;
    let mut ctx = NativeContext::new(serv);
    // SAFETY: as the caller guarantees.
    let event_record = unsafe { &*event.cast::<swoole_rs_reactor::Event>() };
    ctx.reactor = reactor;
    ctx.event = event;
    // SAFETY: a listening socket's object is its port.
    ctx.port = unsafe {
        (*event_record
            .socket
            .cast::<swoole_rs_reactor::SocketRecord>())
        .object
    };
    ctx.server_fd = event_record.fd;
    // SAFETY: as the caller guarantees.
    c_int::narrow(unsafe { run(RUN_ACCEPT, 0, &mut ctx) })
}

/// `Server::is_healthy_connection(now, conn)`.
///
/// # Safety
/// `serv` is a live `Server` and `conn` one of its connections.
#[cfg_attr(
    feature = "upstream-symbols",
    unsafe(export_name = "_ZNK6swoole6Server21is_healthy_connectionEdPKNS_10ConnectionE")
)]
pub unsafe extern "C" fn is_healthy_connection(
    serv: *mut c_void,
    now: f64,
    conn: *const ConnectionRecord,
) -> bool {
    // SAFETY: per the caller, `serv` is a live `Server` for the call.
    let serv = unsafe { view::Server::from_raw(serv) };
    // SAFETY: as the caller guarantees.
    let conn = unsafe { &*conn };
    // SAFETY: as the caller guarantees.
    serv.healthy(now, conn)
}

unsafe extern "C" {
    /// native/error/error.cc's `swoole_get_last_error_msg()`.
    #[link_name = "_Z25swoole_get_last_error_msgv"]
    fn swoole_get_last_error_msg() -> *const c_char;
}

/// `Server::get_startup_error_message()`.
#[cfg_attr(
    feature = "upstream-symbols",
    unsafe(export_name = "_ZNK6swoole6Server25get_startup_error_messageEv")
)]
pub extern "C" fn get_startup_error_message(_serv: *mut c_void) -> *const c_char {
    // SAFETY: the calling thread's error text, always terminated.
    let message = unsafe { swoole_get_last_error_msg() };
    // SAFETY: as above.
    unsafe { crate::swr_v1_server_startup_error(message, log::swr_v1_core_last_error()) }
}

/// `Server::set_max_connection(max)`.
///
/// # Safety
/// `serv` is a live `Server`.
#[cfg_attr(
    feature = "upstream-symbols",
    unsafe(export_name = "_ZN6swoole6Server18set_max_connectionEj")
)]
pub unsafe extern "C" fn set_max_connection(serv: *mut c_void, max: u32) {
    // SAFETY: as the caller guarantees.
    unsafe {
        run(
            RUN_SET_MAX_CONNECTION,
            max.into(),
            &mut NativeContext::new(serv),
        )
    };
}

/// `Server::feedback(conn, event)`.
///
/// # Safety
/// `serv` is a live `Server` and `conn` one of its connections.
#[cfg_attr(
    feature = "upstream-symbols",
    unsafe(export_name = "_ZN6swoole6Server8feedbackEPNS_10ConnectionENS_15ServerEventTypeE")
)]
pub unsafe extern "C" fn feedback(serv: *mut c_void, conn: *mut c_void, event: c_int) -> bool {
    let mut ctx = NativeContext::new(serv);
    ctx.conn = conn;
    // SAFETY: as the caller guarantees.
    unsafe { run(RUN_FEEDBACK, i64::from(event).cast_unsigned(), &mut ctx) != 0 }
}

/// `Server::store_pipe_fd(pipe)`.
///
/// # Safety
/// `serv` is a live `Server` and `pipe` a live `UnixSocket`.
pub unsafe extern "C" fn store_pipe_fd(serv: *mut c_void, pipe: *mut c_void) {
    let mut ctx = NativeContext::new(serv);
    ctx.object = pipe;
    // SAFETY: as the caller guarantees.
    unsafe { run(RUN_ST_STORE_PIPE_FD, 0, &mut ctx) };
}

/// `Server::schedule_worker(fd, data)`.
///
/// # Safety
/// `serv` is a live `Server`; `data` null or a live `SendData`.
pub unsafe extern "C" fn schedule_worker(serv: *mut c_void, fd: c_int, data: *mut c_void) -> c_int {
    let mut ctx = NativeContext::new(serv);
    ctx.server_fd = fd;
    ctx.send = data;
    // SAFETY: as the caller guarantees.
    c_int::narrow(unsafe { run(RUN_SCHEDULE_WORKER, i64::from(fd).cast_unsigned(), &mut ctx) })
}

/// `Server::add_port(type, host, port)`: the new port, or null.
///
/// # Safety
/// `serv` is a live `Server`; `host` a NUL-terminated string.
#[cfg_attr(
    feature = "upstream-symbols",
    unsafe(export_name = "_ZN6swoole6Server8add_portE12swSocketTypePKci")
)]
pub unsafe extern "C" fn add_port(
    serv: *mut c_void,
    socket_type: c_int,
    host: *const c_char,
    port: c_int,
) -> *mut c_void {
    let mut ctx = NativeContext::new(serv);
    ctx.socket_type = socket_type;
    ctx.host = host;
    ctx.number = port;
    // SAFETY: as the caller guarantees.
    if unsafe { run(RUN_ADD_PORT, 0, &mut ctx) } != 0 {
        ctx.port
    } else {
        ptr::null_mut()
    }
}

/// `Server::abort_connection(reactor, port, socket)`.
///
/// # Safety
/// `serv` is a live `Server`; the others are the reactor, port and socket to abort.
#[cfg_attr(
    feature = "upstream-symbols",
    unsafe(
        export_name = "_ZNK6swoole6Server16abort_connectionEPNS_7ReactorEPKNS_10ListenPortEPNS_7network6SocketE"
    )
)]
pub unsafe extern "C" fn abort_connection(
    serv: *mut c_void,
    reactor: *mut c_void,
    port: *const c_void,
    sock: *mut c_void,
) {
    let mut ctx = NativeContext::new(serv);
    ctx.reactor = reactor;
    ctx.port = port.cast_mut();
    ctx.sock = sock;
    // SAFETY: as the caller guarantees.
    unsafe { run(RUN_ST_ABORT, 0, &mut ctx) };
}

/// `Server::add_connection(port, socket, server_fd)`: the new connection, or null.
///
/// # Safety
/// `serv` is a live `Server`; `port` and `sock` the accepting port and accepted socket.
#[cfg_attr(
    feature = "upstream-symbols",
    unsafe(
        export_name = "_ZN6swoole6Server14add_connectionEPKNS_10ListenPortEPNS_7network6SocketEi"
    )
)]
pub unsafe extern "C" fn add_connection(
    serv: *mut c_void,
    port: *const c_void,
    sock: *mut c_void,
    server_fd: c_int,
) -> *mut c_void {
    let mut ctx = NativeContext::new(serv);
    ctx.port = port.cast_mut();
    ctx.sock = sock;
    ctx.server_fd = server_fd;
    // SAFETY: as the caller guarantees.
    if unsafe { run(RUN_ADD_CONNECTION, 0, &mut ctx) } != 0 {
        ctx.conn
    } else {
        ptr::null_mut()
    }
}

/// `Server::send(session_id, data, length)`: the operation reads the `SendData` it lends.
///
/// # Safety
/// `serv` is a live `Server`; `data` holds `length` bytes.
#[cfg_attr(
    feature = "upstream-symbols",
    unsafe(export_name = "_ZNK6swoole6Server4sendElPKvj")
)]
pub unsafe extern "C" fn send(
    serv: *mut c_void,
    session_id: c_long,
    data: *const c_void,
    length: u32,
) -> bool {
    let mut send = SendData {
        info: DataHead {
            fd: session_id,
            kind: SEND_DATA,
            len: length,
            ..DataHead::default()
        },
        data: data.cast(),
    };
    let mut ctx = NativeContext::new(serv);
    ctx.send = (&raw mut send).cast();
    ctx.session_id = session_id;
    ctx.data = data;
    ctx.length = length;
    let arg = ctx.send.expose_provenance() as u64;
    // SAFETY: as the caller guarantees; the SendData lives for the call.
    unsafe { run(RUN_ST_SEND, arg, &mut ctx) != 0 }
}

/// `Server::set_document_root(path)`, before the server starts.
#[expect(
    clippy::must_use_candidate,
    reason = "`swoole_server_set` ignores the result, as upstream's does"
)]
pub fn set_document_root(serv: view::Server<'_>, path: &[u8]) -> bool {
    let mut ctx = NativeContext::new(serv.raw());
    ctx.data = path.as_ptr().cast();
    // SAFETY: the live server; the path lives for the call.
    unsafe { run(RUN_ST_DOCUMENT_ROOT, path.len() as u64, &mut ctx) != 0 }
}

/// [`set_document_root`] for the C++ test fixtures and tests/modules/http's cases.
///
/// # Safety
/// As [`set_document_root`]; `path` is readable for `length` bytes.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn swr_v1_server_set_document_root(
    serv: *mut c_void,
    path: *const u8,
    length: usize,
) -> bool {
    // SAFETY: per the caller.
    let path = unsafe { core::slice::from_raw_parts(path, length) };
    // SAFETY: per the caller.
    set_document_root(unsafe { view::Server::from_raw(serv) }, path)
}

/// The master timer's tick, `Server::timer_callback`, as a plain timer callback.
///
/// # Safety
/// `serv` is a live `Server`, in its master.
#[cfg(all(feature = "upstream-symbols", not(test)))]
pub(crate) unsafe extern "C" fn master_timer_tick(serv: *mut c_void) {
    // SAFETY: as the caller guarantees.
    unsafe {
        run(
            crate::ops::RUN_MASTER_TIMER,
            0,
            &mut NativeContext::new(serv),
        )
    };
}

/// `Server::get_timeout_callback`'s timer, a plain timer callback over the
/// connection: the idle check runs over its port and this thread's reactor, the
/// one that added the timer.
///
/// # Safety
/// `conn` is a live connection of `SwooleG.server`, on this thread's reactor.
#[cfg(all(feature = "upstream-symbols", not(test)))]
pub(crate) unsafe extern "C" fn idle_timeout_tick(conn: *mut c_void) {
    let serv = swoole_rs_corebase::swr_v1_core_server();
    let mut ctx = NativeContext::new(serv);
    // SAFETY: as the caller guarantees.
    let record = unsafe { &*conn.cast::<ConnectionRecord>() };
    // SAFETY: the live server and the connection's listening fd.
    ctx.port = unsafe { view::Server::from_raw(serv) }.port_by_server_fd(record.server_fd);
    ctx.reactor = swoole_rs_corebase::thread_reactor();
    ctx.sock = record.socket;
    ctx.conn = conn;
    // SAFETY: as the caller guarantees.
    unsafe { run(crate::ops::RUN_IDLE_TIMEOUT, 0, &mut ctx) };
}

/// `Server::sendwait(session_id, data, length)`.
#[must_use]
pub fn sendwait(serv: view::Server<'_>, session_id: c_long, data: &[u8]) -> bool {
    let mut ctx = NativeContext::new(serv.raw());
    ctx.session_id = session_id;
    ctx.data = data.as_ptr().cast();
    #[expect(
        clippy::cast_possible_truncation,
        reason = "Server::sendwait's uint32_t length, as upstream"
    )]
    let length = data.len() as u32;
    ctx.length = length;
    // SAFETY: the live server; `data` holds `length` bytes for the call.
    unsafe { run(RUN_SENDWAIT, session_id.cast_unsigned(), &mut ctx) != 0 }
}

/// `Server::has_kernel_nobufs_error(session_id)`.
#[must_use]
pub fn has_kernel_nobufs_error(serv: view::Server<'_>, session_id: c_long) -> bool {
    run_on(serv, RUN_KERNEL_NOBUFS, session_id.cast_unsigned()) != 0
}

/// `Server::send_to_connection(send)`: `SW_OK` or `SW_ERR`.
///
/// # Safety
/// `serv` is a live `Server`; `send` a live `SendData`.
#[cfg_attr(
    feature = "upstream-symbols",
    unsafe(export_name = "_ZNK6swoole6Server18send_to_connectionEPKNS_8SendDataE")
)]
pub unsafe extern "C" fn send_to_connection(serv: *mut c_void, send: *const c_void) -> c_int {
    // SAFETY: as the caller guarantees.
    let header = unsafe { &*send.cast::<SendData>() };
    let mut ctx = NativeContext::new(serv);
    ctx.send = send.cast_mut();
    ctx.session_id = header.info.fd;
    ctx.data = header.data.cast();
    ctx.length = header.info.len;
    // SAFETY: as the caller guarantees.
    c_int::narrow(unsafe { run(RUN_SC_SEND, header.info.kind.into(), &mut ctx) })
}

const IPC_BUFFER_SIZE: usize = swoole_rs_corebase::constants::SW_IPC_BUFFER_SIZE;

/// `Server::sendfile`'s buffer, aligned for the `SendfileTask` it starts with.
#[repr(C, align(8))]
struct SendfileBuffer([u8; IPC_BUFFER_SIZE]);

/// `Server::sendfile(session_id, file, l_file, offset, length)`.
///
/// # Safety
/// `serv` is a live `Server`; `file` holds `l_file` bytes.
#[cfg_attr(
    all(feature = "upstream-symbols", target_os = "macos"),
    unsafe(export_name = "_ZNK6swoole6Server8sendfileElPKcjxm")
)]
#[cfg_attr(
    all(feature = "upstream-symbols", not(target_os = "macos")),
    unsafe(export_name = "_ZNK6swoole6Server8sendfileElPKcjlm")
)]
pub unsafe extern "C" fn sendfile(
    serv: *mut c_void,
    session_id: c_long,
    file: *const c_char,
    l_file: u32,
    offset: libc::off_t,
    length: usize,
) -> bool {
    let mut buffer = SendfileBuffer([0; IPC_BUFFER_SIZE]);
    // `SendfileTask::length` follows its `off_t offset` (server.cc asserts it).
    buffer.0[8..16].copy_from_slice(&length.to_ne_bytes());
    let mut ctx = NativeContext::new(serv);
    ctx.session_id = session_id;
    ctx.data = file.cast();
    ctx.length = l_file;
    ctx.buffer = buffer.0.as_mut_ptr().cast();
    ctx.offset = offset;
    // SAFETY: as the caller guarantees; the buffer lives for the call.
    unsafe { run(RUN_SENDFILE, session_id.cast_unsigned(), &mut ctx) != 0 }
}

/// `Server::create_base_factory()`: the new `BaseFactory`, or null.
#[must_use]
pub fn create_base_factory(serv: view::Server<'_>) -> *mut c_void {
    let mut ctx = NativeContext::new(serv.raw());
    // SAFETY: a live server, and no other object for the operation to read.
    if unsafe { run(RUN_BASE_CREATE, 0, &mut ctx) } != 0 {
        ctx.object
    } else {
        ptr::null_mut()
    }
}

/// Runs `BaseFactory` member `op` (a `RUN_BASE_*`) with `arg` over its server, with
/// `send` lent.
///
/// # Safety
/// `factory` is a live `Factory`; `send` null or what `op` reads.
#[cfg(any(test, feature = "upstream-symbols"))]
pub(crate) unsafe fn on_base(factory: *mut c_void, send: *mut c_void, op: u32, arg: u64) -> bool {
    // `Factory::server_`, its first member (server.cc asserts the layout).
    // SAFETY: as the caller guarantees.
    let serv = unsafe { *factory.cast::<*mut c_void>() };
    let mut ctx = NativeContext::new(serv);
    ctx.send = send;
    // SAFETY: as the caller guarantees.
    unsafe { run(op, arg, &mut ctx) != 0 }
}

/// `BaseFactory::end(session_id, flags)`'s argument: the C++ forward's
/// `(uint64_t) session_id | ((uint64_t) flags << 48)`.
#[cfg(any(test, feature = "upstream-symbols"))]
pub(crate) fn end_arg(session_id: c_long, flags: c_int) -> u64 {
    session_id.cast_unsigned() | (i64::from(flags).cast_unsigned() << 48)
}

/// `serv->reactor_thread_barrier.wait()`.
#[cfg(all(feature = "upstream-symbols", not(test)))]
pub(crate) fn reactor_barrier(serv: view::Server<'_>) {
    run_on(serv, RUN_NATIVE_OP, crate::ops::OP_REACTOR_BARRIER.into());
}

/// Runs the native op `op` over `ctx`, which holds its operands.
///
/// # Safety
/// `ctx.serv` is a live `Server`, and the other fields are what `op` reads.
unsafe fn native_op(op: u32, ctx: &mut NativeContext) -> bool {
    // SAFETY: as the caller guarantees.
    unsafe { run(RUN_NATIVE_OP, op.into(), ctx) != 0 }
}

/// `Server::get_package_max_length(conn)`.
///
/// # Safety
/// `serv` is a live `Server` and `conn` one of its connections.
pub unsafe fn package_max_length(serv: *mut c_void, conn: *mut c_void) -> u32 {
    // SAFETY: per the caller, `serv` is a live `Server` for the call.
    let serv = unsafe { view::Server::from_raw(serv) };
    // SAFETY: as the caller guarantees.
    let fd = unsafe { &*conn.cast::<ConnectionRecord>() }.fd;
    let port = serv.port_by_fd(fd);
    // SAFETY: the connection's live port, as in C++, which doesn't check it either.
    unsafe { view::port(port) }.protocol.package_max_length
}

/// `Server::CLOSE_RESET`, `Server::CLOSE_ACTIVELY`.
const CLOSE_RESET: c_int = 1 << 1;
const CLOSE_ACTIVELY: c_int = 1 << 2;

/// `Server::close(session_id, reset)`: the factory ends the session, actively.
///
/// # Safety
/// `serv` is a live `Server`.
#[cfg_attr(
    feature = "upstream-symbols",
    unsafe(export_name = "_ZNK6swoole6Server5closeElb")
)]
pub unsafe extern "C" fn close(serv: *mut c_void, session_id: c_long, reset: bool) -> bool {
    let mut ctx = NativeContext::new(serv);
    ctx.session_id = session_id;
    ctx.number = CLOSE_ACTIVELY | if reset { CLOSE_RESET } else { 0 };
    // SAFETY: as the caller guarantees.
    unsafe { native_op(OP_FACTORY_END, &mut ctx) }
}

/// `Server::notify(conn, event)`: the factory hears `event` for `conn`.
pub fn notify(serv: view::Server<'_>, conn: &ConnectionRecord, event: c_int) -> bool {
    #[expect(
        clippy::cast_possible_truncation,
        clippy::cast_sign_loss,
        reason = "DataHead's uint8_t type and uint16_t server_fd keep the low bits, as the C++ assignments did"
    )]
    let mut head = DataHead {
        fd: conn.fd.into(),
        reactor_id: conn.reactor_id.into(),
        kind: event as u8,
        server_fd: conn.server_fd as u16,
        ..DataHead::default()
    };
    let mut ctx = NativeContext::new(serv.raw());
    ctx.object = (&raw mut head).cast();
    // SAFETY: the live server; the head lives for the call.
    unsafe { native_op(OP_FACTORY_NOTIFY, &mut ctx) }
}

/// `SW_SERVER_EVENT_PIPE_MESSAGE` (server.cc asserts it).
const EVENT_PIPE_MESSAGE: u8 = 10;

/// `Server::send_pipe_message(worker_id, msg)`: `msg` to that worker, through the master.
///
/// # Safety
/// `serv` is a live `Server`, `worker_id` one of its workers and `msg` a live `EventData`.
pub unsafe extern "C" fn send_pipe_message(
    serv: *mut c_void,
    worker_id: u32,
    msg: *mut DataHead,
) -> bool {
    // SAFETY: as the caller guarantees: an `EventData` starts with its `DataHead`.
    let head = unsafe { &mut *msg };
    head.kind = EVENT_PIPE_MESSAGE;
    let mut ctx = NativeContext::new(serv);
    ctx.number = worker_id.cast_signed();
    ctx.object = msg.cast();
    // `EventData::size()`: the head and its payload.
    ctx.length = 40 + head.len;
    // SAFETY: as the caller guarantees.
    unsafe { native_op(OP_PIPE_TO_WORKER, &mut ctx) }
}

/// `Server::add_command(name, accepted_process_types, handler)`: whether the
/// command joined. The handler's context lives as long as the server.
///
/// # Safety
/// `serv` is a live `Server`; `name` holds `length` bytes; `handler.invoke` is a
/// `commands::Handler`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn swr_v1_server_add_command(
    serv: *mut c_void,
    name: *const c_char,
    length: usize,
    accepted_types: c_int,
    handler: crate::CommandFn,
) -> bool {
    let add = crate::commands::AddCommand {
        // SAFETY: per the caller.
        name: unsafe { core::slice::from_raw_parts(name.cast(), length) },
        accepted_types,
        handler,
    };
    let arg = core::ptr::from_ref(&add).expose_provenance() as u64;
    // SAFETY: as the caller guarantees; `add` lives for the call.
    unsafe { run(RUN_ST_ADD_COMMAND, arg, &mut NativeContext::new(serv)) != 0 }
}

/// `Server::command(process_id, process_type, name, message, callback)`: whether
/// the request went. Its reply reaches `callback` once, perhaps before this returns.
///
/// # Safety
/// `serv` is a live `Server`; `name` and `message` hold their lengths in bytes;
/// `callback.invoke` is a `commands::Callback` whose context lives until it is called.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn swr_v1_server_command(
    serv: *mut c_void,
    process_id: u32,
    process_type: c_int,
    name: *const c_char,
    name_length: usize,
    message: *const c_char,
    length: usize,
    callback: crate::CommandFn,
) -> bool {
    let send = crate::commands::SendCommand {
        serv,
        process_id,
        process_type,
        // SAFETY: per the caller.
        name: unsafe { core::slice::from_raw_parts(name.cast(), name_length) },
        // SAFETY: per the caller.
        message: unsafe { core::slice::from_raw_parts(message.cast(), length) },
        callback,
    };
    let arg = core::ptr::from_ref(&send).expose_provenance() as u64;
    // SAFETY: as the caller guarantees; `send` lives for the call.
    unsafe { run(RUN_ST_COMMAND, arg, &mut NativeContext::new(serv)) != 0 }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ops::{RUN_BASE_DISPATCH, RUN_BASE_END, RUN_BASE_NOTIFY};
    use std::sync::Mutex;

    /// The last call the fake runner saw: op, arg and the context's fields.
    type Seen = (u32, u64, usize, usize, usize, usize, c_int, usize);
    static SEEN: Mutex<Vec<Seen>> = Mutex::new(Vec::new());
    /// The send fields of each call: op, arg, serv, send, data, length, session,
    /// offset and the `SendfileTask::length` in the buffer, if any.
    type Sent = (u32, u64, usize, usize, usize, u32, c_long, i64, usize);
    static SENT: Mutex<Vec<Sent>> = Mutex::new(Vec::new());
    /// The worker or flags and the `DataHead` of each native op that takes one.
    static HEADS: Mutex<Vec<(c_int, DataHead)>> = Mutex::new(Vec::new());

    pub(super) unsafe fn fake(op: u32, arg: u64, ctx: *mut NativeContext) -> i64 {
        // SAFETY: the methods pass their own live context.
        let ctx = unsafe { &mut *ctx };
        SEEN.lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .push((
                op,
                arg,
                ctx.serv.addr(),
                ctx.port.addr(),
                ctx.sock.addr(),
                ctx.object.addr(),
                ctx.number,
                ctx.host.addr(),
            ));
        let task_length = if ctx.buffer.is_null() {
            0
        } else {
            // SAFETY: sendfile's buffer starts with a `SendfileTask`.
            unsafe { ctx.buffer.add(8).cast::<usize>().read_unaligned() }
        };
        SENT.lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .push((
                op,
                arg,
                ctx.serv.addr(),
                ctx.send.addr(),
                ctx.data.addr(),
                ctx.length,
                ctx.session_id,
                ctx.offset,
                task_length,
            ));
        if op == RUN_NATIVE_OP {
            let head = ctx.object.cast::<DataHead>();
            let number = ctx.number;
            let push = |head: DataHead| {
                HEADS
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner)
                    .push((number, head));
            };
            match u32::try_from(arg).unwrap_or(0) {
                // SAFETY: these lend a head.
                OP_FACTORY_NOTIFY | OP_PIPE_TO_WORKER => push(unsafe { *head }),
                _ => push(DataHead::default()),
            }
            return 1;
        }
        ctx.port = ptr::without_provenance_mut(0x50);
        ctx.conn = ptr::without_provenance_mut(0x60);
        ctx.object = ptr::without_provenance_mut(0x70);
        match op {
            RUN_SHUTDOWN | RUN_ADD_PORT | RUN_BASE_CREATE | RUN_ST_SEND => 1,
            RUN_SC_SEND => -1,
            RUN_IDLE_WORKERS => -1,
            _ => 0,
        }
    }

    /// A server whose state and config are null: the fake runner reads neither.
    static FAKE: [usize; 2] = [0; 2];

    fn fake_serv() -> *mut c_void {
        (&raw const FAKE).cast_mut().cast()
    }

    fn server() -> view::Server<'static> {
        unsafe { view::Server::from_raw(fake_serv()) }
    }

    fn last() -> Seen {
        *SEEN
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .last()
            .unwrap_or(&(0, 0, 0, 0, 0, 0, 0, 0))
    }

    fn sent() -> Sent {
        *SENT
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .last()
            .unwrap_or(&(0, 0, 0, 0, 0, 0, 0, 0, 0))
    }

    #[test]
    fn methods_fill_the_context_their_forward_did() {
        let serv = fake_serv();
        let sv = serv.addr();
        let at = |a: usize| ptr::without_provenance_mut::<c_void>(a);
        unsafe {
            assert!(shutdown(serv), "shutdown's 1 is true");
            assert_eq!(last().0, RUN_SHUTDOWN, "shutdown op");
            assert_eq!(get_idle_worker_num(serv), u32::MAX, "(uint32_t) -1");
            set_max_connection(serv, 7);
            assert_eq!(last().1, 7, "max connection arg");
            assert_eq!(
                schedule_worker(serv, -2, ptr::null_mut()),
                0,
                "schedule result"
            );
            assert_eq!(
                last().1,
                u64::MAX - 1,
                "fd sign-extends like (uint64_t)(int64_t) fd"
            );
            assert_eq!(
                add_port(serv, 1, c"h".as_ptr(), 9).addr(),
                0x50,
                "nonzero yields the port"
            );
            let (op, _, s, _, _, _, number, host) = last();
            assert!(
                op == RUN_ADD_PORT && s == sv && number == 9 && host != 0,
                "add_port context"
            );
            assert!(
                add_connection(serv, at(0x20), at(0x30), 3).is_null(),
                "zero yields null"
            );
            assert_eq!(
                last(),
                (RUN_ADD_CONNECTION, 0, sv, 0x20, 0x30, 0, 0, 0),
                "add_connection context"
            );
            assert!(!feedback(serv, at(0x60), 5), "feedback result");
            assert_eq!(last().1, 5, "event arg");
            send_family_and_base_factory(serv);
        }
    }

    unsafe fn send_family_and_base_factory(serv: *mut c_void) {
        let sv = serv.addr();
        let at = |a: usize| ptr::without_provenance_mut::<c_void>(a);
        unsafe {
            let data = at(0x80);
            assert!(send(serv, 9, data, 4), "send's 1 is true");
            assert_eq!(
                sent(),
                (RUN_ST_SEND, sent().3 as u64, sv, sent().3, 0x80, 4, 9, 0, 0),
                "send context, with its SendData lent"
            );
            assert_ne!(sent().3, 0, "a SendData is lent");
            assert!(!sendwait(server(), 9, &[0; 4]), "sendwait result");
            assert_eq!(sent().1, 9, "sendwait passes the session");
            assert!(!has_kernel_nobufs_error(server(), 11), "nobufs result");
            assert_eq!(sent().1, 11, "nobufs passes the session");
            assert!(
                !sendfile(serv, 12, c"f".as_ptr(), 1, 3, 99),
                "sendfile result"
            );
            let (op, arg, _, _, _, length, session, offset, task_length) = sent();
            assert!(
                op == RUN_SENDFILE && arg == 12 && length == 1 && session == 12,
                "sendfile context"
            );
            assert_eq!((offset, task_length), (3, 99), "offset and task length");

            let mut task = SendData {
                info: DataHead {
                    fd: 13,
                    len: 5,
                    kind: 2,
                    ..DataHead::default()
                },
                data: data.cast_const().cast(),
            };
            let task_at = (&raw mut task).cast::<c_void>();
            assert_eq!(send_to_connection(serv, task_at), -1, "SW_ERR");
            assert_eq!(
                sent(),
                (RUN_SC_SEND, 2, sv, task_at.addr(), 0x80, 5, 13, 0, 0),
                "send_to_connection reads the SendData"
            );

            assert_eq!(
                create_base_factory(server()).addr(),
                0x70,
                "the created factory"
            );
            assert_eq!(sent().0, RUN_BASE_CREATE, "create op");
            // A Factory: server_, then manager_.
            let mut factory = [serv, at(0x90)];
            let factory_at = factory.as_mut_ptr().cast::<c_void>();
            let task_arg = task_at.addr() as u64;
            assert!(
                !on_base(factory_at, task_at, RUN_BASE_DISPATCH, task_arg),
                "dispatch result"
            );
            let (op, arg, s, lent, ..) = sent();
            assert!(
                op == RUN_BASE_DISPATCH
                    && arg == task_at.addr() as u64
                    && s == sv
                    && lent == task_at.addr(),
                "dispatch lends the task"
            );
            on_base(factory_at, ptr::null_mut(), RUN_BASE_NOTIFY, 0xa0);
            let (op, arg, s, lent, ..) = sent();
            assert!(
                op == RUN_BASE_NOTIFY && arg == 0xa0 && s == sv && lent == 0,
                "notify passes the head"
            );
            on_base(factory_at, ptr::null_mut(), RUN_BASE_END, end_arg(7, 6));
            assert_eq!(sent().1, 7 | (6 << 48), "end packs flags above the session");
            close_notify_and_pipe_messages(serv);
        }
    }

    fn head() -> (c_int, DataHead) {
        *HEADS
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .last()
            .unwrap_or(&(0, DataHead::default()))
    }

    unsafe fn close_notify_and_pipe_messages(serv: *mut c_void) {
        unsafe {
            assert!(close(serv, 21, true), "close result");
            let (op, arg, _, _, _, _, session, ..) = sent();
            assert!(
                op == RUN_NATIVE_OP && arg == u64::from(OP_FACTORY_END) && session == 21,
                "close ends the session"
            );
            assert_eq!(head().0, CLOSE_ACTIVELY | CLOSE_RESET, "reset flags");
            close(serv, 21, false);
            assert_eq!(head().0, CLOSE_ACTIVELY, "active flags");

            // SAFETY: a zeroed record is a valid connection.
            let mut conn: ConnectionRecord = core::mem::zeroed();
            conn.fd = 22;
            conn.reactor_id = 3;
            conn.server_fd = 0x1_0005;
            assert!(notify(server(), &conn, 0x104), "notify result");
            assert_eq!(sent().1, u64::from(OP_FACTORY_NOTIFY), "notify op");
            let (_, h) = head();
            assert!(
                h.fd == 22 && h.reactor_id == 3 && h.kind == 4 && h.server_fd == 5,
                "notify head keeps the low bits"
            );

            let mut message = DataHead {
                len: 3,
                ..DataHead::default()
            };
            assert!(send_pipe_message(serv, 2, &raw mut message), "pipe result");
            let (op, arg, _, _, data, length, ..) = sent();
            assert!(
                op == RUN_NATIVE_OP
                    && arg == u64::from(OP_PIPE_TO_WORKER)
                    && length == 43
                    && data == 0,
                "the packed message goes to the worker with its size"
            );
            let (worker, h) = head();
            assert!(
                worker == 2 && h.kind == EVENT_PIPE_MESSAGE,
                "worker and pipe type"
            );
        }
    }
}
