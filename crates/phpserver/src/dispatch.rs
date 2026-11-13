//! Server events into PHP callbacks: the arguments (`Swoole\Server\Event` and
//! friends, or plain values), the call and its "handler error" warning. The
//! Server's on* slots hold this module's `extern "C"` fns (stored with a null
//! `ctx`, see `swoole_rs_server::callbacks`); each re-raises a bailout it
//! trapped last, from its outermost frame (docs/zend.md).
use crate::callable::Callable;
use crate::lifecycle::{Install, PortCallback, ServerCallback};
use crate::lifecycle::{Props, ServerObject};
use crate::server::DATA;
use std::{
    any::Any,
    ffi::{CStr, c_int},
    mem::ManuallyDrop,
    ptr,
    sync::{Arc, OnceLock, atomic::Ordering::Relaxed},
};
use swoole_rs_clients::address::{swr_v1_clients_address_port, with_host};
use swoole_rs_corebase::ServerEvent;
use swoole_rs_corebase::constants::SW_IPC_BUFFER_SIZE;
use swoole_rs_corebase::{DataHead, log, net_types::SocketAddress};
use swoole_rs_phpcoroutines::create_coroutine;
use swoole_rs_process::methods::ExitStatus;
use swoole_rs_process::task_file::Packed;
use swoole_rs_process::worker_record::Worker;
use swoole_rs_server::ConnectionRecord;
use swoole_rs_server::callbacks::{Callback, CallbackKind, Ctx};
use swoole_rs_zend::host::{callable, function_call};
use swoole_rs_zend::{
    E_WARNING, Owned, OwnedArray, Value, ZString,
    bailout::{self, Bailout},
    long_zval, message, report,
    sys::{zend_fcall_info_cache, zval, zval_ptr_dtor},
};

/// `SW_ERROR_PHP_RUNTIME_NOTICE`.
const SW_ERROR_PHP_RUNTIME_NOTICE: c_int = 510;

use crate::instance::{php_swoole_get_recv_data, php_swoole_server_get_callback};
use swoole_rs_server::{Mode, server_methods, view};

/// What the dispatch reads of the Server, per event.
struct Facts {
    master_pid: i64,
    manager_pid: i64,
    event_object: bool,
    enable_coroutine: bool,
    coroutine: bool,
    enable_library: bool,
    task_worker: bool,
}

/// A datagram and its sender, for onPacket.
struct Packet<'a> {
    data: &'a [u8],
    address: Option<&'a CStr>,
    port: i64,
    server_port: i64,
}

/// `DATA`'s classes.
const EVENT: usize = 0;
const PACKET: usize = 1;
const PIPE_MESSAGE: usize = 2;
const STATUS_INFO: usize = 3;

/// The result of a call through the host.
#[derive(PartialEq, Eq)]
enum Called {
    Failed,
    Done,
    Bailout,
}

/// An `IS_UNDEF` zval.
#[inline]
pub(crate) fn undef() -> zval {
    swoole_rs_zend::UNDEF
}

/// `zend::function::call(cb, argc, args, nullptr, coroutine)`, warning
/// "`<class>->on<event> handler error`" (`php_swoole_error`) when it fails.
#[inline]
fn call(cb: &Callable, args: &mut [zval], coroutine: bool, event: &str) -> Called {
    match invoke(cb, args, ptr::null_mut(), coroutine) {
        Ok(true) => Called::Done,
        Err(_) => Called::Bailout,
        Ok(false) => failed(event),
    }
}

/// `zend::function::call(cb, argc, args, retval, coroutine)`: whether the
/// callable ran (or its coroutine started), or the bailout it raised, which
/// the caller re-raises or hands to C++ (it is no longer pending here).
pub(crate) fn invoke(
    cb: &Callable,
    args: &mut [zval],
    retval: *mut zval,
    coroutine: bool,
) -> Result<bool, Bailout> {
    #[expect(clippy::cast_possible_truncation, reason = "at most five arguments")]
    let argc = args.len() as u32;
    // SAFETY: the resolved cache of a callable the caller holds through the
    // call, `argc` initialised arguments and the return slot or null; the
    // coroutine leaf and the reporter run inside C traps.
    let called = unsafe {
        function_call(
            cb.cache_ptr(),
            argc,
            args.as_mut_ptr(),
            retval,
            coroutine,
            create_coroutine,
            swoole_rs_zend::host::NO_THEN,
        )
    };
    called.map_err(|bailout| {
        let _relayed = bailout::relay_to_c(Err(bailout));
        Bailout
    })
}

/// The warning of a failed call; a user error handler that bails out ends
/// the event there, as upstream.
#[cold]
fn failed(event: &str) -> Called {
    set_last_error(SW_ERROR_PHP_RUNTIME_NOTICE);
    if crate::server::display_errors() {
        let held = ServerObject::running();
        let object = held.as_deref().and_then(ServerObject::of);
        let name = object.map(crate::lifecycle::class).unwrap_or_default();
        let text = message!("{name}->{event} handler error");
        if report(E_WARNING, text.as_cstr()).is_err() {
            return Called::Bailout;
        }
    }
    Called::Failed
}

