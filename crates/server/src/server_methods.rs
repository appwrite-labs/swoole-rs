//! The public `Server` methods of `src/server/master.cc`, and the `BaseFactory`
//! methods that were `src/server/base.cc`, that are one operation over a fresh
//! context. Each method fills the context the way its C++ forward did and runs over
//! it in Rust.

use core::ffi::{CStr, c_int, c_long, c_void};
use core::ptr;
use std::ffi::CString;

use crate::base_factory::{SEND_DATA, SendData};
use crate::ops::{OP_FACTORY_END, OP_FACTORY_NOTIFY, OP_PIPE_TO_WORKER};
use crate::ops::{
    RUN_ACCEPT, RUN_CREATE, RUN_DESTROY, RUN_INCOMING, RUN_SIGNAL, RUN_START, RUN_START_CHECK,
    RUN_START_MASTER_THREAD, RUN_STORE_LISTEN_SOCKET,
};
use crate::ops::{
    RUN_ADD_CONNECTION, RUN_ADD_PORT, RUN_BASE_CREATE, RUN_FEEDBACK, RUN_IDLE_TASK_WORKERS,
    RUN_IDLE_WORKERS, RUN_NATIVE_OP, RUN_SC_SEND, RUN_SCHEDULE_WORKER, RUN_SENDFILE, RUN_SENDWAIT,
    RUN_SET_MAX_CONNECTION, RUN_SHUTDOWN, RUN_ST_ABORT, RUN_ST_CLEAR_TIMER, RUN_ST_DOCUMENT_ROOT,
    RUN_ST_SEND, RUN_ST_STORE_PIPE_FD, RUN_SYSTEMD_SOCKETS, RUN_TASKING,
};
use crate::view;
use swoole_rs_corebase::{DataHead, log};

/// What an operation acts on besides the server. The borrowed fields live for `'a`,
/// the call.
pub struct NativeContext<'a> {
    pub(crate) serv: *mut c_void,
    pub(crate) reactor: *mut c_void,
    pub(crate) event: Option<&'a swoole_rs_reactor::Event>,
    pub(crate) port: *mut c_void,
    pub(crate) sock: *mut c_void,
    /// The connection the operation acts on or selects.
    pub(crate) conn: Option<crate::Connection<'a>>,
    pub(crate) server_fd: c_int,
    pub(crate) reactor_id: u8,
    pub(crate) send: *mut c_void,
    pub(crate) data: *const c_void,
    /// The bytes the operations that read a slice take: `sendwait`'s data, `sendfile`'s
    /// name and the document root, each up to its first NUL.
    pub(crate) bytes: &'a [u8],
    pub(crate) length: u32,
    pub(crate) session_id: c_long,
    pub(crate) buffer: Option<&'a mut SendfileBuffer>,
    pub(crate) offset: i64,
    pub(crate) socket_type: c_int,
    pub(crate) host: Option<&'a CStr>,
    pub(crate) number: c_int,
    pub(crate) object: *mut c_void,
}

impl NativeContext<'_> {
    pub(crate) fn new(serv: *mut c_void) -> Self {
        Self {
            serv,
            reactor: ptr::null_mut(),
            event: None,
            port: ptr::null_mut(),
            sock: ptr::null_mut(),
            conn: None,
            server_fd: 0,
            reactor_id: 0,
            send: ptr::null_mut(),
            data: ptr::null(),
            bytes: &[],
            length: 0,
            session_id: 0,
            buffer: None,
            offset: 0,
            socket_type: 0,
            host: None,
            number: 0,
            object: ptr::null_mut(),
        }
    }
}

// Runs `op` over `ctx`, whose `serv` is a live `Server` and whose other fields are what
// `op` reads: straight into Rust, or the tests' fake runner.
#[cfg(not(test))]
use crate::run_method as run;
#[cfg(test)]
use tests::fake as run;

/// Runs `op` with `arg` over a context that lends `serv` and nothing else.
fn run_on(serv: view::Server<'_>, op: u32, arg: u64) -> i64 {
    run_over(serv, op, arg).0
}

