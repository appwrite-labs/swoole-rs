//! The public `Server` methods of `src/server/master.cc`, and the `BaseFactory`
//! methods that were `src/server/base.cc`, that are one operation over a fresh
//! context, defined under their upstream (Itanium) symbols when the hybrid links
//! this crate (`upstream-symbols`). Native code registers the guarded runner
//! (`server_run`) once; each method fills the context the way its C++ forward did
//! and runs over it.

use core::ffi::{c_char, c_int, c_long, c_void};
use core::ptr;
use std::sync::OnceLock;

use crate::base_factory::{SEND_DATA, SendData};
use crate::ops::{OP_FACTORY_END, OP_FACTORY_NOTIFY, OP_PIPE_TO_WORKER, OP_TASK_PACK};
use crate::ops::{
    RUN_ACCEPT, RUN_CREATE, RUN_DESTROY, RUN_INCOMING, RUN_ST_INIT_PIPE_SOCKETS, RUN_START,
    RUN_START_CHECK, RUN_START_MASTER_THREAD, RUN_STORE_LISTEN_SOCKET,
};
use crate::ops::{
    RUN_ADD_CONNECTION, RUN_ADD_PORT, RUN_BASE_CREATE, RUN_BASE_DISPATCH, RUN_BASE_END,
    RUN_BASE_FINISH, RUN_BASE_NOTIFY, RUN_FEEDBACK, RUN_IDLE_TASK_WORKERS, RUN_IDLE_WORKERS,
    RUN_KERNEL_NOBUFS, RUN_NATIVE_OP, RUN_REOPEN_LOGGER, RUN_SC_SEND, RUN_SCHEDULE_WORKER,
    RUN_SENDFILE, RUN_SENDWAIT, RUN_SET_MAX_CONNECTION, RUN_SHUTDOWN, RUN_ST_ABORT,
    RUN_ST_ADD_COMMAND, RUN_ST_CLEAR_TIMER, RUN_ST_COMMAND, RUN_ST_DOCUMENT_ROOT, RUN_ST_FOREACH,
    RUN_ST_SEND, RUN_ST_STORE_PIPE_FD, RUN_SYSTEMD_SOCKETS, RUN_TASKING,
};
use crate::records::ConnectionRecord;
use crate::view;
use swoole_rs_corebase::{DataHead, hook, log};

/// master.cc's `ServerRustContext`, which asserts this layout.
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

const _: () = assert!(
    size_of::<NativeContext>() == 136 && core::mem::offset_of!(NativeContext, object) == 128,
    "master.cc ServerRustContext layout"
);

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

/// master.cc's `server_run`: one operation over a context, native exception custody included.
pub type NativeRun = unsafe extern "C-unwind" fn(u32, u64, *mut NativeContext) -> i64;

static NATIVE: OnceLock<NativeRun> = OnceLock::new();

/// Registers master.cc's runner, which the methods below call.
#[unsafe(no_mangle)]
pub extern "C" fn swr_v1_server_set_native(run: NativeRun) {
    // A second registration is the same function; keeping the first is equivalent.
    NATIVE.get_or_init(|| run);
}

/// Runs `op` over `ctx` through the registered runner.
///
/// # Safety
/// `ctx.serv` is a live `Server`, and the other fields are what `op` reads.
unsafe fn run(op: u32, arg: u64, ctx: &mut NativeContext) -> i64 {
    let Some(native) = NATIVE.get() else {
        // master.cc registers before any Server exists.
        std::process::abort()
    };
    // SAFETY: as the caller guarantees.
    unsafe { native(op, arg, ctx) }
}