/// `\Swoole\Server\Helper::<name>(...args)` when the library is enabled.
#[inline]
fn helper(facts: &Facts, name: &CStr, args: &mut [zval]) -> bool {
    if !facts.enable_library {
        return false;
    }
    let name = [br"\Swoole\Server\Helper::", name.to_bytes()].concat();
    let args: Vec<&Value> = args.iter_mut().map(Value::of).collect();
    swoole_rs_zend::host::named_call(&name, &args).is_err()
}

fn facts(server: view::Server<'_>) -> Facts {
    let config = server.config();
    let gs = server.counters();
    let task_worker = crate::server::worker_type() == Some(crate::server::WorkerType::Task);
    Facts {
        master_pid: i64::from(gs.master_pid.load(core::sync::atomic::Ordering::Relaxed)),
        manager_pid: i64::from(gs.manager_pid.load(core::sync::atomic::Ordering::Relaxed)),
        event_object: config.event_object.get(),
        enable_coroutine: config.enable_coroutine.get(),
        coroutine: coroutine_enabled(server),
        enable_library: swoole_rs_phpswoole::enable_library(),
        task_worker,
    }
}

fn coroutine_enabled(server: view::Server<'_>) -> bool {
    let config = server.config();
    match crate::server::worker_type() {
        Some(crate::server::WorkerType::Task) => config.task_enable_coroutine.get(),
        Some(crate::server::WorkerType::Manager) => false,
        _ => config.enable_coroutine.get(),
    }
}

/// `SWR_PHPSERVER_INSTALL_ALLOCATORS`: the server's message bus, and its receive
/// buffers in base and thread mode, allocate Zend strings.
pub(crate) fn use_zend_strings(server: view::Server<'_>) {
    let allocator = swoole_rs_phpswoole::sw_zend_string_allocator();
    server.set_bus_allocator(false, swoole_rs_phpswoole::zend_string_allocator());
    let config = server.config();
    if config.mode_ != Mode::Process {
        config
            .recv_buffer_allocator
            .store(allocator.cast_mut(), Relaxed);
    }
}

/// onWorkerStart's preparation: a task worker without coroutines unhooks the
/// runtime (not in thread mode), and the worker's bus allocates Zend strings.
fn prepare_worker(server: view::Server<'_>) {
    let config = server.config();
    if crate::server::worker_type() == Some(crate::server::WorkerType::Task)
        && !config.task_enable_coroutine.get()
        && config.mode_ != Mode::Thread
    {
        let _unhooked = swoole_rs_phpruntime::php_coroutine_enable_hook(0);
    }
    server.set_bus_allocator(true, swoole_rs_phpswoole::zend_string_allocator());
}

/// The running server's object zval, as a callback's first argument: a copy
/// that takes no reference.
#[must_use]
pub fn zserv() -> zval {
    crate::instance::server_zval()
}

pub(crate) fn dtor(value: &mut zval) {
    // SAFETY: a value this dispatch owns.
    unsafe { zval_ptr_dtor(value) };
}

/// `php_swoole_server_onReceive`'s arguments after `args[0]`, the server.
/// Returns argc.
fn receive_args(
    server: view::Server<'_>,
    request: &RecvData,
    event_object: bool,
    args: &mut [zval; 4],
) -> usize {
    let info = request.info();
    let (fd, reactor_id) = (info.fd, i64::from(info.reactor_id));
    if event_object {
        let props = Props::init(&mut args[1], &DATA[EVENT]);
        props.long(c"fd", fd);
        props.long(c"reactor_id", reactor_id);
        props.double(c"dispatch_time", info.time);
        props.set(c"data", &php_swoole_get_recv_data(server, request));
        2
    } else {
        args[1] = long_zval(fd);
        args[2] = long_zval(reactor_id);
        args[3] = php_swoole_get_recv_data(server, request).into_zval();
        4
    }
}

/// After the call: the warning and close of a failed call, and the
/// arguments' release. True on a bailout.
fn receive_done(server: view::Server<'_>, succeeded: bool, fd: i64, args: &mut [zval]) -> bool {
    if !succeeded {
        if failed("onReceive") == Called::Bailout {
            return true;
        }
        server_methods::close(server, fd, false);
    }
    if let Some(last) = args.last_mut() {
        dtor(last);
    }
    false
}

/// `php_swoole_server_onConnect`/`onClose`'s call, with the chosen callback.
fn on_connection(
    server: view::Server<'_>,
    cb: &Callable,
    close: bool,
    fd: i64,
    reactor_id: i64,
    time: f64,
) -> bool {
    let facts = facts(server);
    let mut args = [zserv(), undef(), undef()];
    let argc = if facts.event_object {
        let props = Props::init(&mut args[1], &DATA[EVENT]);
        props.long(c"fd", fd);
        props.long(c"reactor_id", reactor_id);
        props.double(c"dispatch_time", time);
        2
    } else {
        args[1] = long_zval(fd);
        args[2] = long_zval(reactor_id);
        3
    };
    let event = if close { "onClose" } else { "onConnect" };
    if call(
        cb,
        args.get_mut(..argc).unwrap_or_default(),
        facts.enable_coroutine,
        event,
    ) == Called::Bailout
    {
        return true;
    }
    if facts.event_object {
        dtor(&mut args[1]);
    }
    false
}