/// [`run_on`], and the context afterwards, for what `op` left in it.
fn run_over<'a>(serv: view::Server<'_>, op: u32, arg: u64) -> (i64, NativeContext<'a>) {
    let mut ctx = NativeContext::new(serv.raw());
    // SAFETY: a live server, and no other object for the operation to read.
    let result = unsafe { run(op, arg, &mut ctx) };
    (result, ctx)
}

/// Each method that is one operation over the server alone: its Rust name, return type
/// and operation. `bool` and `int` returns narrow like C++.
macro_rules! plain_methods {
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
    fn shutdown() -> bool = RUN_SHUTDOWN;
    fn get_idle_worker_num() -> u32 = RUN_IDLE_WORKERS;
    fn start() -> c_int = RUN_START;
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

/// `Server::create()`: `SW_ERR` once created, else the hook and then startup's create.
#[must_use]
pub fn create(serv: view::Server<'_>) -> c_int {
    if !serv.config().factory_.is_null() {
        return -1;
    }
    serv.call_hook(view::ServerHook::BeforeCreate);
    c_int::narrow(run_on(serv, RUN_CREATE, 0))
}

/// `Server::destroy()`: nothing before the server is created.
pub fn destroy(serv: view::Server<'_>) {
    if !serv.config().factory_.is_null() {
        run_on(serv, RUN_DESTROY, 0);
    }
}

/// `delete serv`'s first step: [`destroy`] a server still running in its master,
/// which runs its shutdown callbacks. Nothing the second time.
pub fn destroy_running(serv: view::Server<'_>) {
    let counters = serv.counters();
    let master = std::process::id().cast_signed()
        == counters
            .master_pid
            .load(core::sync::atomic::Ordering::Relaxed);
    if master
        && counters
            .shutdown
            .load(core::sync::atomic::Ordering::Relaxed)
            == 0
    {
        destroy(serv);
    }
}

/// `Server::start_master_thread(reactor)`, on the master's reactor.
#[cfg_attr(
    test,
    expect(dead_code, reason = "its callers are the hybrid's, outside tests")
)]
#[must_use]
pub(crate) fn start_master_thread(
    serv: view::Server<'_>,
    reactor: crate::reactor_handle::Reactor,
) -> c_int {
    let mut ctx = NativeContext::new(serv.raw());
    ctx.reactor = reactor.ptr();
    // SAFETY: the live server and its master's live reactor, lent for the call.
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
        swoole_rs_corebase::fatal("pipe sockets set up before the server started");
    }
    let mut ctx = NativeContext::new(serv.raw());
    ctx.object = mb;
    // SAFETY: as the caller guarantees.
    unsafe { run(crate::ops::RUN_ST_INIT_PIPE_SOCKETS, 0, &mut ctx) };
}

/// `Server::connection_incoming(reactor, conn)`, over the port `conn` arrived on.
///
/// # Safety
/// `reactor` is the reactor of `conn`, one of `serv`'s live connections.
pub unsafe fn connection_incoming(
    serv: view::Server<'_>,
    reactor: *mut c_void,
    conn: crate::Connection<'_>,
) -> c_int {
    let mut ctx = NativeContext::new(serv.raw());
    ctx.reactor = reactor;
    ctx.port = serv
        .port_by_server_fd(conn.server_fd())
        .map_or(ptr::null_mut(), view::Port::raw);
    ctx.sock = conn.socket_ptr();
    ctx.conn = Some(conn);
    // SAFETY: as the caller guarantees.
    c_int::narrow(unsafe { run(RUN_INCOMING, 0, &mut ctx) })
}

/// Arms the accept-retry timer on this thread: once, after `msec`, it re-enables
/// accepting on `serv`. The timer's node, or null.
#[cfg(all(feature = "upstream-symbols", not(test)))]
pub(crate) fn accept_retry_timer(serv: view::Server<'_>, msec: c_long) -> *mut c_void {
    let serv = serv.raw();
    swoole_rs_timer::add_fn(msec, false, move || {
        // SAFETY: the server that armed this one-shot timer, on its own thread; it
        // outlives its timers, which `clear_timer` deletes before it goes.
        let serv = unsafe { view::Server::from_raw(serv) };
        run_on(serv, crate::ops::RUN_ENABLE_ACCEPT, 0);
    })
}