/// Each method that is one operation over the server alone: its symbol, Rust
/// name, return type and operation. `bool` and `int` returns narrow like C++.
macro_rules! plain_methods {
    ($($symbol:literal fn $name:ident() -> $r:ty = $op:ident;)*) => {$(
        #[doc = concat!("`", $symbol, "`.")]
        ///
        /// # Safety
        /// `serv` is a live `Server`.
        #[cfg_attr(feature = "upstream-symbols", unsafe(export_name = $symbol))]
        pub unsafe extern "C-unwind" fn $name(serv: *mut c_void) -> $r {
            // SAFETY: as the caller guarantees.
            Narrow::narrow(unsafe { run($op, 0, &mut NativeContext::new(serv)) })
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
    "_ZN6swoole6Server11clear_timerEv" fn clear_timer() -> () = RUN_ST_CLEAR_TIMER;
    "_ZN6swoole6Server8shutdownEv" fn shutdown() -> bool = RUN_SHUTDOWN;
    "_ZNK6swoole6Server28signal_handler_reopen_loggerEv" fn signal_handler_reopen_logger() -> bool = RUN_REOPEN_LOGGER;
    "_ZN6swoole6Server18add_systemd_socketEv" fn add_systemd_socket() -> c_int = RUN_SYSTEMD_SOCKETS;
    "_ZNK6swoole6Server19get_idle_worker_numEv" fn get_idle_worker_num() -> u32 = RUN_IDLE_WORKERS;
    "_ZNK6swoole6Server24get_idle_task_worker_numEv" fn get_idle_task_worker_num() -> c_int = RUN_IDLE_TASK_WORKERS;
    "_ZNK6swoole6Server15get_tasking_numEv" fn get_tasking_num() -> c_int = RUN_TASKING;
    "_ZN6swoole6Server5startEv" fn start() -> c_int = RUN_START;
    "_ZN6swoole6Server11start_checkEv" fn start_check() -> c_int = RUN_START_CHECK;
    "_ZN6swoole6Server19store_listen_socketEv" fn store_listen_socket() -> () = RUN_STORE_LISTEN_SOCKET;
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
    // SAFETY: as the caller guarantees.
    if !unsafe { view::config(serv) }.factory_.is_null() {
        return -1;
    }
    if hook::swoole_isset_hook(HOOK_BEFORE_SERVER_CREATE) {
        // SAFETY: the hook's callbacks take the server.
        unsafe { hook::swoole_call_hook(HOOK_BEFORE_SERVER_CREATE, serv) };
    }
    // SAFETY: as the caller guarantees.
    c_int::narrow(unsafe { run(RUN_CREATE, 0, &mut NativeContext::new(serv)) })
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
    // SAFETY: as the caller guarantees.
    if !unsafe { view::config(serv) }.factory_.is_null() {
        // SAFETY: as the caller guarantees.
        unsafe { run(RUN_DESTROY, 0, &mut NativeContext::new(serv)) };
    }
}

/// `Server::Server(mode)` after native/integration/server.cc has built the C++
/// members: maps the `gs_size`-byte shared record, sets the mode and makes this
/// `SwooleG.server`. False when the record can't be mapped.
///
/// # Safety
/// `serv` is a `Server` under construction whose `ServerFields` are built.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn swr_v1_server_open(
    serv: *mut c_void,
    mode: c_int,
    gs_size: usize,
) -> bool {
    let gs = swoole_rs_memory::members::sw_shm_malloc(gs_size);
    if gs.is_null() {
        log::sys_warning(
            c"Server",
            format_args!("[Master] Fatal Error: failed to allocate memory for Server->gs"),
        );
        return false;
    }
    // SAFETY: a fresh mapping of a whole `ServerGS`, which starts with the counters.
    unsafe { crate::swr_v1_server_shared_init(gs) };
    // SAFETY: as the caller guarantees.
    let config = unsafe { view::config(serv) };
    config.gs = gs;
    config.mode_ = u8::try_from(mode).unwrap_or_default();
    swoole_rs_corebase::swr_v1_core_set_server(serv);
    true
}

/// `Server::~Server()` before native/integration/server.cc destroys the C++
/// members: destroys a server still running in its master, deletes each port
/// with `delete_port`, then frees the CPU list and the shared record.
///
/// # Safety
/// `serv` is a `Server` that [`swr_v1_server_open`] opened, being destroyed;
/// `delete_port` deletes a `ListenPort`.
#[unsafe(no_mangle)]
pub unsafe extern "C-unwind" fn swr_v1_server_close(
    serv: *mut c_void,
    delete_port: unsafe extern "C" fn(*mut c_void),
) {
    // SAFETY: as the caller guarantees.
    let counters = unsafe { view::counters(serv) };
    // SAFETY: getpid has no preconditions.
    let master = unsafe { libc::getpid() } == counters.master_pid;
    if master
        && counters
            .shutdown
            .load(core::sync::atomic::Ordering::Relaxed)
            == 0
    {
        // SAFETY: as the caller guarantees.
        unsafe { destroy(serv) };
    }
    // SAFETY: as the caller guarantees; deleting a port leaves the list as it is.
    let ports = (0..).map(|index| unsafe { view::port_at(serv, index) });
    for port in ports.take_while(|port| !port.is_null()) {
        // SAFETY: a live port, deleted once.
        unsafe { delete_port(port) };
    }
    // SAFETY: as the caller guarantees.
    let config = unsafe { view::config(serv) };
    // SAFETY: the list settings.rs allocated, or null.
    unsafe { swoole_rs_corebase::sw_free(config.cpu_affinity_available.cast()) };
    config.cpu_affinity_available = ptr::null_mut();
    swoole_rs_memory::members::sw_shm_free(config.gs);
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
/// `serv` is a live `Server` and `mb` a live `MessageBus`.
#[cfg_attr(
    feature = "upstream-symbols",
    unsafe(export_name = "_ZNK6swoole6Server17init_pipe_socketsEPNS_10MessageBusE")
)]
pub unsafe extern "C-unwind" fn init_pipe_sockets(serv: *mut c_void, mb: *mut c_void) {
    // SAFETY: as the caller guarantees.
    if !unsafe { view::is_started(serv) } {
        // upstream's `assert(is_started())`.
        std::process::abort();
    }
    let mut ctx = NativeContext::new(serv);
    ctx.object = mb;
    // SAFETY: as the caller guarantees.
    unsafe { run(RUN_ST_INIT_PIPE_SOCKETS, 0, &mut ctx) };
}

/// `Server::connection_incoming(reactor, conn)`, over the port `conn` arrived on.
///
/// # Safety
/// `serv` is a live `Server`, `reactor` its reactor and `conn` one of its connections.
#[cfg_attr(
    feature = "upstream-symbols",
    unsafe(export_name = "_ZNK6swoole6Server19connection_incomingEPNS_7ReactorEPNS_10ConnectionE")
)]
pub unsafe extern "C-unwind" fn connection_incoming(
    serv: *mut c_void,
    reactor: *mut c_void,
    conn: *mut ConnectionRecord,
) -> c_int {
    // SAFETY: as the caller guarantees.
    let record = unsafe { &*conn };
    let mut ctx = NativeContext::new(serv);
    ctx.reactor = reactor;
    // SAFETY: as the caller guarantees.
    ctx.port = unsafe { view::port_by_server_fd(serv, record.server_fd) };
    ctx.sock = record.socket;
    ctx.conn = conn.cast();
    // SAFETY: as the caller guarantees.
    c_int::narrow(unsafe { run(RUN_INCOMING, 0, &mut ctx) })
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
    let fields = unsafe { swoole_rs_reactor::reactor_upstream::prepared(reactor) }.fields;
    // SAFETY: as above.
    let mut ctx = NativeContext::new(unsafe { (*fields).ptr });
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
    // SAFETY: as the caller guarantees.
    let conn = unsafe { &*conn };
    // SAFETY: as the caller guarantees.
    unsafe { view::healthy(serv, now, conn) }
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
#[cfg_attr(
    feature = "upstream-symbols",
    unsafe(export_name = "_ZN6swoole6Server13store_pipe_fdEPNS_10UnixSocketE")
)]
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
#[cfg_attr(
    feature = "upstream-symbols",
    unsafe(export_name = "_ZN6swoole6Server15schedule_workerEiPNS_8SendDataE")
)]
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