/// `php_swoole_server_onBufferFull`/`onBufferEmpty`'s call.
fn on_buffer(server: view::Server<'_>, full: bool, fd: i64, server_fd: c_int) -> bool {
    let (kind, event) = if full {
        (PortCallback::BufferFull, "onBufferFull")
    } else {
        (PortCallback::BufferEmpty, "onBufferEmpty")
    };
    let Some(cb) = php_swoole_server_get_callback(server, server_fd, kind) else {
        return false;
    };
    let mut args = [zserv(), long_zval(fd)];
    call(&cb, &mut args, false, event) == Called::Bailout
}

/// `php_swoole_server_onPacket`'s call, once the datagram is read.
fn on_packet(server: view::Server<'_>, packet: &Packet<'_>, server_fd: c_int, time: f64) -> bool {
    let facts = facts(server);
    let mut args = [zserv(), undef(), undef()];
    let argc = if facts.event_object {
        let props = Props::init(&mut args[1], &DATA[PACKET]);
        props.long(c"server_socket", i64::from(server_fd));
        props.double(c"dispatch_time", time);
        if packet.server_port >= 0 {
            props.long(c"server_port", packet.server_port);
        }
        if let Some(address) = packet.address {
            props.string(c"address", address);
            if packet.port >= 0 {
                props.long(c"port", packet.port);
            }
        }
        props.bytes(c"data", packet.data);
        2
    } else {
        let mut address = OwnedArray::default();
        address.insert_long(b"server_socket", i64::from(server_fd));
        address.insert_double(b"dispatch_time", time);
        if packet.server_port >= 0 {
            address.insert_long(b"server_port", packet.server_port);
        }
        if let Some(text) = packet.address {
            address.insert_bytes(b"address", Some(text.to_bytes()));
            if packet.port >= 0 {
                address.insert_long(b"port", packet.port);
            }
        }
        args[1] = Owned::string(ZString::new(packet.data)).into_zval();
        args[2] = address.into_owned().into_zval();
        3
    };
    let Some(cb) = php_swoole_server_get_callback(server, server_fd, PortCallback::Packet) else {
        dtor(&mut args[1]);
        if !facts.event_object {
            dtor(&mut args[2]);
        }
        return false;
    };
    // Upstream names onPipeMessage in onPacket's warning.
    if call(
        &cb,
        args.get_mut(..argc).unwrap_or_default(),
        facts.enable_coroutine,
        "onPipeMessage",
    ) == Called::Bailout
    {
        return true;
    }
    dtor(&mut args[1]);
    if !facts.event_object {
        dtor(&mut args[2]);
    }
    false
}

/// `php_swoole_server_onPipeMessage`'s call with the unpacked `data`, which
/// the caller still owns.
fn on_pipe_message(
    server: view::Server<'_>,
    data: &mut zval,
    source_worker_id: i64,
    time: f64,
) -> bool {
    let Some(cb) = crate::lifecycle::callback(ServerCallback::PipeMessage) else {
        return false;
    };
    let facts = facts(server);
    let mut args = [zserv(), undef(), undef()];
    let argc = if facts.event_object {
        let props = Props::init(&mut args[1], &DATA[PIPE_MESSAGE]);
        props.long(c"worker_id", source_worker_id);
        props.long(c"source_worker_id", source_worker_id);
        props.double(c"dispatch_time", time);
        props.value(c"data", data);
        2
    } else {
        args[1] = long_zval(source_worker_id);
        // Borrowed without a reference, as upstream's `args[2] = zresult.value`.
        args[2] = *data;
        3
    };
    if call(
        &cb,
        args.get_mut(..argc).unwrap_or_default(),
        facts.coroutine,
        "onPipeMessage",
    ) == Called::Bailout
    {
        return true;
    }
    if facts.event_object {
        dtor(&mut args[1]);
    }
    false
}

/// What a lifecycle event does: its warning name, the helper it runs first and
/// whether its callback may run in a coroutine.
fn lifecycle(kind: ServerCallback) -> Option<(&'static str, &'static CStr, bool)> {
    Some(match kind {
        ServerCallback::Start => ("onStart", c"onStart", true),
        ServerCallback::ManagerStart => ("onManagerStart", c"onManagerStart", false),
        ServerCallback::ManagerStop => ("onManagerStop", c"onManagerStop", false),
        ServerCallback::BeforeShutdown => ("onBeforeShutdown", c"onBeforeShutdown", true),
        ServerCallback::Shutdown => ("onShutdown", c"onShutdown", false),
        ServerCallback::WorkerStart => ("onWorkerStart", c"onWorkerStart", true),
        ServerCallback::BeforeReload => ("onBeforeReload", c"onBeforeReload", false),
        ServerCallback::AfterReload => ("onAfterReload", c"onAfterReload", false),
        ServerCallback::WorkerStop => ("onWorkerStop", c"onWorkerStop", false),
        ServerCallback::WorkerExit => ("onWorkerExit", c"onWorkerExit", false),
        _ => return None,
    })
}