/// `Server::accept_connection(reactor, event)`: the master reactor's read handler
/// for listening sockets, over the server the reactor carries.
///
/// Registered only on a server's reactor (its `ptr` is the live `Server`) for its
/// listening sockets.
pub(crate) extern "C" fn accept_connection(
    reactor: swoole_rs_reactor::Running<'_>,
    event: &mut swoole_rs_reactor::Event,
) -> c_int {
    let serv = reactor.object();
    let mut ctx = NativeContext::new(serv);
    ctx.reactor = reactor.as_ptr();
    ctx.server_fd = event.fd();
    ctx.event = Some(event);
    // SAFETY: the server reactor's live server, and its event.
    c_int::narrow(unsafe { run(RUN_ACCEPT, 0, &mut ctx) })
}

/// `Server::get_startup_error_message()`.
#[must_use]
pub fn get_startup_error_message() -> CString {
    let message = swoole_rs_error::last_error_message();
    crate::state::startup_error(message, log::swr_v1_core_last_error())
}

/// `Server::set_max_connection(max)`.
pub fn set_max_connection(serv: view::Server<'_>, max: u32) {
    run_on(serv, RUN_SET_MAX_CONNECTION, max.into());
}

/// `Server::feedback(conn, event)`, over one of `serv`'s connections.
#[must_use]
pub fn feedback(serv: view::Server<'_>, conn: crate::Connection<'_>, event: c_int) -> bool {
    let mut ctx = NativeContext::new(serv.raw());
    ctx.conn = Some(conn);
    // SAFETY: the live server and its connection, lent for the call.
    unsafe { run(RUN_FEEDBACK, i64::from(event).cast_unsigned(), &mut ctx) != 0 }
}

/// `Server::store_pipe_fd(worker->pipe_object)`, when `worker` has a pipe.
pub(crate) fn store_pipe_fd(
    serv: view::Server<'_>,
    worker: swoole_rs_process::worker_record::Worker<'_>,
) {
    let pipe = worker.pipe_object();
    if pipe.is_null() {
        return;
    }
    let mut ctx = NativeContext::new(serv.raw());
    ctx.object = pipe;
    // SAFETY: the live server, and the pipe object of one of its live workers: the
    // live `UnixSocket` the server or its pool made for the worker.
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

/// `Server::add_port(type, host, port)`: the new port, or `None`.
#[must_use]
pub fn add_port<'a>(
    serv: view::Server<'a>,
    socket_type: c_int,
    host: &CStr,
    port: c_int,
) -> Option<view::Port<'a>> {
    let mut ctx = NativeContext::new(serv.raw());
    ctx.socket_type = socket_type;
    ctx.host = Some(host);
    ctx.number = port;
    // SAFETY: the live server.
    if unsafe { run(RUN_ADD_PORT, 0, &mut ctx) } == 0 {
        return None;
    }
    // SAFETY: the port the server just added, which it keeps for its life.
    unsafe { view::Port::from_raw(ctx.port) }
}

/// `Server::abort_connection(reactor, port, socket)`.
///
/// # Safety
/// `serv` is a live `Server`; the others are the reactor, port and socket to abort.
pub unsafe fn abort_connection(
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
pub unsafe fn add_connection(
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
            .map_or(ptr::null_mut(), |conn| conn.as_ptr().cast())
    } else {
        ptr::null_mut()
    }
}