/// `Server::foreach_connection(callback)`.
///
/// # Safety
/// `serv` is a live `Server`; `callback` a live `std::function<void(Connection *)>`.
#[cfg_attr(
    all(feature = "upstream-symbols", target_os = "macos"),
    unsafe(
        export_name = "_ZNK6swoole6Server18foreach_connectionERKNSt3__18functionIFvPNS_10ConnectionEEEE"
    )
)]
#[cfg_attr(
    all(feature = "upstream-symbols", not(target_os = "macos")),
    unsafe(
        export_name = "_ZNK6swoole6Server18foreach_connectionERKSt8functionIFvPNS_10ConnectionEEE"
    )
)]
pub unsafe extern "C" fn foreach_connection(serv: *mut c_void, callback: *const c_void) {
    let mut ctx = NativeContext::new(serv);
    ctx.object = callback.cast_mut();
    // SAFETY: as the caller guarantees.
    unsafe { run(RUN_ST_FOREACH, 0, &mut ctx) };
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

/// `Server::set_document_root(path)`.
///
/// # Safety
/// `serv` is a live `Server`, not started.
pub unsafe fn set_document_root(serv: *mut c_void, path: &[u8]) -> bool {
    let mut ctx = NativeContext::new(serv);
    ctx.data = path.as_ptr().cast();
    // SAFETY: as the caller guarantees; the path lives for the call.
    unsafe { run(RUN_ST_DOCUMENT_ROOT, path.len() as u64, &mut ctx) != 0 }
}

/// `Server::sendwait(session_id, data, length)`.
///
/// # Safety
/// `serv` is a live `Server`; `data` holds `length` bytes.
#[cfg_attr(
    feature = "upstream-symbols",
    unsafe(export_name = "_ZNK6swoole6Server8sendwaitElPKvj")
)]
pub unsafe extern "C" fn sendwait(
    serv: *mut c_void,
    session_id: c_long,
    data: *const c_void,
    length: u32,
) -> bool {
    let mut ctx = NativeContext::new(serv);
    ctx.session_id = session_id;
    ctx.data = data;
    ctx.length = length;
    // SAFETY: as the caller guarantees.
    unsafe { run(RUN_SENDWAIT, session_id.cast_unsigned(), &mut ctx) != 0 }
}