/// The lifecycle events' bodies (`php_swoole_server_onStart` and friends);
/// `worker_id` is the worker's for the worker events.
fn on_lifecycle(server: view::Server<'_>, kind: ServerCallback, worker_id: i64) -> bool {
    let Some((event, helper_name, coroutine)) = lifecycle(kind) else {
        return false;
    };
    // An event with no server object to hand PHP (the object is being freed,
    // which shuts down a server still running) calls nothing.
    if ServerObject::running().is_none() {
        return false;
    }
    let cb = crate::lifecycle::callback(kind);
    let facts = facts(server);
    let mut args = [zserv(), long_zval(worker_id)];
    {
        // The running server's object, held only for these writes.
        let held = ServerObject::running();
        let props = Props::running(held.as_deref());
        if matches!(
            kind,
            ServerCallback::Start | ServerCallback::ManagerStart | ServerCallback::WorkerStart
        ) {
            props.long(c"master_pid", facts.master_pid);
            props.long(c"manager_pid", facts.manager_pid);
        }
        if kind == ServerCallback::WorkerStart {
            props.long(c"worker_id", worker_id);
            props.bool(c"taskworker", facts.task_worker);
            props.long(c"worker_pid", i64::from(std::process::id()));
        }
    }
    if kind == ServerCallback::WorkerStart {
        prepare_worker(server);
    }
    let argc = if matches!(
        kind,
        ServerCallback::WorkerStart | ServerCallback::WorkerStop | ServerCallback::WorkerExit
    ) {
        2
    } else {
        1
    };
    if helper(
        &facts,
        helper_name,
        args.get_mut(..argc).unwrap_or_default(),
    ) {
        return true;
    }
    let Some(cb) = cb else {
        return false;
    };
    let coroutine = coroutine && facts.coroutine;
    call(
        &cb,
        args.get_mut(..argc).unwrap_or_default(),
        coroutine,
        event,
    ) == Called::Bailout
}

/// `php_swoole_server_onWorkerError`'s body.
fn on_worker_error(
    server: view::Server<'_>,
    worker_id: i64,
    pid: i64,
    status: i64,
    code: i64,
    signal: i64,
) -> bool {
    let cb = crate::lifecycle::callback(ServerCallback::WorkerError);
    let facts = facts(server);
    let mut args = [zserv(), undef(), undef(), undef(), undef()];
    let argc = if facts.event_object {
        let props = Props::init(&mut args[1], &DATA[STATUS_INFO]);
        props.long(c"worker_id", worker_id);
        props.long(c"worker_pid", pid);
        props.long(c"status", status);
        props.long(c"exit_code", code);
        props.long(c"signal", signal);
        2
    } else {
        args[1] = long_zval(worker_id);
        args[2] = long_zval(pid);
        args[3] = long_zval(code);
        args[4] = long_zval(signal);
        5
    };
    if helper(
        &facts,
        c"onWorkerError",
        args.get_mut(..1).unwrap_or_default(),
    ) {
        return true;
    }
    if let Some(cb) = cb
        && call(
            &cb,
            args.get_mut(..argc).unwrap_or_default(),
            false,
            "onWorkerError",
        ) == Called::Bailout
    {
        return true;
    }
    if facts.event_object {
        dtor(&mut args[1]);
    }
    false
}

// The Server's on* slots. Each fn takes the slot's null `ctx`, then the
// member's own arguments, and relays a trapped bailout last: its frame owns
// nothing that needs a drop by then.

/// Raise a bailout the event trapped, from this outermost Rust frame.
fn relay(bailed: bool) {
    if bailed {
        bailout::raise(Err(Bailout));
    }
}

/// `SW_OK` / `SW_ERR`.
const SW_OK: c_int = swoole_rs_corebase::constants::ResultCode::SW_OK as c_int;
const SW_ERR: c_int = swoole_rs_corebase::constants::ResultCode::SW_ERR as c_int;
/// `SocketType`s of a datagram's sender.
const UDP: c_int = 2;
const UDP6: c_int = 4;
const UNIX_DGRAM: c_int = 6;
/// `websocket::STATUS_ACTIVE`.
const WEBSOCKET_ACTIVE: u8 = 3;

/// `RecvData`: the event's head, then its payload. Native code lends each
/// receive slot its request for the call; its fields are private, so safe code
/// can't make one whose payload pointer and length disagree.
pub use swoole_rs_server::reactor_entry::RecvData;

fn port_callback(server: view::Server<'_>, server_fd: u16, kind: PortCallback) -> Option<Callable> {
    php_swoole_server_get_callback(server, c_int::from(server_fd), kind)
}

fn worker_id(worker: Worker<'_>) -> i64 {
    i64::from(worker.id())
}

macro_rules! lifecycle_slots {
    ($($name:ident => $kind:expr;)*) => {$(
        extern "C" fn $name(_: Ctx, server: view::Server<'_>) {
            relay(on_lifecycle(server, $kind, 0));
        }
    )*};
}

lifecycle_slots! {
    on_start => ServerCallback::Start;
    on_before_shutdown => ServerCallback::BeforeShutdown;
    on_shutdown => ServerCallback::Shutdown;
    on_manager_start => ServerCallback::ManagerStart;
    on_manager_stop => ServerCallback::ManagerStop;
    on_before_reload => ServerCallback::BeforeReload;
    on_after_reload => ServerCallback::AfterReload;
}