/// `Server::send(session_id, data, length)`: the operation reads the `SendData` it lends.
/// The length narrows to `uint32_t`, as the C++ callers' `ZSTR_LEN` did.
#[expect(
    clippy::must_use_candidate,
    reason = "callers ignore the result where upstream's do"
)]
pub fn send(serv: view::Server<'_>, session_id: c_long, data: &[u8]) -> bool {
    #[expect(
        clippy::cast_possible_truncation,
        reason = "Server::send's uint32_t length, as upstream's callers narrow it"
    )]
    let length = data.len() as u32;
    let data: *const c_void = data.as_ptr().cast();
    let mut send = SendData {
        info: DataHead {
            fd: session_id,
            kind: SEND_DATA,
            len: length,
            ..DataHead::default()
        },
        data: data.cast(),
    };
    let mut ctx = NativeContext::new(serv.raw());
    ctx.send = (&raw mut send).cast();
    ctx.session_id = session_id;
    ctx.data = data;
    ctx.length = length;
    let arg = ctx.send.expose_provenance() as u64;
    // SAFETY: the live server; the data and the SendData live for the call.
    unsafe { run(RUN_ST_SEND, arg, &mut ctx) != 0 }
}

/// `text` up to its first NUL, as the C string at its start reads: one `memchr` for
/// the usual text without one.
fn until_nul(text: &[u8]) -> &[u8] {
    if text.contains(&0) {
        text.split(|byte| *byte == 0).next().unwrap_or_default()
    } else {
        text
    }
}

/// `Server::set_document_root(path)`, before the server starts.
#[expect(
    clippy::must_use_candidate,
    reason = "`swoole_server_set` ignores the result, as upstream's does"
)]
pub fn set_document_root(serv: view::Server<'_>, path: &[u8]) -> bool {
    let mut ctx = NativeContext::new(serv.raw());
    ctx.data = path.as_ptr().cast();
    ctx.bytes = until_nul(path);
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
pub(crate) unsafe extern "C-unwind" fn master_timer_tick(serv: *mut c_void) {
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
pub(crate) unsafe extern "C-unwind" fn idle_timeout_tick(conn: *mut c_void) {
    // SAFETY: the connection's server, live for the tick per the caller.
    let Some(serv) = (unsafe { view::Server::current() }) else {
        return;
    };
    let mut ctx = NativeContext::new(serv.raw());
    // SAFETY: as the caller guarantees.
    let Some(conn) = (unsafe { crate::Connection::from_ptr(conn.cast()) }) else {
        return;
    };
    ctx.port = serv
        .port_by_server_fd(conn.server_fd())
        .map_or(ptr::null_mut(), view::Port::raw);
    ctx.reactor = swoole_rs_corebase::thread_reactor();
    ctx.sock = conn.socket_ptr();
    ctx.conn = Some(conn);
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
    ctx.bytes = data.get(..length as usize).unwrap_or(data);
    // SAFETY: the live server; `data` holds `length` bytes for the call.
    unsafe { run(RUN_SENDWAIT, session_id.cast_unsigned(), &mut ctx) != 0 }
}

/// `Server::has_kernel_nobufs_error(session_id)`: whether a send to the session's
/// socket, or in process mode to its reactor pipe, hit `ENOBUFS` since the last
/// call (which clears it). Called after every `Server::send`, so it reads the
/// records directly rather than through the op runner.
#[must_use]
pub fn has_kernel_nobufs_error(serv: view::Server<'_>, session_id: c_long) -> bool {
    let state = serv.state();
    // `get_connection(int)` of the session id, as master.cc did.
    #[expect(clippy::cast_possible_truncation, reason = "C++'s int parameter")]
    let fd = session_id as c_int;
    let Some(conn) = state.connection(c_long::from(fd), 0) else {
        return false;
    };
    let Some(socket) = conn.socket() else {
        return false;
    };
    let config = state.config_ref();
    if u64::from(config.mode_) != crate::MODE_PROCESS {
        return socket.take_kernel_nobufs();
    }
    // `serv->get_reactor_pipe_socket(session_id, conn->reactor_id)`.
    let pipes = i64::from(state.tables_ref().reactor_pipe_num);
    let index = session_id % pipes;
    let id = i64::from(conn.reactor_id()) + index * i64::from(config.reactor_num.get());
    let id = u16::try_from(id).unwrap_or(u16::MAX);
    serv.worker_pipe(id, false)
        .is_some_and(|pipe| pipe.take_kernel_nobufs())
}

/// `Server::send_to_connection(send)`: `SW_OK` or `SW_ERR`.
///
/// # Safety
/// `send`'s data is live for the call.
#[expect(
    clippy::must_use_candidate,
    reason = "callers ignore the result where upstream's do"
)]
pub unsafe fn send_to_connection(serv: view::Server<'_>, send: &SendData) -> c_int {
    let header = send;
    let mut ctx = NativeContext::new(serv.raw());
    ctx.send = ptr::from_ref(send).cast_mut().cast();
    ctx.session_id = header.info.fd;
    ctx.data = header.data.cast();
    ctx.length = header.info.len;
    // SAFETY: as the caller guarantees.
    c_int::narrow(unsafe { run(RUN_SC_SEND, header.info.kind.into(), &mut ctx) })
}