/// `Server::has_kernel_nobufs_error(session_id)`.
///
/// # Safety
/// `serv` is a live `Server`.
#[cfg_attr(
    feature = "upstream-symbols",
    unsafe(export_name = "_ZNK6swoole6Server23has_kernel_nobufs_errorEl")
)]
pub unsafe extern "C" fn has_kernel_nobufs_error(serv: *mut c_void, session_id: c_long) -> bool {
    let mut ctx = NativeContext::new(serv);
    // SAFETY: as the caller guarantees.
    unsafe { run(RUN_KERNEL_NOBUFS, session_id.cast_unsigned(), &mut ctx) != 0 }
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
    // `SendfileTask::length` follows its `off_t offset` (master.cc asserts it).
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
///
/// # Safety
/// `serv` is a live `Server`.
#[cfg_attr(
    feature = "upstream-symbols",
    unsafe(export_name = "_ZN6swoole6Server19create_base_factoryEv")
)]
pub unsafe extern "C" fn create_base_factory(serv: *mut c_void) -> *mut c_void {
    let mut ctx = NativeContext::new(serv);
    // SAFETY: as the caller guarantees.
    if unsafe { run(RUN_BASE_CREATE, 0, &mut ctx) } != 0 {
        ctx.object
    } else {
        ptr::null_mut()
    }
}

/// `BaseFactory::start()`: base mode has nothing to start.
#[cfg_attr(
    feature = "upstream-symbols",
    unsafe(export_name = "_ZN6swoole11BaseFactory5startEv")
)]
pub extern "C" fn base_factory_start(_factory: *mut c_void) -> bool {
    true
}

/// `BaseFactory::shutdown()`: nor to stop.
#[cfg_attr(
    feature = "upstream-symbols",
    unsafe(export_name = "_ZN6swoole11BaseFactory8shutdownEv")
)]
pub extern "C" fn base_factory_shutdown(_factory: *mut c_void) -> bool {
    true
}

/// Runs a `BaseFactory` operation over its server, with `send` lent.
///
/// # Safety
/// `factory` is a live `Factory`; `send` null or what `op` reads.
unsafe fn on_base(factory: *mut c_void, send: *mut c_void, op: u32, arg: u64) -> bool {
    // `Factory::server_`, the one member after the vtable pointer (master.cc asserts it).
    // SAFETY: as the caller guarantees.
    let serv = unsafe { *factory.cast::<*mut c_void>().wrapping_add(1) };
    let mut ctx = NativeContext::new(serv);
    ctx.send = send;
    // SAFETY: as the caller guarantees.
    unsafe { run(op, arg, &mut ctx) != 0 }
}

