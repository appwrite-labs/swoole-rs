//! Server events into PHP callbacks: the arguments (`Swoole\Server\Event` and
//! friends, or plain values), the call and its "handler error" warning. The
//! Server's on* slots hold this module's `extern "C"` fns (stored with a null
//! `ctx`, see `swoole_rs_server::callbacks`); each re-raises a bailout it
//! trapped last, from its outermost frame (docs/zend.md).
use crate::lifecycle::{Props, ServerObject};
use crate::server::DATA;
use std::{
    ffi::{CStr, c_char, c_int, c_void},
    ptr,
};
use swoole_rs_clients::address::{swr_v1_clients_address_port, swr_v1_clients_address_string};
use swoole_rs_corebase::{DataHead, log, net_types::SocketAddress};
use swoole_rs_corebase::{constants::SW_IPC_BUFFER_SIZE, net_types::Protocol};
use swoole_rs_phpcoroutines::create_coroutine;
use swoole_rs_process::task_file::Packed;
use swoole_rs_process::worker_record::Worker;
use swoole_rs_process::worker_record::WorkerRecord;
use swoole_rs_server::ConnectionRecord;
use swoole_rs_server::callbacks::{
    CB_AFTER_RELOAD, CB_BEFORE_RELOAD, CB_BEFORE_SHUTDOWN, CB_BUFFER_EMPTY, CB_BUFFER_FULL,
    CB_CLOSE, CB_CONNECT, CB_DISPATCH_FUNC, CB_FINISH, CB_MANAGER_START, CB_MANAGER_STOP,
    CB_PACKET, CB_PIPE_MESSAGE, CB_RECEIVE, CB_SHUTDOWN, CB_START, CB_TASK, CB_USER_WORKER_START,
    CB_WORKER_ERROR, CB_WORKER_EXIT, CB_WORKER_START, CB_WORKER_STOP,
};
use swoole_rs_zend::host::{callable, function_call};
use swoole_rs_zend::{
    E_WARNING, OwnedArray,
    bailout::{self, Bailout},
    class_name, long_zval, message, object_of, report, string,
    sys::{
        IS_NULL, ZEND_ACC_VARIADIC, swr_zend_callable_warning, swr_zend_stack_restore,
        swr_zend_stack_unlimit, zend_fcall_info_cache, zval, zval_ptr_dtor,
    },
    value::{get_long, set_str},
};

/// `SW_ERROR_PHP_RUNTIME_NOTICE`.
const SW_ERROR_PHP_RUNTIME_NOTICE: c_int = 510;

use crate::instance::{
    php_swoole_get_recv_data, php_swoole_server_get_callback, php_swoole_server_zval_ptr,
};
use swoole_rs_server::{server_methods, view};

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

/// `SW_SERVER_CB_*` (server-level callbacks).
const ON_START: c_int = 0;
const ON_BEFORE_SHUTDOWN: c_int = 1;
const ON_SHUTDOWN: c_int = 2;
const ON_WORKER_START: c_int = 3;
const ON_WORKER_STOP: c_int = 4;
const ON_BEFORE_RELOAD: c_int = 5;
const ON_AFTER_RELOAD: c_int = 6;
const ON_WORKER_EXIT: c_int = 9;
const ON_WORKER_ERROR: c_int = 10;
const ON_MANAGER_START: c_int = 11;
const ON_MANAGER_STOP: c_int = 12;
const ON_PIPE_MESSAGE: c_int = 13;
/// Port-level callbacks.
const ON_PACKET: c_int = 3;
const ON_BUFFER_FULL: c_int = 10;
const ON_BUFFER_EMPTY: c_int = 11;

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
fn call(cb: *mut c_void, args: &mut [zval], coroutine: bool, event: &str) -> Called {
    match invoke(cb, args, ptr::null_mut(), coroutine) {
        Ok(true) => Called::Done,
        Err(_) => Called::Bailout,
        Ok(false) => args
            .first()
            .map_or(Called::Failed, |server| failed(server, event)),
    }
}