const IPC_BUFFER_SIZE: usize = swoole_rs_corebase::constants::SW_IPC_BUFFER_SIZE;

/// `Server::sendfile`'s buffer, aligned for the `SendfileTask` it starts with.
#[repr(C, align(8))]
pub(crate) struct SendfileBuffer(pub(crate) [u8; IPC_BUFFER_SIZE]);

/// `Server::sendfile(session_id, file, l_file, offset, length)`, `l_file` being the name's
/// length narrowed to `uint32_t`, as upstream's callers do. The task copies the name up to
/// its first NUL (`swoole_strlcpy`), so that much of it is lent.
#[expect(
    clippy::must_use_candidate,
    reason = "callers ignore the result where upstream's do"
)]
pub fn sendfile(
    serv: view::Server<'_>,
    session_id: c_long,
    file: &[u8],
    offset: libc::off_t,
    length: usize,
) -> bool {
    #[expect(
        clippy::cast_possible_truncation,
        reason = "Server::sendfile's uint32_t name length, as upstream's callers narrow it"
    )]
    let l_file = file.len() as u32;
    let mut buffer = SendfileBuffer([0; IPC_BUFFER_SIZE]);
    // `SendfileTask::length` follows its `off_t offset` (server.cc asserts it).
    buffer.0[8..16].copy_from_slice(&length.to_ne_bytes());
    let mut ctx = NativeContext::new(serv.raw());
    ctx.session_id = session_id;
    ctx.bytes = until_nul(file);
    ctx.length = l_file;
    ctx.buffer = Some(&mut buffer);
    ctx.offset = offset;
    // SAFETY: the live server.
    unsafe { run(RUN_SENDFILE, session_id.cast_unsigned(), &mut ctx) != 0 }
}

/// `Server::create_base_factory()`: the new `BaseFactory`, or null.
#[must_use]
pub fn create_base_factory(serv: view::Server<'_>) -> *mut c_void {
    let (created, ctx) = run_over(serv, RUN_BASE_CREATE, 0);
    if created != 0 {
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

/// `Server::get_package_max_length(conn)`, over the fd of one of `serv`'s connections.
#[must_use]
pub fn package_max_length(serv: view::Server<'_>, fd: c_int) -> u32 {
    // The connection's live port, as in C++, which doesn't check it either.
    serv.port_of_fd(fd)
        .map_or(0, |port| port.protocol.get().package_max_length)
}

/// `Server::CLOSE_RESET`, `Server::CLOSE_ACTIVELY`.
const CLOSE_RESET: c_int = 1 << 1;
const CLOSE_ACTIVELY: c_int = 1 << 2;

/// `Server::close(session_id, reset)`: the factory ends the session, actively.
#[expect(
    clippy::must_use_candidate,
    reason = "callers ignore the result where upstream's do"
)]
pub fn close(serv: view::Server<'_>, session_id: c_long, reset: bool) -> bool {
    let mut ctx = NativeContext::new(serv.raw());
    ctx.session_id = session_id;
    ctx.number = CLOSE_ACTIVELY | if reset { CLOSE_RESET } else { 0 };
    // SAFETY: the live server.
    unsafe { run(RUN_NATIVE_OP, OP_FACTORY_END.into(), &mut ctx) != 0 }
}