/// A `BaseFactory` method over one pointer, which is the operation's argument.
macro_rules! base_methods {
    ($($symbol:literal fn $name:ident($p:ident) = $op:ident, lend $lend:expr;)*) => {$(
        #[doc = concat!("`", $symbol, "`.")]
        ///
        /// # Safety
        /// `factory` is a live `BaseFactory`; the argument is the live record it takes.
        #[cfg_attr(feature = "upstream-symbols", unsafe(export_name = $symbol))]
        pub unsafe extern "C" fn $name(factory: *mut c_void, $p: *mut c_void) -> bool {
            let arg = $p.expose_provenance() as u64;
            // SAFETY: as the caller guarantees.
            unsafe { on_base(factory, if $lend { $p } else { ptr::null_mut() }, $op, arg) }
        }
    )*};
}

base_methods! {
    "_ZN6swoole11BaseFactory8dispatchEPNS_8SendDataE" fn base_factory_dispatch(task) = RUN_BASE_DISPATCH, lend true;
    "_ZN6swoole11BaseFactory6finishEPNS_8SendDataE" fn base_factory_finish(data) = RUN_BASE_FINISH, lend true;
    "_ZN6swoole11BaseFactory6notifyEPNS_8DataHeadE" fn base_factory_notify(info) = RUN_BASE_NOTIFY, lend false;
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
    // SAFETY: as the caller guarantees.
    let fd = unsafe { &*conn.cast::<ConnectionRecord>() }.fd;
    // SAFETY: as the caller guarantees.
    let port = unsafe { view::port_by_fd(serv, fd) };
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
///
/// # Safety
/// `serv` is a live `Server` and `conn` one of its connections.
#[cfg_attr(
    feature = "upstream-symbols",
    unsafe(export_name = "_ZNK6swoole6Server6notifyEPNS_10ConnectionENS_15ServerEventTypeE")
)]
pub unsafe extern "C" fn notify(
    serv: *mut c_void,
    conn: *const ConnectionRecord,
    event: c_int,
) -> bool {
    // SAFETY: as the caller guarantees.
    let conn = unsafe { &*conn };
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
    let mut ctx = NativeContext::new(serv);
    ctx.object = (&raw mut head).cast();
    // SAFETY: as the caller guarantees; the head lives for the call.
    unsafe { native_op(OP_FACTORY_NOTIFY, &mut ctx) }
}

/// `SW_SERVER_EVENT_PIPE_MESSAGE` (master.cc asserts it).
const EVENT_PIPE_MESSAGE: u8 = 10;

/// `Server::send_pipe_message(worker_id, msg)`: `msg` to that worker, through the master.
///
/// # Safety
/// `serv` is a live `Server`, `worker_id` one of its workers and `msg` a live `EventData`.
#[cfg_attr(
    feature = "upstream-symbols",
    unsafe(export_name = "_ZN6swoole6Server17send_pipe_messageEjPNS_9EventDataE")
)]
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

/// An `EventData`: `SW_IPC_MAX_SIZE` bytes (master.cc asserts it), aligned for its head.
#[repr(C, align(8))]
struct EventData([u8; IPC_BUFFER_SIZE + 40]);

/// `Server::send_pipe_message(worker_id, data, len)`: `data` packed as a task.
///
/// # Safety
/// `serv` is a live `Server` and `worker_id` one of its workers; `data` holds `len` bytes.
#[cfg_attr(
    feature = "upstream-symbols",
    unsafe(export_name = "_ZN6swoole6Server17send_pipe_messageEjPKcm")
)]
pub unsafe extern "C" fn send_pipe_message_data(
    serv: *mut c_void,
    worker_id: u32,
    data: *const c_char,
    len: usize,
) -> bool {
    let mut buf = EventData([0; IPC_BUFFER_SIZE + 40]);
    let mut ctx = NativeContext::new(serv);
    ctx.object = buf.0.as_mut_ptr().cast();
    ctx.data = data.cast();
    ctx.offset = i64::try_from(len).unwrap_or(i64::MAX);
    // SAFETY: as the caller guarantees; the buffer lives for the call.
    if !unsafe { native_op(OP_TASK_PACK, &mut ctx) } {
        return false;
    }
    // SAFETY: as the caller guarantees; task_pack filled the buffer's head.
    unsafe { send_pipe_message(serv, worker_id, buf.0.as_mut_ptr().cast()) }
}