extern "C" fn on_worker_start(_: Ctx, server: view::Server<'_>, worker: Worker<'_>) {
    relay(on_lifecycle(
        server,
        ServerCallback::WorkerStart,
        worker_id(worker),
    ));
}

extern "C" fn on_worker_stop(_: Ctx, server: view::Server<'_>, worker: Worker<'_>) {
    if swoole_rs_process::methods::worker_is_running() {
        relay(on_lifecycle(
            server,
            ServerCallback::WorkerStop,
            worker_id(worker),
        ));
    }
}

extern "C" fn on_worker_exit(_: Ctx, server: view::Server<'_>, worker: Worker<'_>) {
    relay(on_lifecycle(
        server,
        ServerCallback::WorkerExit,
        worker_id(worker),
    ));
}

extern "C" fn on_worker_error_slot(
    _: Ctx,
    server: view::Server<'_>,
    worker: Worker<'_>,
    exit: &ExitStatus,
) {
    let (pid, status) = (exit.pid(), exit.status());
    let (code, signal) = (libc::WEXITSTATUS(status), libc::WTERMSIG(status));
    let id = worker_id(worker);
    let error = |n: c_int| i64::from(n);
    relay(on_worker_error(
        server,
        id,
        error(pid),
        error(status),
        error(code),
        error(signal),
    ));
}

/// A task event's result, or its bailout raised.
fn task_result(outcome: Result<c_int, Bailout>) -> c_int {
    let result = outcome.unwrap_or_else(|bailed| {
        bailed.defer();
        SW_OK
    });
    // A callable's release can leave one pending behind an `Ok`.
    bailout::finish();
    result
}

extern "C" fn on_task(_: Ctx, server: view::Server<'_>, task: Packed<'_>) -> c_int {
    task_result(crate::task::on_task(server, task))
}

extern "C" fn on_finish(_: Ctx, server: view::Server<'_>, task: Packed<'_>) -> c_int {
    task_result(crate::task::on_finish(server, task))
}

extern "C" fn on_pipe_message_slot(_: Ctx, server: view::Server<'_>, message: Packed<'_>) {
    let (mut data, unpacked, mut bailed) = match crate::task::decode(message) {
        Ok(Some((data, done))) => (data.into_zval(), done, false),
        Ok(None) => (undef(), false, false),
        Err(Bailout) => (undef(), false, true),
    };
    if unpacked {
        let info = message.head();
        // `Server::get_task_src_worker_id`.
        let source = i64::from(info.reactor_id);
        bailed = on_pipe_message(server, &mut data, source, info.time);
    }
    if !bailed {
        dtor(&mut data);
    }
    relay(bailed);
}

/// `php_swoole_server_onReceive`: the port's `onReceive`, if any (and
/// crates/phphttp's fallback).
#[must_use]
pub fn php_swoole_server_on_receive(server: view::Server<'_>, request: &RecvData) -> c_int {
    let info = request.info();
    let Some(cb) = port_callback(server, info.server_fd, PortCallback::Receive) else {
        return SW_OK;
    };
    let config = server.config();
    let mut args = [zserv(), undef(), undef(), undef()];
    let argc = receive_args(server, request, config.event_object.get(), &mut args);
    let called = invoke(
        &cb,
        args.get_mut(..argc).unwrap_or_default(),
        ptr::null_mut(),
        config.enable_coroutine.get(),
    );
    relay(called.map_or(true, |succeeded| {
        receive_done(
            server,
            succeeded,
            info.fd,
            args.get_mut(..argc).unwrap_or_default(),
        )
    }));
    SW_OK
}

extern "C" fn on_receive(_: Ctx, server: view::Server<'_>, request: &RecvData) -> c_int {
    php_swoole_server_on_receive(server, request)
}