/// `Server::notify(conn, event)`: the factory hears `event` for `conn`.
#[expect(
    clippy::must_use_candidate,
    reason = "callers ignore the result where upstream's do"
)]
pub fn notify(serv: view::Server<'_>, conn: crate::Connection<'_>, event: c_int) -> bool {
    #[expect(
        clippy::cast_possible_truncation,
        clippy::cast_sign_loss,
        reason = "DataHead's uint8_t type and uint16_t server_fd keep the low bits, as the C++ assignments did"
    )]
    let mut head = DataHead {
        fd: conn.fd().into(),
        reactor_id: conn.reactor_id().into(),
        kind: event as u8,
        server_fd: conn.server_fd() as u16,
        ..DataHead::default()
    };
    let mut ctx = NativeContext::new(serv.raw());
    ctx.object = (&raw mut head).cast();
    // SAFETY: the live server; the head lives for the call.
    unsafe { run(RUN_NATIVE_OP, OP_FACTORY_NOTIFY.into(), &mut ctx) != 0 }
}

/// `SW_SERVER_EVENT_PIPE_MESSAGE` (server.cc asserts it).
const EVENT_PIPE_MESSAGE: u8 = 10;

/// `Server::send_pipe_message(worker_id, msg)`: `msg` to that worker, through the master.
pub fn send_pipe_message(
    serv: view::Server<'_>,
    worker_id: u32,
    msg: &mut swoole_rs_process::task_file::TaskBuffer,
) -> bool {
    msg.set_kind(EVENT_PIPE_MESSAGE);
    let mut ctx = NativeContext::new(serv.raw());
    ctx.number = worker_id.cast_signed();
    // `EventData::size()`: the head and its payload.
    ctx.length = 40 + msg.head().len;
    ctx.object = msg.as_mut_ptr();
    // SAFETY: the live server, and the packed message, which lives for the call and
    // holds the payload its head counts.
    unsafe { run(RUN_NATIVE_OP, OP_PIPE_TO_WORKER.into(), &mut ctx) != 0 }
}

/// Runs a command `operation` over the host of a `Server` method whose context lends
/// `serv` and nothing else, as [`run_on`] does an operation by number; true, as the
/// runner's `SW_ERR` narrows to, when a native fault stops it.
fn over_server(
    serv: view::Server<'_>,
    operation: impl FnOnce(&mut crate::Native<'_, '_>) -> crate::Effect<bool>,
) -> bool {
    let mut ctx = NativeContext::new(serv.raw());
    let mut host = crate::Native {
        state: Some(serv.state()),
        master: &raw mut ctx,
        server: Some(serv),
        reactor: None,
    };
    operation(&mut host).unwrap_or(true)
}

/// `Server::add_command(name, accepted_process_types, handler)`: whether the
/// command joined. The server keeps the handler for its life.
pub fn swr_v1_server_add_command(
    serv: view::Server<'_>,
    name: &[u8],
    accepted_types: c_int,
    handler: crate::CommandHandler,
) -> bool {
    let add = crate::commands::AddCommand {
        name,
        accepted_types,
        handler,
    };
    over_server(serv, |h| crate::commands::add_command(h, &add))
}

/// `Server::command(process_id, process_type, name, message, callback)`: whether
/// the request went. Its reply reaches `callback` once, perhaps before this returns.
pub fn swr_v1_server_command(
    serv: view::Server<'_>,
    process_id: u32,
    process_type: c_int,
    name: &[u8],
    message: &[u8],
    callback: crate::CommandCallback,
) -> bool {
    let send = crate::commands::SendCommand {
        serv,
        process_id,
        process_type,
        name,
        message,
        callback,
    };
    over_server(serv, |h| crate::commands::command(h, &send))
}