/// `BaseFactory::end(session_id, flags)`.
///
/// # Safety
/// `factory` is a live `BaseFactory`.
#[cfg_attr(
    feature = "upstream-symbols",
    unsafe(export_name = "_ZN6swoole11BaseFactory3endEli")
)]
pub unsafe extern "C" fn base_factory_end(
    factory: *mut c_void,
    session_id: c_long,
    flags: c_int,
) -> bool {
    // The C++ forward's `(uint64_t) session_id | ((uint64_t) flags << 48)`.
    let arg = session_id.cast_unsigned() | (i64::from(flags).cast_unsigned() << 48);
    // SAFETY: as the caller guarantees.
    unsafe { on_base(factory, ptr::null_mut(), RUN_BASE_END, arg) }
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

    unsafe extern "C-unwind" fn fake(op: u32, arg: u64, ctx: *mut NativeContext) -> i64 {
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
                OP_TASK_PACK => {
                    // SAFETY: the context lends the buffer to pack into.
                    unsafe { (*head).len = 3 };
                    return 1;
                }
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
        swr_v1_server_set_native(fake);
        let serv = ptr::without_provenance_mut(0x10);
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
                op == RUN_ADD_PORT && s == 0x10 && number == 9 && host != 0,
                "add_port context"
            );
            assert!(
                add_connection(serv, at(0x20), at(0x30), 3).is_null(),
                "zero yields null"
            );
            assert_eq!(
                last(),
                (RUN_ADD_CONNECTION, 0, 0x10, 0x20, 0x30, 0, 0, 0),
                "add_connection context"
            );
            foreach_connection(serv, at(0x40));
            assert_eq!(last().5, 0x40, "callback in object");
            assert!(!feedback(serv, at(0x60), 5), "feedback result");
            assert_eq!(last().1, 5, "event arg");
            send_family_and_base_factory(serv);
        }
    }

    unsafe fn send_family_and_base_factory(serv: *mut c_void) {
        let at = |a: usize| ptr::without_provenance_mut::<c_void>(a);
        unsafe {
            let data = at(0x80);
            assert!(send(serv, 9, data, 4), "send's 1 is true");
            assert_eq!(
                sent(),
                (
                    RUN_ST_SEND,
                    sent().3 as u64,
                    0x10,
                    sent().3,
                    0x80,
                    4,
                    9,
                    0,
                    0
                ),
                "send context, with its SendData lent"
            );
            assert_ne!(sent().3, 0, "a SendData is lent");
            assert!(!sendwait(serv, 9, data, 4), "sendwait result");
            assert_eq!(sent().1, 9, "sendwait passes the session");
            assert!(!has_kernel_nobufs_error(serv, 11), "nobufs result");
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
                (RUN_SC_SEND, 2, 0x10, task_at.addr(), 0x80, 5, 13, 0, 0),
                "send_to_connection reads the SendData"
            );

            assert_eq!(
                create_base_factory(serv).addr(),
                0x70,
                "the created factory"
            );
            assert_eq!(sent().0, RUN_BASE_CREATE, "create op");
            // A Factory: its vptr, then server_.
            let mut factory = [at(0x90), serv];
            let factory_at = factory.as_mut_ptr().cast::<c_void>();
            assert!(base_factory_start(factory_at) && base_factory_shutdown(factory_at));
            assert!(
                !base_factory_dispatch(factory_at, task_at),
                "dispatch result"
            );
            let (op, arg, s, lent, ..) = sent();
            assert!(
                op == RUN_BASE_DISPATCH
                    && arg == task_at.addr() as u64
                    && s == 0x10
                    && lent == task_at.addr(),
                "dispatch lends the task"
            );
            base_factory_notify(factory_at, at(0xa0));
            let (op, arg, s, lent, ..) = sent();
            assert!(
                op == RUN_BASE_NOTIFY && arg == 0xa0 && s == 0x10 && lent == 0,
                "notify passes the head"
            );
            base_factory_end(factory_at, 7, 6);
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
            assert!(notify(serv, &raw const conn, 0x104), "notify result");
            assert_eq!(sent().1, u64::from(OP_FACTORY_NOTIFY), "notify op");
            let (_, h) = head();
            assert!(
                h.fd == 22 && h.reactor_id == 3 && h.kind == 4 && h.server_fd == 5,
                "notify head keeps the low bits"
            );

            assert!(
                send_pipe_message_data(serv, 2, c"abc".as_ptr(), 3),
                "pipe result"
            );
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