/// `zend::function::call(cb, argc, args, retval, coroutine)`: whether the
/// callable ran (or its coroutine started), or the bailout it raised, which
/// the caller re-raises or hands to C++ (it is no longer pending here).
pub(crate) fn invoke(
    cb: *mut c_void,
    args: &mut [zval],
    retval: *mut zval,
    coroutine: bool,
) -> Result<bool, Bailout> {
    #[expect(clippy::cast_possible_truncation, reason = "at most five arguments")]
    let argc = args.len() as u32;
    // SAFETY: `cb` is a live callable handle.
    let cache = unsafe { callable::cache(cb.cast()) };
    // SAFETY: a resolved cache, `argc` initialised arguments and the return
    // slot or null; the coroutine leaf and the reporter run inside C traps.
    let called = unsafe {
        function_call(
            cache,
            argc,
            args.as_mut_ptr(),
            retval,
            coroutine,
            create_coroutine,
            swr_zend_callable_warning,
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
fn failed(server: &zval, event: &str) -> Called {
    set_last_error(SW_ERROR_PHP_RUNTIME_NOTICE);
    if crate::server::display_errors() {
        // SAFETY: the server object.
        let object = unsafe { object_of(server) };
        // SAFETY: a live object.
        let name = unsafe { class_name(object) };
        let text = message!("{}->{event} handler error", name.to_string_lossy());
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
    // SAFETY: initialised arguments.
    unsafe { swoole_rs_zend::host::named_call(&name, args) }.is_err()
}

fn facts(server: view::Server<'_>) -> Facts {
    let config = server.config();
    let gs = server.counters();
    let task_worker = crate::server::worker_type() == crate::server::TASK_WORKER;
    Facts {
        master_pid: i64::from(gs.master_pid),
        manager_pid: i64::from(gs.manager_pid),
        event_object: config.event_object,
        enable_coroutine: config.enable_coroutine,
        coroutine: coroutine_enabled(server),
        enable_library: swoole_rs_phpswoole::enable_library(),
        task_worker,
    }
}

fn coroutine_enabled(server: view::Server<'_>) -> bool {
    let config = server.config();
    match crate::server::worker_type() {
        crate::server::TASK_WORKER => config.task_enable_coroutine,
        crate::server::MANAGER => false,
        _ => config.enable_coroutine,
    }
}

/// `serv->get_worker_message_bus()`: this thread's in thread mode, else the server's.
pub(crate) fn message_bus(server: view::Server<'_>) -> *mut c_void {
    if i64::from(server.config().mode_) == crate::lifecycle::MODE_THREAD {
        swoole_rs_corebase::thread_message_bus()
    } else {
        server.message_bus()
    }
}

/// `set_allocator(sw_zend_string_allocator())` on `bus`, a `MessageBus`: its Rust
/// bus, then its allocator.
fn bus_uses_zend_strings(bus: *mut c_void) {
    let allocator = swoole_rs_phpswoole::sw_zend_string_allocator();
    // SAFETY: a live `MessageBus`, whose allocator only this thread reads meanwhile.
    unsafe { (*bus.cast::<[*const c_void; 2]>())[1] = allocator };
}

/// `SWR_PHPSERVER_INSTALL_ALLOCATORS`: the server's message bus, and its receive
/// buffers in base and thread mode, allocate Zend strings.
pub(crate) fn use_zend_strings(server: view::Server<'_>) {
    bus_uses_zend_strings(server.message_bus());
    // SAFETY: the server is configured on this thread, with no other reference to the field.
    let config = unsafe { server.config_mut() };
    if i64::from(config.mode_) != crate::lifecycle::MODE_PROCESS {
        config.recv_buffer_allocator = swoole_rs_phpswoole::sw_zend_string_allocator();
    }
}

/// onWorkerStart's preparation: a task worker without coroutines unhooks the
/// runtime (not in thread mode), and the worker's bus allocates Zend strings.
fn prepare_worker(server: view::Server<'_>) {
    let config = server.config();
    if crate::server::worker_type() == crate::server::TASK_WORKER
        && !config.task_enable_coroutine
        && i64::from(config.mode_) != crate::lifecycle::MODE_THREAD
    {
        let _unhooked = swoole_rs_phpruntime::php_coroutine_enable_hook(0);
    }
    bus_uses_zend_strings(message_bus(server));
}

pub(crate) fn zserv() -> zval {
    let zserv = php_swoole_server_zval_ptr();
    // SAFETY: the server's live object zval; copied without a reference, as
    // upstream's `args[0] = *zserv`.
    unsafe { *zserv }
}

fn dtor(value: &mut zval) {
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
    let info = &request.info;
    let (fd, reactor_id) = (info.fd, i64::from(info.reactor_id));
    if event_object {
        let props = Props::init(&mut args[1], &DATA[EVENT]);
        props.long(c"fd", fd);
        props.long(c"reactor_id", reactor_id);
        props.double(c"dispatch_time", info.time);
        let mut data = undef();
        // SAFETY: `data` receives the payload.
        unsafe { php_swoole_get_recv_data(server, &raw mut data, request) };
        props.value(c"data", &mut data);
        dtor(&mut data);
        2
    } else {
        args[1] = long_zval(fd);
        args[2] = long_zval(reactor_id);
        // SAFETY: as above.
        unsafe { php_swoole_get_recv_data(server, &raw mut args[3], request) };
        4
    }
}

/// After the call: the warning and close of a failed call, and the
/// arguments' release. True on a bailout.
fn receive_done(server: *mut c_void, succeeded: bool, fd: i64, args: &mut [zval]) -> bool {
    if !succeeded {
        if args
            .first()
            .is_some_and(|server| failed(server, "onReceive") == Called::Bailout)
        {
            return true;
        }
        // SAFETY: a live server.
        server_methods::close(unsafe { view::Server::from_raw(server) }, fd, false);
    }
    if let Some(last) = args.last_mut() {
        dtor(last);
    }
    false
}

/// `php_swoole_server_onConnect`/`onClose`'s call, with the chosen callback.
fn on_connection(
    server: view::Server<'_>,
    cb: *mut c_void,
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
        (ON_BUFFER_FULL, "onBufferFull")
    } else {
        (ON_BUFFER_EMPTY, "onBufferEmpty")
    };
    let cb = php_swoole_server_get_callback(server, server_fd, kind);
    if cb.is_null() {
        return false;
    }
    let mut args = [zserv(), long_zval(fd)];
    call(cb, &mut args, false, event) == Called::Bailout
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
        // SAFETY: the datagram's bytes, copied into a new string.
        unsafe {
            swoole_rs_zend::sys::swr_zval_stringl(
                &raw mut args[1],
                packet.data.as_ptr().cast(),
                packet.data.len(),
            );
        };
        args[2] = address.into_owned().into_zval();
        3
    };
    let cb = php_swoole_server_get_callback(server, server_fd, ON_PACKET);
    // Upstream names onPipeMessage in onPacket's warning.
    if call(
        cb,
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
    let cb = crate::lifecycle::callback(ON_PIPE_MESSAGE);
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
        cb,
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
fn lifecycle(kind: c_int) -> Option<(&'static str, &'static CStr, bool)> {
    Some(match kind {
        ON_START => ("onStart", c"onStart", true),
        ON_MANAGER_START => ("onManagerStart", c"onManagerStart", false),
        ON_MANAGER_STOP => ("onManagerStop", c"onManagerStop", false),
        ON_BEFORE_SHUTDOWN => ("onBeforeShutdown", c"onBeforeShutdown", true),
        ON_SHUTDOWN => ("onShutdown", c"onShutdown", false),
        ON_WORKER_START => ("onWorkerStart", c"onWorkerStart", true),
        ON_BEFORE_RELOAD => ("onBeforeReload", c"onBeforeReload", false),
        ON_AFTER_RELOAD => ("onAfterReload", c"onAfterReload", false),
        ON_WORKER_STOP => ("onWorkerStop", c"onWorkerStop", false),
        ON_WORKER_EXIT => ("onWorkerExit", c"onWorkerExit", false),
        _ => return None,
    })
}

/// The lifecycle events' bodies (`php_swoole_server_onStart` and friends);
/// `worker_id` is the worker's for the worker events.
fn on_lifecycle(server: view::Server<'_>, kind: c_int, worker_id: i64) -> bool {
    let Some((event, helper_name, coroutine)) = lifecycle(kind) else {
        return false;
    };
    let cb = crate::lifecycle::callback(kind);
    let facts = facts(server);
    let mut args = [zserv(), long_zval(worker_id)];
    let props = Props::server(ServerObject::running());
    if matches!(kind, ON_START | ON_MANAGER_START | ON_WORKER_START) {
        props.long(c"master_pid", facts.master_pid);
        props.long(c"manager_pid", facts.manager_pid);
    }
    if kind == ON_WORKER_START {
        props.long(c"worker_id", worker_id);
        props.bool(c"taskworker", facts.task_worker);
        props.long(c"worker_pid", i64::from(std::process::id()));
        prepare_worker(server);
    }
    let argc = if matches!(kind, ON_WORKER_START | ON_WORKER_STOP | ON_WORKER_EXIT) {
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
    if cb.is_null() {
        return false;
    }
    let coroutine = coroutine && facts.coroutine;
    call(
        cb,
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
    let cb = crate::lifecycle::callback(ON_WORKER_ERROR);
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
    if !cb.is_null()
        && call(
            cb,
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
const SW_OK: c_int = 0;
const SW_ERR: c_int = -1;
/// Port-level callbacks the slots choose.
const ON_CONNECT: c_int = 0;
const ON_RECEIVE: c_int = 1;
const ON_CLOSE: c_int = 2;
const ON_DISCONNECT: c_int = 9;
/// `SocketType`s of a datagram's sender.
const UDP: c_int = 2;
const UDP6: c_int = 4;
const UNIX_DGRAM: c_int = 6;
/// `websocket::STATUS_ACTIVE`.
const WEBSOCKET_ACTIVE: u8 = 3;

/// `RecvData`: the event's head, then its payload. Native code lends each
/// receive slot its request for the call; private fields, so safe code can't
/// make one whose payload pointer and length disagree.
#[repr(C)]
pub struct RecvData {
    pub(crate) info: DataHead,
    pub(crate) data: *const c_char,
}

impl RecvData {
    /// The event's head.
    #[must_use]
    pub fn info(&self) -> &DataHead {
        &self.info
    }
}

/// `DgramPacket`'s head; the datagram follows.
#[repr(C)]
struct DgramPacket {
    socket_type: c_int,
    socket_addr: SocketAddress,
    length: u32,
}

/// `ExitStatus`.
#[repr(C)]
struct ExitStatus {
    pid: c_int,
    status: c_int,
}

type Ctx = *mut c_void;

use swoole_rs_phpprocess::swr_v1_phpprocess_start;

fn port_callback(server: view::Server<'_>, server_fd: u16, kind: c_int) -> *mut c_void {
    php_swoole_server_get_callback(server, c_int::from(server_fd), kind)
}

fn worker_id(worker: &WorkerRecord) -> i64 {
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
    on_start => ON_START;
    on_before_shutdown => ON_BEFORE_SHUTDOWN;
    on_shutdown => ON_SHUTDOWN;
    on_manager_start => ON_MANAGER_START;
    on_manager_stop => ON_MANAGER_STOP;
    on_before_reload => ON_BEFORE_RELOAD;
    on_after_reload => ON_AFTER_RELOAD;
}

extern "C" fn on_worker_start(_: Ctx, server: view::Server<'_>, worker: &WorkerRecord) {
    relay(on_lifecycle(server, ON_WORKER_START, worker_id(worker)));
}

extern "C" fn on_worker_stop(_: Ctx, server: view::Server<'_>, worker: &WorkerRecord) {
    if swoole_rs_process::methods::worker_is_running() {
        relay(on_lifecycle(server, ON_WORKER_STOP, worker_id(worker)));
    }
}

extern "C" fn on_worker_exit(_: Ctx, server: view::Server<'_>, worker: &WorkerRecord) {
    relay(on_lifecycle(server, ON_WORKER_EXIT, worker_id(worker)));
}

extern "C" fn on_worker_error_slot(
    _: Ctx,
    server: view::Server<'_>,
    worker: &WorkerRecord,
    exit: &ExitStatus,
) {
    let &ExitStatus { pid, status } = exit;
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
    let mut data = undef();
    let (unpacked, mut bailed) = match crate::task::decode(message, &raw mut data) {
        Ok(unpacked) => (unpacked, false),
        Err(Bailout) => (false, true),
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
    let info = &request.info;
    let cb = port_callback(server, info.server_fd, ON_RECEIVE);
    if cb.is_null() {
        return SW_OK;
    }
    let config = server.config();
    let mut args = [zserv(), undef(), undef(), undef()];
    let argc = receive_args(server, request, config.event_object, &mut args);
    let called = invoke(
        cb,
        args.get_mut(..argc).unwrap_or_default(),
        ptr::null_mut(),
        config.enable_coroutine,
    );
    relay(called.map_or(true, |succeeded| {
        receive_done(
            server.raw(),
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

extern "C" fn on_http_receive(_: Ctx, server: view::Server<'_>, request: &RecvData) -> c_int {
    let Some(families) = swoole_rs_phpswoole::families() else {
        return SW_OK;
    };
    let request = ptr::from_ref(request).cast_mut().cast();
    // SAFETY: the Server's own event.
    unsafe { (families.http.on_receive)(server.raw(), request) }
}

/// `php_swoole_redis_server_onReceive`: Redis ports parse the request in
/// crates/phpredis, others take the plain `onReceive`.
extern "C" fn on_redis_receive(_: Ctx, server: view::Server<'_>, request: &RecvData) -> c_int {
    let fd = request.info.fd;
    let Some(conn) = server.by_session(fd) else {
        log::warning(
            c"php_swoole_redis_server_onReceive",
            format_args!("connection[{fd}] is closed"),
        );
        return SW_ERR;
    };
    let port = server.port_by_fd(conn.fd);
    // SAFETY: null, or the connection's live port.
    if port.is_null() || !unsafe { view::port(port) }.open_redis_protocol {
        return php_swoole_server_on_receive(server, request);
    }
    let Some(redis) = crate::server::redis_receive() else {
        return php_swoole_server_on_receive(server, request);
    };
    let data = swoole_rs_zend::Owned::default();
    // SAFETY: the live server and its received request; `data` takes the payload.
    unsafe {
        php_swoole_get_recv_data(server, data.as_ptr(), request);
    }
    let result = redis(server, fd, &data, coroutine_enabled(server));
    relay(result.is_err());
    result.unwrap_or(SW_ERR)
}

extern "C" fn on_packet_slot(_: Ctx, server: view::Server<'_>, request: &RecvData) -> c_int {
    let packet = request.data;
    #[expect(clippy::cast_ptr_alignment, reason = "read unaligned")]
    let dgram = packet.cast::<DgramPacket>();
    // SAFETY: an onPacket event's payload is its `DgramPacket`.
    let DgramPacket {
        socket_type,
        socket_addr,
        length,
    } = unsafe { dgram.read_unaligned() };
    let info = &request.info;
    let addr = &socket_addr.addr;
    let text = |family, ip: *const c_void| {
        // SAFETY: the address of `family` in the packet's sender.
        unsafe { swr_v1_clients_address_string(family, ip) }
    };
    let (address, port) = match socket_type {
        UDP => {
            // SAFETY: a UDP packet's sender is IPv4.
            let sin = unsafe { addr.inet_v4 };
            let address = text(libc::AF_INET, (&raw const sin.sin_addr).cast());
            (address, i64::from(u16::from_be(sin.sin_port)))
        }
        UDP6 => {
            // SAFETY: a UDP6 packet's sender is IPv6.
            let sin6 = unsafe { addr.inet_v6 };
            let address = text(libc::AF_INET6, (&raw const sin6.sin6_addr).cast());
            (address, i64::from(u16::from_be(sin6.sin6_port)))
        }
        // SAFETY: a unix datagram's sender is a path, in this frame's copy.
        UNIX_DGRAM => (unsafe { (&raw const addr.un.sun_path) }.cast(), -1),
        _ => (ptr::null(), -1),
    };
    let data = packet.wrapping_add(size_of::<DgramPacket>()).cast::<u8>();
    let sender = Packet {
        // SAFETY: the datagram's `length` bytes follow its head.
        data: unsafe { std::slice::from_raw_parts(data, length as usize) },
        // SAFETY: null, or the sender's NUL-terminated text (this thread's
        // scratch or this frame's copy), unchanged until the call returns.
        address: (!address.is_null()).then(|| unsafe { CStr::from_ptr(address) }),
        port,
        server_port: server.at(c_int::from(info.server_fd)).map_or(-1, |sock| {
            i64::from(swr_v1_clients_address_port(&sock.info))
        }),
    };
    relay(on_packet(
        server,
        &sender,
        c_int::from(info.server_fd),
        info.time,
    ));
    SW_OK
}

extern "C" fn on_connect(_: Ctx, server: view::Server<'_>, info: &DataHead) {
    let cb = port_callback(server, info.server_fd, ON_CONNECT);
    if !cb.is_null() {
        let reactor_id = i64::from(info.reactor_id);
        relay(on_connection(
            server, cb, false, info.fd, reactor_id, info.time,
        ));
    }
}

/// Wake the coroutines `send_yield` parked on a session: resume them when
/// its output drained, or cancel them when it closed (each leaves the queue
/// as it wakes).
fn send_wake(session_id: i64, cancel: bool) {
    // Raw handles only: a woken coroutine may bail out across this frame.
    let waiters = crate::lifecycle::send_waiters(session_id);
    if waiters.is_null() {
        return;
    }
    loop {
        let co = crate::lifecycle::send_next(waiters, !cancel);
        if co.is_null() {
            break;
        }
        // SAFETY: a coroutine parked in `send_yield`, until it wakes.
        unsafe { swoole_rs_phpcoroutines::wake(co, cancel) };
    }
    crate::lifecycle::send_waiters_free(waiters);
}

/// `php_swoole_server_onClose` (crates/phphttp's fallback).
/// # Safety
/// `server` is the running server and `info` the closed session's.
pub unsafe fn php_swoole_server_on_close(server: view::Server<'_>, info: *mut DataHead) {
    // SAFETY: per the caller.
    close(server, unsafe { &*info });
}

/// `php_swoole_server_onClose`: wake the session's senders, then `onClose`
/// (`onDisconnect` for a WebSocket port's connection that never opened).
fn close(server: view::Server<'_>, info: &DataHead) {
    let session_id = info.fd;
    let config = server.config();
    if config.enable_coroutine && config.send_yield {
        send_wake(session_id, true);
    }
    let mut cb = port_callback(server, info.server_fd, ON_CLOSE);
    let Some(conn) = server.by_session(session_id).map(ptr::from_mut) else {
        return;
    };
    // SAFETY: the session's live connection slot.
    if unsafe { (*conn).websocket_status } != WEBSOCKET_ACTIVE {
        let port = server.port_by_server_fd(c_int::from(info.server_fd));
        // SAFETY: the session's live port.
        let websocket = !port.is_null() && unsafe { view::port(port) }.open_websocket_protocol;
        if websocket && crate::lifecycle::isset(server, port, ON_DISCONNECT) {
            cb = port_callback(server, info.server_fd, ON_DISCONNECT);
        }
    }
    let reactor_id = i64::from(info.reactor_id);
    if !cb.is_null() {
        relay(on_connection(
            server, cb, true, session_id, reactor_id, info.time,
        ));
    }
    // SAFETY: as above; the callback may have changed it.
    let conn = unsafe { &*conn };
    if conn.http2_stream != 0
        && let Some(families) = swoole_rs_phpswoole::families()
    {
        (families.http.http2_on_close)(conn.session_id);
    }
}

extern "C" fn on_close(_: Ctx, server: view::Server<'_>, info: &DataHead) {
    close(server, info);
}

extern "C" fn on_http_close(_: Ctx, server: view::Server<'_>, info: &DataHead) {
    let Some(families) = swoole_rs_phpswoole::families() else {
        return;
    };
    // SAFETY: the Server's own event.
    unsafe { (families.http.on_close)(server.raw(), ptr::from_ref(info).cast_mut().cast()) };
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
    if server.config().send_yield {
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
    let server_object = crate::lifecycle::ServerObject::running();
    let process = if i64::from(config.mode_) == crate::lifecycle::MODE_THREAD {
        let index = id - i64::from(config.worker_num) - i64::from(config.task_worker_num);
        let index = usize::try_from(index).unwrap_or(usize::MAX);
        let process = server_object.user_process(index);
        // A user worker: just the message bus allocator.
        prepare_worker(server);
        process
    } else {
        // The worker `addProcess` made holds its process object.
        worker.user_data().cast::<zval>()
    };
    // SAFETY: the process object.
    let object = unsafe { object_of(process) };
    let ce = crate::lifecycle::process_class();
    // SAFETY: a live `Swoole\Process` object.
    unsafe { swoole_rs_zend::Props::new(ce, object) }.long(c"id", id);
    let gs = server.counters();
    let props = Props::server(server_object);
    props.long(c"master_pid", i64::from(gs.master_pid));
    props.long(c"manager_pid", i64::from(gs.manager_pid));
    // SAFETY: the worker and its process object; this frame owns nothing that
    // needs a drop, as the process exits through a bailout.
    unsafe { swr_v1_phpprocess_start(worker.raw().cast(), process) };
}

/// `SWR_PORTS_HOOK_*` of a port callback just set: install the Server's slot
/// unless one is set.
pub(crate) fn port_hook(server: view::Server<'_>, hook: u32) {
    let (slot, invoke) = match hook {
        1 => (CB_CONNECT, on_connect as *const c_void),
        2 => (CB_PACKET, on_packet_slot as *const c_void),
        3 => (CB_CLOSE, on_close as *const c_void),
        4 => (CB_BUFFER_FULL, on_buffer_full as *const c_void),
        5 => (CB_BUFFER_EMPTY, on_buffer_empty as *const c_void),
        _ => return,
    };
    if !server.has_callback(slot) {
        // SAFETY: as above; the fn has the slot's signature.
        unsafe { server.set_callback(slot, invoke) };
    }
}

/// `SWR_PHPSERVER_INSTALL_*` that fills slots.
pub(crate) fn install(server: view::Server<'_>, which: u32) {
    use crate::lifecycle::install::{
        BUFFER_EMPTY, CLOSE, CONTROL, HTTP_CLOSE, HTTP_RECEIVE, PIPE_MESSAGE, RECEIVE,
        REDIS_RECEIVE, TASK, USER_WORKER_START,
    };
    let slots: &[(u32, *const c_void)] = match which {
        CONTROL => &[
            (CB_START, on_start as *const c_void),
            (CB_BEFORE_SHUTDOWN, on_before_shutdown as *const c_void),
            (CB_SHUTDOWN, on_shutdown as *const c_void),
            (CB_WORKER_START, on_worker_start as *const c_void),
            (CB_WORKER_STOP, on_worker_stop as *const c_void),
            (CB_WORKER_EXIT, on_worker_exit as *const c_void),
            (CB_BEFORE_RELOAD, on_before_reload as *const c_void),
            (CB_AFTER_RELOAD, on_after_reload as *const c_void),
            (CB_MANAGER_START, on_manager_start as *const c_void),
            (CB_MANAGER_STOP, on_manager_stop as *const c_void),
            (CB_WORKER_ERROR, on_worker_error_slot as *const c_void),
        ],
        TASK => &[
            (CB_TASK, on_task as *const c_void),
            (CB_FINISH, on_finish as *const c_void),
        ],
        PIPE_MESSAGE => &[(CB_PIPE_MESSAGE, on_pipe_message_slot as *const c_void)],
        BUFFER_EMPTY => &[(CB_BUFFER_EMPTY, on_buffer_empty as *const c_void)],
        CLOSE => &[(CB_CLOSE, on_close as *const c_void)],
        HTTP_CLOSE => &[(CB_CLOSE, on_http_close as *const c_void)],
        RECEIVE => &[(CB_RECEIVE, on_receive as *const c_void)],
        HTTP_RECEIVE => &[(CB_RECEIVE, on_http_receive as *const c_void)],
        REDIS_RECEIVE => &[(CB_RECEIVE, on_redis_receive as *const c_void)],
        USER_WORKER_START if server.has_callback(CB_USER_WORKER_START) => &[],
        USER_WORKER_START => &[(CB_USER_WORKER_START, on_user_worker_start as *const c_void)],
        _ => return,
    };
    for &(slot, invoke) in slots {
        // SAFETY: a live server, configured before it starts; each fn has its
        // slot's signature.
        unsafe { server.set_callback(slot, invoke) };
    }
}

/// `SW_SERVER_EVENT_CLOSE`.
const EVENT_CLOSE: u8 = 4;

/// The `dispatch_func` setting: `handle` (a `callable::factory` handle) picks
/// the worker of each event from now on, in the reactor's one thread.
/// # Errors
/// Releasing the previous callable bailed out.
/// # Safety
/// `server` is a live server, not started.
pub(crate) unsafe fn set_dispatch_func(
    server: *mut c_void,
    handle: *mut callable::Callable,
) -> Result<(), Bailout> {
    // SAFETY: per the caller, `server` is a live `Server` for the call.
    let server = unsafe { view::Server::from_raw(server) };
    // SAFETY: the server is configured on this thread, with no other reference to the field.
    let config = unsafe { server.config_mut() };
    let previous = std::mem::replace(&mut config.private_data_3, handle.cast());
    // SAFETY: per the caller; the slot fn has `dispatch_func`'s signature.
    unsafe { server.set_callback(CB_DISPATCH_FUNC, on_dispatch as *const c_void) };
    config.single_thread = true;
    release_dispatch_func(previous)
}

/// Free the `dispatch_func` callable `set_dispatch_func` stored, if any.
/// # Errors
/// A destructor bailed out.
pub(crate) fn release_dispatch_func(handle: *mut c_void) -> Result<(), Bailout> {
    if handle.is_null() {
        return Ok(());
    }
    // SAFETY: a handle `set_dispatch_func` owned, released once.
    unsafe { callable::release(handle.cast()) }
}

/// `sw_zend_function_max_num_args`.
fn max_num_args(cache: *mut zend_fcall_info_cache) -> u32 {
    // SAFETY: a resolved cache.
    let function = unsafe { (*cache).function_handler };
    // SAFETY: its live function; every kind starts with `common`.
    let common = unsafe { &raw const (*function).common };
    // SAFETY: as above.
    let common = unsafe { common.read() };
    if common.fn_flags & ZEND_ACC_VARIADIC != 0 {
        u32::MAX
    } else {
        common.num_args
    }
}

/// `sw_zend_call_function_ex(nullptr, cache, args, retval)` under
/// `HOOK_PHP_CALL_STACK`: the reactor thread's stack is not the VM's.
fn call_unlimited(
    cache: *mut zend_fcall_info_cache,
    args: &mut [zval],
    retval: &mut zval,
) -> Result<bool, Bailout> {
    let mut saved = [ptr::null_mut(); 2];
    // SAFETY: two writable slots.
    unsafe { swr_zend_stack_unlimit(saved.as_mut_ptr()) };
    #[expect(clippy::cast_possible_truncation, reason = "at most four arguments")]
    let argc = args.len() as u32;
    // SAFETY: a resolved cache, initialised arguments and a return slot.
    let called = unsafe {
        function_call(
            cache,
            argc,
            args.as_mut_ptr(),
            retval,
            false,
            create_coroutine,
            swr_zend_callable_warning,
        )
    };
    // SAFETY: what `swr_zend_stack_unlimit` saved.
    unsafe { swr_zend_stack_restore(saved.as_ptr()) };
    called
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
    // SAFETY: as above.
    let cache = unsafe { callable::cache(config.private_data_3.cast()) };
    let zserv = php_swoole_server_zval_ptr();
    // SAFETY: its live object, copied without a reference as upstream.
    let zserv = unsafe { zserv.read() };
    let session = match conn {
        Some(conn) => conn.session_id,
        None => send.map_or(0, |send| send.info.fd),
    };
    let kind = send.map_or(EVENT_CLOSE, |send| send.info.kind);
    let mut args = [
        zserv,
        long_zval(session),
        long_zval(i64::from(kind)),
        undef(),
    ];
    let mut argc = 3;
    if let Some(send) = send.filter(|_| max_num_args(cache) > 3) {
        let length = (send.info.len as usize).min(SW_IPC_BUFFER_SIZE);
        // SAFETY: the payload holds `info.len` bytes.
        let bytes = unsafe { std::slice::from_raw_parts(send.data.cast::<u8>(), length) };
        // SAFETY: an undefined slot takes the new string.
        unsafe { set_str(&raw mut args[3], string::copy(bytes)) };
        argc = 4;
    }
    let mut retval = undef();
    let mut worker_id = -1;
    match call_unlimited(cache, args.get_mut(..argc).unwrap_or_default(), &mut retval) {
        Ok(false) => {
            if failed(&args[0], "onDispatch") == Called::Bailout {
                Bailout.defer();
            }
        }
        Ok(true) if swoole_rs_zend::kind(&retval) != IS_NULL => {
            // SAFETY: the call's return value.
            let id = unsafe { get_long(&raw const retval) };
            if id >= i64::from(config.worker_num) {
                let text = message!("invalid target worker-id[{id}]");
                crate::server::fatal(E_WARNING, text.as_cstr());
            } else {
                worker_id = id;
            }
            dtor(&mut retval);
        }
        Ok(true) => {}
        Err(_) => Bailout.defer(),
    }
    if argc == 4 {
        dtor(&mut args[3]);
    }
    bailout::finish();
    c_int::try_from(worker_id).unwrap_or(-1)
}

/// `PacketLength`'s head: the bytes read so far.
#[repr(C)]
struct PacketLength {
    buf: *const c_char,
    buf_size: u32,
}

/// `php_swoole_server_length_func`: a port's `package_length_func($data)`.
/// # Safety
/// `protocol` is a port's live protocol whose `private_data_1` is the
/// callable its settings made; `packet` the bytes so far.
pub(crate) unsafe extern "C" fn length_func(
    protocol: *const Protocol,
    _socket: *mut c_void,
    packet: *const c_void,
) -> isize {
    // SAFETY: per the caller.
    let owner = unsafe { (*protocol).private_data_1 };
    // SAFETY: a live handle.
    let cache = unsafe { callable::cache(owner.cast()) };
    // SAFETY: per the caller.
    let PacketLength {
        buf,
        buf_size: size,
    } = unsafe { packet.cast::<PacketLength>().read() };
    // SAFETY: as above.
    let bytes = unsafe { std::slice::from_raw_parts(buf.cast::<u8>(), size as usize) };
    let mut data = undef();
    // SAFETY: an undefined slot takes the new string.
    unsafe { set_str(&raw mut data, string::copy(bytes)) };
    let mut retval = undef();
    let mut result = -1;
    match call_unlimited(cache, std::slice::from_mut(&mut data), &mut retval) {
        Ok(false) => crate::server::fatal(E_WARNING, c"length function handler error"),
        Ok(true) => {
            // SAFETY: the call's return value.
            let value = unsafe { get_long(&raw const retval) };
            result = isize::try_from(value).unwrap_or(-1);
            dtor(&mut retval);
        }
        Err(_) => Bailout.defer(),
    }
    dtor(&mut data);
    bailout::finish();
    result
}

/// `*php_swoole_server_zval_ptr(serv) = *zserv`: no reference is taken.
pub(crate) fn set_server_zval(zserv: *mut zval) {
    let slot = php_swoole_server_zval_ptr();
    // SAFETY: the live object zval.
    let value = unsafe { *zserv };
    // SAFETY: the server's instance slot.
    unsafe { *slot = value };
}

/// The "`<class>->on<event> handler error`" warning of a failed call.
pub(crate) fn handler_error(server: &zval, event: &str) -> Result<(), Bailout> {
    if failed(server, event) == Called::Bailout {
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
            ON_START,
            ON_MANAGER_START,
            ON_MANAGER_STOP,
            ON_BEFORE_SHUTDOWN,
            ON_SHUTDOWN,
            ON_WORKER_START,
            ON_BEFORE_RELOAD,
            ON_AFTER_RELOAD,
            ON_WORKER_STOP,
            ON_WORKER_EXIT,
        ] {
            let (event, helper, _) = lifecycle(kind).unwrap();
            assert_eq!(event.as_bytes(), helper.to_bytes());
        }
        assert!(lifecycle(ON_WORKER_ERROR).is_none());
        assert!(lifecycle(ON_PIPE_MESSAGE).is_none());
    }
}