/// `Server::call_command_callback(request_id, message)`: the reply to `request_id`
/// reaches its callback, or the server logs an invalid command result.
pub fn swr_v1_server_command_result(serv: view::Server<'_>, request_id: i64, message: &[u8]) {
    crate::commands::command_result(serv.state(), request_id, message);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ops::{RUN_BASE_DISPATCH, RUN_BASE_END, RUN_BASE_NOTIFY};
    use crate::records::ConnectionRecord;
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
    /// The name the last sendfile lent, read as the task's `swoole_strlcpy` reads it.
    static SENDFILE_NAME: Mutex<Vec<u8>> = Mutex::new(Vec::new());

    pub(super) unsafe fn fake(op: u32, arg: u64, ctx: &mut NativeContext<'_>) -> i64 {
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
                ctx.host.map_or(0, |host| host.as_ptr().addr()),
            ));
        if op == RUN_SENDFILE {
            // sendfile lends the name as far as the task copies it.
            *SENDFILE_NAME
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner) = ctx.bytes.to_vec();
        }
        // sendfile's buffer starts with a `SendfileTask`, its length at 8.
        let task_length = ctx.buffer.as_ref().map_or(0, |buffer| {
            usize::from_ne_bytes(buffer.0[8..16].try_into().unwrap_or_default())
        });
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
        assert!(shutdown(server()), "shutdown's 1 is true");
        assert_eq!(last().0, RUN_SHUTDOWN, "shutdown op");
        assert_eq!(get_idle_worker_num(server()), u32::MAX, "(uint32_t) -1");
        set_max_connection(server(), 7);
        assert_eq!(last().1, 7, "max connection arg");
        unsafe {
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
                add_port(server(), 1, c"h", 9).map(|port| port.raw().addr()),
                Some(0x50),
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
            // SAFETY: a zeroed record is a valid connection.
            let mut conn: ConnectionRecord = core::mem::zeroed();
            assert!(
                !feedback(server(), (&mut conn).into(), 5),
                "feedback result"
            );
            assert_eq!(last().1, 5, "event arg");
            send_family_and_base_factory(serv);
        }
    }

    unsafe fn send_family_and_base_factory(serv: *mut c_void) {
        let sv = serv.addr();
        let at = |a: usize| ptr::without_provenance_mut::<c_void>(a);
        unsafe {
            let data = at(0x80);
            let bytes = [0u8; 4];
            let at_bytes = bytes.as_ptr().addr();
            assert!(send(server(), 9, &bytes), "send's 1 is true");
            assert_eq!(
                sent(),
                (
                    RUN_ST_SEND,
                    sent().3 as u64,
                    sv,
                    sent().3,
                    at_bytes,
                    4,
                    9,
                    0,
                    0
                ),
                "send context, with its SendData lent"
            );
            assert_ne!(sent().3, 0, "a SendData is lent");
            assert!(!sendwait(server(), 9, &[0; 4]), "sendwait result");
            assert_eq!(sent().1, 9, "sendwait passes the session");
            assert!(
                !sendfile(server(), 12, &b"fx"[..1], 3, 99),
                "sendfile result"
            );
            assert_eq!(
                *SENDFILE_NAME
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner),
                b"f",
                "the name ends at its length, not past it"
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
            assert_eq!(send_to_connection(server(), &task), -1, "SW_ERR");
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
            close_notify_and_pipe_messages();
        }
    }

    fn head() -> (c_int, DataHead) {
        *HEADS
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .last()
            .unwrap_or(&(0, DataHead::default()))
    }

    unsafe fn close_notify_and_pipe_messages() {
        unsafe {
            assert!(close(server(), 21, true), "close result");
            let (op, arg, _, _, _, _, session, ..) = sent();
            assert!(
                op == RUN_NATIVE_OP && arg == u64::from(OP_FACTORY_END) && session == 21,
                "close ends the session"
            );
            assert_eq!(head().0, CLOSE_ACTIVELY | CLOSE_RESET, "reset flags");
            close(server(), 21, false);
            assert_eq!(head().0, CLOSE_ACTIVELY, "active flags");

            // SAFETY: a zeroed record is a valid connection.
            let mut conn: ConnectionRecord = core::mem::zeroed();
            conn.fd = 22;
            conn.reactor_id = 3;
            conn.server_fd = 0x1_0005;
            assert!(notify(server(), (&mut conn).into(), 0x104), "notify result");
            assert_eq!(sent().1, u64::from(OP_FACTORY_NOTIFY), "notify op");
            let (_, h) = head();
            assert!(
                h.fd == 22 && h.reactor_id == 3 && h.kind == 4 && h.server_fd == 5,
                "notify head keeps the low bits"
            );

            let mut message = swoole_rs_process::task_file::TaskBuffer::default();
            assert!(message.pack(b"abc"), "packs inline");
            assert!(send_pipe_message(server(), 2, &mut message), "pipe result");
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

    #[test]
    #[cfg_attr(miri, ignore = "Miri has no shared mappings")]
    fn kernel_nobufs_takes_the_sockets_flag_or_in_process_mode_the_reactor_pipes() {
        use crate::records::ConnectionRecord;
        use crate::records::tests::naming;
        use swoole_rs_process::worker_record::WorkerRecord;
        use swoole_rs_reactor::SocketRecord;

        // SAFETY: plain records, for which zero is valid.
        let (mut sock, mut pipe): (SocketRecord, SocketRecord) =
            unsafe { (core::mem::zeroed(), core::mem::zeroed()) };
        sock.kernel_nobufs = true;
        pipe.kernel_nobufs = true;
        // SAFETY: as above.
        let mut connections: [ConnectionRecord; 4] = unsafe { core::mem::zeroed() };
        connections[3].socket = (&raw mut sock).cast();
        connections[3].reactor_id = 1;
        // SAFETY: as above.
        let mut workers: [WorkerRecord; 4] = unsafe { core::mem::zeroed() };
        // Session 3 on reactor 1 of 2, over 2 pipes: worker 1 + (3 % 2) * 2.
        workers[3].pipe_worker = (&raw mut pipe).cast();
        let state = crate::swr_v1_server_state_new();
        // SAFETY: a fresh state; the records above outlive every read below.
        let fields = unsafe {
            let config = (*state).config();
            // The whole `serv->gs`, as `ServerObject` maps it: the worker lookup reads its
            // zeroed event pool, past the counters.
            (*config).gs =
                swoole_rs_memory::members::Shm::calloc(1, size_of::<crate::shared::ServerShared>());
            (*config).workers = naming(&mut workers);
            (*config).worker_num.set(4);
            (*config).reactor_num.set(2);
            let tables = &mut *(*state).tables();
            tables.connection_list = naming(&mut connections);
            tables.max_connection = 4;
            tables.reactor_pipe_num = 2;
            [state.cast::<c_void>(), config.cast::<c_void>()]
        };
        // SAFETY: laid out as a Server's head, over the live state and config.
        let serv = unsafe { view::Server::from_raw((&raw const fields).cast_mut().cast()) };
        let set_mode = |mode: u8| {
            // SAFETY: the state's config, which no reference holds across this write.
            unsafe { (*(*state).config()).mode_ = mode };
        };

        set_mode(1);
        assert!(!has_kernel_nobufs_error(serv, 2), "no socket");
        assert!(!has_kernel_nobufs_error(serv, 4), "past the table");
        assert!(has_kernel_nobufs_error(serv, 3), "the socket's flag");
        assert!(!has_kernel_nobufs_error(serv, 3), "taken");
        assert!(pipe.kernel_nobufs, "base mode leaves the pipe");

        sock.kernel_nobufs = true;
        set_mode(2);
        assert!(has_kernel_nobufs_error(serv, 3), "the reactor pipe's flag");
        assert!(!has_kernel_nobufs_error(serv, 3), "taken");
        assert!(sock.kernel_nobufs, "process mode leaves the socket");

        // SAFETY: allocated above, not used again.
        unsafe {
            (*(*state).config()).gs.free();
            crate::swr_v1_server_state_free(state);
        }
    }
}