/// crates/phphttp's `Swoole\Http\Server` hooks, which crates/ffi registers.
pub struct HttpHooks {
    /// `php_swoole_http_server_onReceive`.
    pub on_receive: fn(view::Server<'_>, &RecvData) -> c_int,
    /// `php_swoole_http_server_onClose`.
    pub on_close: fn(view::Server<'_>, &DataHead),
    /// `swoole_http_server_can_cache_session_addr`.
    pub can_cache_session_addr: fn(view::Server<'_>) -> bool,
}

static HTTP: OnceLock<&'static HttpHooks> = OnceLock::new();

/// Keep crates/phphttp's hooks; the first registration stands.
pub fn register_http(hooks: &'static HttpHooks) {
    HTTP.get_or_init(|| hooks);
}

/// The registered HTTP hooks.
pub(crate) fn http() -> Option<&'static HttpHooks> {
    HTTP.get().copied()
}

extern "C" fn on_http_receive(_: Ctx, server: view::Server<'_>, request: &RecvData) -> c_int {
    http().map_or(SW_OK, |http| (http.on_receive)(server, request))
}

/// `php_swoole_redis_server_onReceive`: Redis ports parse the request in
/// crates/phpredis, others take the plain `onReceive`.
extern "C" fn on_redis_receive(_: Ctx, server: view::Server<'_>, request: &RecvData) -> c_int {
    let fd = request.info().fd;
    let Some(conn) = server.by_session(fd) else {
        log::warning(
            c"php_swoole_redis_server_onReceive",
            format_args!("connection[{fd}] is closed"),
        );
        return SW_ERR;
    };
    if !server
        .port_of_fd(conn.fd())
        .is_some_and(|port| port.open_redis_protocol.get())
    {
        return php_swoole_server_on_receive(server, request);
    }
    let Some(redis) = crate::server::redis_receive() else {
        return php_swoole_server_on_receive(server, request);
    };
    let data = php_swoole_get_recv_data(server, request);
    let result = redis(server, fd, &data, coroutine_enabled(server));
    // The relay's longjmp would skip `data`'s release.
    drop(data);
    relay(result.is_err());
    result.unwrap_or(SW_ERR)
}

extern "C" fn on_packet_slot(_: Ctx, server: view::Server<'_>, request: &RecvData) -> c_int {
    let Some((socket_type, socket_addr, data)) = request.datagram() else {
        return SW_ERR;
    };
    let info = request.info();
    // The sender, of the packet's socket type: an IP address and port, or a path.
    let sender = SocketAddress {
        kind: socket_type,
        ..socket_addr
    };
    let port = if matches!(socket_type, UDP | UDP6) {
        i64::from(swr_v1_clients_address_port(&sender))
    } else {
        -1
    };
    with_host(&sender, |address| {
        let sender = Packet {
            data,
            address: matches!(socket_type, UDP | UDP6 | UNIX_DGRAM).then_some(address),
            port,
            server_port: server.at(c_int::from(info.server_fd)).map_or(-1, |sock| {
                i64::from(swr_v1_clients_address_port(&sock.info()))
            }),
        };
        relay(on_packet(
            server,
            &sender,
            c_int::from(info.server_fd),
            info.time,
        ));
    });
    SW_OK
}

extern "C" fn on_connect(_: Ctx, server: view::Server<'_>, info: &DataHead) {
    if let Some(cb) = port_callback(server, info.server_fd, PortCallback::Connect) {
        let reactor_id = i64::from(info.reactor_id);
        relay(on_connection(
            server, &cb, false, info.fd, reactor_id, info.time,
        ));
    }
}

/// Wake the coroutines `send_yield` parked on a session: resume them when
/// its output drained, or cancel them when it closed (each leaves the queue
/// as it wakes).
fn send_wake(session_id: i64, cancel: bool) {
    // Nothing to drop: a woken coroutine may bail out across this frame.
    let Some(waiters) = crate::lifecycle::send_waiters(session_id) else {
        return;
    };
    while let Some(co) = crate::lifecycle::send_next(&waiters, !cancel) {
        // SAFETY: a coroutine parked in `send_yield`, until it wakes.
        unsafe { co.wake(cancel) };
    }
    drop(ManuallyDrop::into_inner(waiters));
}

/// `php_swoole_server_onClose` (crates/phphttp's fallback).
pub fn php_swoole_server_on_close(server: view::Server<'_>, info: &DataHead) {
    close(server, info);
}

/// `php_swoole_server_onClose`: wake the session's senders, then `onClose`
/// (`onDisconnect` for a WebSocket port's connection that never opened).
fn close(server: view::Server<'_>, info: &DataHead) {
    let session_id = info.fd;
    let config = server.config();
    if config.enable_coroutine.get() && config.send_yield.get() {
        send_wake(session_id, true);
    }
    let mut cb = port_callback(server, info.server_fd, PortCallback::Close);
    let Some(conn) = server.by_session(session_id) else {
        return;
    };
    let websocket_status = conn.websocket_status();
    if websocket_status != WEBSOCKET_ACTIVE {
        let port = server.port_by_server_fd(c_int::from(info.server_fd));
        let websocket = port.is_some_and(|port| port.config().open_websocket_protocol.get());
        if websocket
            && crate::instance::php_swoole_server_isset_callback(
                server,
                port,
                PortCallback::Disconnect,
            )
        {
            cb = port_callback(server, info.server_fd, PortCallback::Disconnect);
        }
    }
    let reactor_id = i64::from(info.reactor_id);
    if let Some(cb) = cb {
        relay(on_connection(
            server, &cb, true, session_id, reactor_id, info.time,
        ));
    }
    // The callback may have changed it.
    if conn.http2_stream()
        && let Some(families) = swoole_rs_phpswoole::families()
    {
        (families.http.http2_on_close)(conn.session_id());
    }
}

extern "C" fn on_close(_: Ctx, server: view::Server<'_>, info: &DataHead) {
    close(server, info);
}

extern "C" fn on_http_close(_: Ctx, server: view::Server<'_>, info: &DataHead) {
    if let Some(http) = http() {
        (http.on_close)(server, info);
    }
}

extern "C" fn on_buffer_full(_: Ctx, server: view::Server<'_>, info: &DataHead) {
    relay(on_buffer(
        server,
        true,
        info.fd,
        c_int::from(info.server_fd),
    ));
}

extern "C" fn on_buffer_empty(_: Ctx, server: view::Server<'_>, info: &DataHead) {
    if server.config().send_yield.get() {
        send_wake(info.fd, false);
    }
    relay(on_buffer(
        server,
        false,
        info.fd,
        c_int::from(info.server_fd),
    ));
}

/// `php_swoole_server_onUserWorkerStart`: run the user process's callable in
/// the worker `addProcess` added. The worker is a [`Worker`] handle: starting the
/// process writes its pid and pipe, so no reference to the record may live here.
extern "C" fn on_user_worker_start(_: Ctx, server: view::Server<'_>, worker: Worker<'_>) {
    let id = i64::from(worker.id());
    let config = server.config();
    let process = if config.mode_ == Mode::Thread {
        let index =
            id - i64::from(config.worker_num.get()) - i64::from(config.task_worker_num.get());
        let index = usize::try_from(index).unwrap_or(usize::MAX);
        let process = ServerObject::with_running(|object| object.user_process(index));
        let process = process.unwrap_or(ptr::null_mut());
        // A user worker: just the message bus allocator.
        prepare_worker(server);
        process
    } else {
        // The worker `addProcess` made holds its process object.
        worker.user_data().cast::<zval>()
    };
    // SAFETY: the process object, which the worker or the Server object holds.
    let process = unsafe { Value::from_ptr(process) };
    let props =
        process.and_then(|process| process.props(Some(swoole_rs_phpprocess::process_class())));
    if let Some(props) = props {
        props.long(c"id", id);
    }
    let gs = server.counters();
    {
        // The running server's object, held only for these writes.
        let held = ServerObject::running();
        let props = Props::running(held.as_deref());
        props.long(
            c"master_pid",
            i64::from(gs.master_pid.load(core::sync::atomic::Ordering::Relaxed)),
        );
        props.long(
            c"manager_pid",
            i64::from(gs.manager_pid.load(core::sync::atomic::Ordering::Relaxed)),
        );
    }
    if let Some(process) = process {
        // This frame owns nothing that needs a drop, as the process exits through a
        // bailout.
        swoole_rs_phpprocess::start_worker(worker, process);
    }
}

/// `SWR_PORTS_HOOK_*` of a port callback just set: install the Server's slot
/// unless one is set.
pub(crate) fn port_hook(server: view::Server<'_>, hook: u32) {
    let callback = match hook {
        1 => Callback::Connect(on_connect),
        2 => Callback::Packet(on_packet_slot),
        3 => Callback::Close(on_close),
        4 => Callback::BufferFull(on_buffer_full),
        5 => Callback::BufferEmpty(on_buffer_empty),
        _ => return,
    };
    if !server.has_callback(callback.slot()) {
        server.set_callback(callback);
    }
}

/// The slots an [`Install`] group fills.
pub(crate) fn install(server: view::Server<'_>, which: Install) {
    let slots: &[Callback] = match which {
        Install::Control => &[
            Callback::Start(on_start),
            Callback::BeforeShutdown(on_before_shutdown),
            Callback::Shutdown(on_shutdown),
            Callback::WorkerStart(on_worker_start),
            Callback::WorkerStop(on_worker_stop),
            Callback::WorkerExit(on_worker_exit),
            Callback::BeforeReload(on_before_reload),
            Callback::AfterReload(on_after_reload),
            Callback::ManagerStart(on_manager_start),
            Callback::ManagerStop(on_manager_stop),
            Callback::WorkerError(on_worker_error_slot),
        ],
        Install::Task => &[Callback::Task(on_task), Callback::Finish(on_finish)],
        Install::PipeMessage => &[Callback::PipeMessage(on_pipe_message_slot)],
        Install::BufferEmpty => &[Callback::BufferEmpty(on_buffer_empty)],
        Install::Close => &[Callback::Close(on_close)],
        Install::HttpClose => &[Callback::Close(on_http_close)],
        Install::Receive => &[Callback::Receive(on_receive)],
        Install::HttpReceive => &[Callback::Receive(on_http_receive)],
        Install::RedisReceive => &[Callback::Receive(on_redis_receive)],
        Install::UserWorkerStart if server.has_callback(CallbackKind::UserWorkerStart) => &[],
        Install::UserWorkerStart => &[Callback::UserWorkerStart(on_user_worker_start)],
        Install::Allocators | Install::AdminCommands => return,
    };
    for &callback in slots {
        server.set_callback(callback);
    }
}

/// The `dispatch_func` setting: `callable`, which the config's slot holds until
/// `release_dispatch_func`, picks the worker of each event from now on, in the
/// reactor's one thread; the previous one is released.
/// # Errors
/// The previous one's destructor bailed out.
pub(crate) fn set_dispatch_func(
    server: view::Server<'_>,
    callable: Callable,
) -> Result<(), Bailout> {
    let config = server.config();
    let owner: Box<dyn Any> = Box::new(callable);
    let previous = config.private_data_3.replace(Some(owner));
    server.set_callback(Callback::DispatchFunc(on_dispatch));
    config.single_thread.set(true);
    release_dispatch_func(previous)
}

/// Drop a `dispatch_func` callable the config's slot gave back, if any.
/// # Errors
/// A destructor bailed out.
pub(crate) fn release_dispatch_func(owner: Option<Arc<Box<dyn Any>>>) -> Result<(), Bailout> {
    if owner.is_none() {
        return Ok(());
    }
    drop(owner);
    bailout::outcome(())
}

/// `sw_zend_call_function_ex(nullptr, cache, args, retval)` under
/// `HOOK_PHP_CALL_STACK`: the reactor thread's stack is not the VM's.
fn call_unlimited(
    cache: *mut zend_fcall_info_cache,
    args: &mut [zval],
    retval: *mut zval,
) -> Result<bool, Bailout> {
    #[expect(clippy::cast_possible_truncation, reason = "at most four arguments")]
    let argc = args.len() as u32;
    swoole_rs_zend::with_unlimited_stack(|| {
        // SAFETY: a resolved cache, initialised arguments and a return slot.
        unsafe {
            function_call(
                cache,
                argc,
                args.as_mut_ptr(),
                retval,
                false,
                create_coroutine,
                swoole_rs_zend::host::NO_THEN,
            )
        }
    })
}

/// `php_swoole_server_dispatch_func`: `dispatch_func($server, $fd, $type[, $data])`'s
/// worker id, or -1.
extern "C" fn on_dispatch(
    _: Ctx,
    server: view::Server<'_>,
    conn: Option<&ConnectionRecord>,
    send: Option<&RecvData>,
) -> c_int {
    let config = server.config();
    // Its own reference for the call: the `Rc` inside is only read, so a thread-mode
    // worker leaves the master's count alone.
    let owner = config.private_data_3.get();
    let Some(dispatch) = owner
        .as_deref()
        .and_then(|owner| owner.downcast_ref::<Callable>())
    else {
        return -1;
    };
    let cache = dispatch.cache_ptr();
    // Its live object, copied without a reference as upstream.
    let zserv = zserv();
    let session = match conn {
        Some(conn) => conn.session_id,
        None => send.map_or(0, |send| send.info().fd),
    };
    let kind = send.map_or(ServerEvent::Close.code(), |send| send.info().kind);
    let mut args = [
        zserv,
        long_zval(session),
        long_zval(i64::from(kind)),
        undef(),
    ];
    let mut argc = 3;
    if let Some(send) = send.filter(|_| dispatch.max_num_args() > 3) {
        let payload = send.payload();
        let bytes = payload.get(..SW_IPC_BUFFER_SIZE).unwrap_or(payload);
        args[3] = Owned::string(ZString::new(bytes)).into_zval();
        argc = 4;
    }
    let retval = Owned::default();
    let mut worker_id = -1;
    match call_unlimited(
        cache,
        args.get_mut(..argc).unwrap_or_default(),
        retval.as_ptr(),
    ) {
        Ok(false) => {
            if failed("onDispatch") == Called::Bailout {
                Bailout.defer();
            }
        }
        Ok(true) if !retval.is_null() => {
            let id = retval.long();
            if id >= i64::from(config.worker_num.get()) {
                let text = message!("invalid target worker-id[{id}]");
                crate::server::fatal(E_WARNING, text.as_cstr());
            } else {
                worker_id = id;
            }
        }
        Ok(true) => {}
        Err(_) => Bailout.defer(),
    }
    // Released first: `finish` may jump over this frame.
    drop(retval);
    if argc == 4 {
        dtor(&mut args[3]);
    }
    drop(owner);
    bailout::finish();
    c_int::try_from(worker_id).unwrap_or(-1)
}

/// `php_swoole_server_length_func`: a port's `package_length_func($data)`.
pub(crate) struct Length;

impl swoole_rs_phpswoole::length::PackageLength for Length {
    /// A thread-mode worker reads the master's slot and leaves its count alone.
    fn owns() -> bool {
        crate::lifecycle::owns_shared()
    }

    fn length(callable: &callable::Callable, bytes: &[u8]) -> isize {
        let mut data = Owned::string(ZString::new(bytes)).into_zval();
        let retval = Owned::default();
        let mut result = -1;
        let args = std::slice::from_mut(&mut data);
        match call_unlimited(callable.cache_ptr(), args, retval.as_ptr()) {
            Ok(false) => crate::server::fatal(E_WARNING, c"length function handler error"),
            Ok(true) => result = isize::try_from(retval.long()).unwrap_or(-1),
            Err(_) => Bailout.defer(),
        }
        drop(retval);
        dtor(&mut data);
        result
    }
}

/// The "`<class>->on<event> handler error`" warning of a failed call.
pub(crate) fn handler_error(event: &str) -> Result<(), Bailout> {
    if failed(event) == Called::Bailout {
        return Err(Bailout);
    }
    Ok(())
}

pub(crate) use crate::server::set_last_error;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_lifecycle_event_is_named() {
        for kind in [
            ServerCallback::Start,
            ServerCallback::ManagerStart,
            ServerCallback::ManagerStop,
            ServerCallback::BeforeShutdown,
            ServerCallback::Shutdown,
            ServerCallback::WorkerStart,
            ServerCallback::BeforeReload,
            ServerCallback::AfterReload,
            ServerCallback::WorkerStop,
            ServerCallback::WorkerExit,
        ] {
            let (event, helper, _) = lifecycle(kind).unwrap();
            assert_eq!(event.as_bytes(), helper.to_bytes());
        }
        assert!(lifecycle(ServerCallback::WorkerError).is_none());
        assert!(lifecycle(ServerCallback::PipeMessage).is_none());
    }
}
