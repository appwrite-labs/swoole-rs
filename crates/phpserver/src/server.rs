//! `Swoole\Server`'s classes, its connection and worker methods, and
//! `Swoole\Connection\Iterator`; lifecycle.rs owns the Server objects.
use std::{
    cell::{Cell, OnceCell, RefCell},
    ffi::{CStr, c_int, c_long, c_void},
    ptr,
    sync::atomic::{AtomicPtr, AtomicU64, Ordering},
};
use swoole_rs_buffer::string::CppString;
use swoole_rs_clients::address::{swr_v1_clients_address_port, with_host};
use swoole_rs_corebase::DataHead;
use swoole_rs_ossupport::QueueRecord;
use swoole_rs_phpstubs::server;
use swoole_rs_process::worker_record::Worker;
use swoole_rs_server::{Connection, SharedServer, server_methods, view};
use swoole_rs_zend::{
    Call, Class, Custom, CustomClass, E_WARNING, Minit, Value, report,
    sys::{ZEND_ACC_FINAL, ZEND_ACC_PRIVATE, ZEND_ACC_PUBLIC, zend_class_entry},
};
use swoole_rs_zend::{method_table, sys as z};

/// Swoole error codes (`include/swoole_error.h`).
pub(crate) const SW_ERROR_PHP_FATAL_ERROR: c_int = 503;
const SW_ERROR_NO_PAYLOAD: c_int = 550;
const SW_ERROR_SESSION_NOT_EXIST: c_int = 1005;
/// `SW_SERVER_EVENT_PAUSE_RECV` and `SW_SERVER_EVENT_RESUME_RECV`.
const EVENT_PAUSE_RECV: c_int = 11;
const EVENT_RESUME_RECV: c_int = 12;

/// `conn->closed`.
fn closed(conn: Connection<'_>) -> bool {
    conn.closed().load(Ordering::Relaxed) != 0
}
/// `SW_MAX_FIND_COUNT`.
const MAX_FIND_COUNT: i64 = 100;
/// `php_swoole_fatal_error(E_ERROR, "Invalid instance of %s", ...)` of an
/// object of class `name`: an `E_ERROR`, which bails out and does not return,
/// so the caller's frames own nothing a bailout would skip (as `server` and
/// its kin).
pub(crate) fn invalid(name: &CStr) {
    crate::set_last_error(SW_ERROR_PHP_FATAL_ERROR);
    let reported = {
        let text = swoole_rs_zend::message!("Invalid instance of {}", name.to_string_lossy());
        report(swoole_rs_zend::E_ERROR, text.as_cstr())
    };
    // The bailout jumps past this frame and the caller's, which own nothing.
    swoole_rs_zend::bailout::raise(reported);
}

/// `$this`'s class name, for [`invalid`].
pub(crate) fn this_class(call: &Call) -> &CStr {
    call.class_name().unwrap_or_default()
}

pub(crate) static SERVER: CustomClass<crate::lifecycle::ServerData> = CustomClass::new();
/// `Swoole\Redis\Server`, which crates/phpredis registers at MINIT.
pub static REDIS_SERVER: Class = Class::new();

/// `Swoole\Server`, for a class that extends it.
#[must_use]
pub fn server_class_ref() -> &'static Class {
    &SERVER
}

/// crates/phpredis' `onReceive` of a Redis port: the server, the request's
/// session, its payload and whether handlers run in a coroutine; `SW_OK` or
/// `SW_ERR`, or the handler's bailout.
pub type RedisReceive = fn(
    view::Server<'_>,
    c_long,
    &swoole_rs_zend::Owned,
    bool,
) -> Result<c_int, swoole_rs_zend::bailout::Bailout>;

static REDIS_RECEIVE: std::sync::OnceLock<RedisReceive> = std::sync::OnceLock::new();

/// Install crates/phpredis' `onReceive`, at MINIT; the first stands.
pub fn set_redis_receive(receive: RedisReceive) {
    REDIS_RECEIVE.get_or_init(|| receive);
}

pub(crate) fn redis_receive() -> Option<RedisReceive> {
    REDIS_RECEIVE.get().copied()
}

/// `swoole_server_ce`: `Swoole\Server`, registered at MINIT.
#[must_use]
pub fn server_class_entry() -> *mut zend_class_entry {
    SERVER.entry()
}
pub(crate) static TASK: CustomClass<TaskData> = CustomClass::new();
/// `Event`, `Packet`, `PipeMessage`, `StatusInfo` and `TaskResult`.
pub(crate) static DATA: [Class; 5] = [
    Class::new(),
    Class::new(),
    Class::new(),
    Class::new(),
    Class::new(),
];
static ITERATOR: CustomClass<Iterator> = CustomClass::new();

/// `SW_THREAD`, as MINIT was told.
pub(crate) use swoole_rs_server::THREAD_SUPPORTED as THREAD;

thread_local! {
    /// Each thread's `ServerPortProperty` per port `object_id` (`SW_THREAD`):
    /// every worker thread has its own port objects for the shared ports.
    static PORT_PROPERTIES: RefCell<Vec<*mut c_void>> = const { RefCell::new(Vec::new()) };
}

/// `php_swoole_server_{get,set}_port_property`: the port's `ServerPortProperty`,
/// kept per thread in thread mode and in the port's own `slot` (`port->ptr`)
/// otherwise. `set` stores `property` first; a get returns null for a port this
/// thread has none for.
pub(crate) fn port_property(
    object_id: u16,
    slot: &AtomicPtr<c_void>,
    property: *mut c_void,
    set: bool,
) -> *mut c_void {
    if !THREAD.load(Ordering::Relaxed) {
        if set {
            slot.store(property, Ordering::Relaxed);
        }
        return slot.load(Ordering::Relaxed);
    }
    let index = usize::from(object_id);
    PORT_PROPERTIES.with_borrow_mut(|properties| {
        if set {
            if properties.len() <= index {
                properties.resize(index + 1, ptr::null_mut());
            }
            if let Some(entry) = properties.get_mut(index) {
                *entry = property;
            }
        }
        properties.get(index).copied().unwrap_or(ptr::null_mut())
    })
}

/// `ConnectionIterator`: a `Connection\Iterator`'s position over its server's
/// connections, of one port or (null) all of them.
pub(crate) struct Iterator {
    current_fd: Cell<c_int>,
    session_id: Cell<i64>,
    /// The iterator's share in the server it walks; none for one PHP made itself.
    serv: OnceCell<SharedServer>,
    /// Which of the server's ports, by address; null for all of them.
    port: Cell<*mut c_void>,
    index: Cell<c_int>,
}

impl Iterator {
    /// The port whose connections the iterator walks; none for all of them.
    fn port(&self) -> Option<view::Port<'_>> {
        port_of(self.serv.get()?.view(), self.port.get())
    }
}

/// `server`'s port at `port`, an address kept only to name it; none for null.
pub(crate) fn port_of(server: view::Server<'_>, port: *mut c_void) -> Option<view::Port<'_>> {
    server.ports().iter().copied().find(|own| own.raw() == port)
}

impl Custom for Iterator {
    fn class() -> &'static CustomClass<Self> {
        &ITERATOR
    }

    /// An unbound iterator.
    fn new() -> Self {
        Self {
            current_fd: Cell::new(0),
            session_id: Cell::new(0),
            serv: OnceCell::new(),
            port: Cell::new(ptr::null_mut()),
            index: Cell::new(0),
        }
    }
}

/// Upstream's `ServerTaskObject`: the server and the task's head.
pub(crate) struct TaskData {
    /// The task's share in its server, once `task_bind` gave it one.
    serv: OnceCell<SharedServer>,
    info: Cell<DataHead>,
}

impl Custom for TaskData {
    fn class() -> &'static CustomClass<Self> {
        &TASK
    }

    /// A task bound to no server.
    fn new() -> Self {
        Self {
            serv: OnceCell::new(),
            info: Cell::new(DataHead::default()),
        }
    }
}

/// `$this`, a `Server\Task`; an `E_ERROR`, which does not return, otherwise.
fn task(call: &Call) -> &TaskData {
    call.custom::<TaskData>().unwrap_or_else(|| {
        invalid(this_class(call));
        // The `E_ERROR` bailed out of this frame.
        std::process::abort()
    })
}

/// A `Server\Task`'s `info`, the head of the task it answers.
pub(crate) fn task_info(call: &Call) -> DataHead {
    task(call).info.get()
}

/// Bind a new `Server\Task` to the running server and the head of the task it carries.
pub(crate) fn task_bind(task: &Value, info: &DataHead) {
    if let Some(task) = task.custom::<TaskData>() {
        let held = crate::lifecycle::ServerObject::running();
        let object = held.as_deref().and_then(crate::lifecycle::ServerObject::of);
        if let Some(serv) = object.and_then(crate::lifecycle::ServerObject::share) {
            // A new task has no server yet.
            task.serv.get_or_init(|| serv);
        }
        task.info.set(*info);
    }
}

/// A `Server\Task`'s server; an `E_ERROR`, which does not return, when it has none.
pub(crate) fn task_server(call: &Call) -> view::Server<'_> {
    task(call).serv.get().map_or_else(
        || {
            invalid(this_class(call));
            // The `E_ERROR` bailed out of this frame.
            std::process::abort()
        },
        SharedServer::view,
    )
}

/// A declared property: public unless `Private`.
enum Property {
    Null(&'static CStr),
    Private(&'static CStr),
    Long(&'static CStr, i64),
    Bool(&'static CStr),
    Str(&'static CStr),
    Double(&'static CStr),
}

/// Declare `properties` on `class`, in order.
fn declare(minit: &Minit, class: &Class, properties: &[Property]) {
    for property in properties {
        match *property {
            Property::Null(name) => minit.property_null(class, name, ZEND_ACC_PUBLIC),
            Property::Private(name) => minit.property_null(class, name, ZEND_ACC_PRIVATE),
            Property::Long(name, value) => minit.property_long(class, name, value, ZEND_ACC_PUBLIC),
            Property::Bool(name) => minit.property_bool(class, name, false, ZEND_ACC_PUBLIC),
            Property::Str(name) => minit.property_string(class, name, c"", ZEND_ACC_PUBLIC),
            Property::Double(name) => minit.property_double(class, name, 0.0, ZEND_ACC_PUBLIC),
        }
    }
}

use Property::{Bool, Double, Long, Null, Private, Str};

/// The data classes: no methods, standard objects.
const DATA_CLASSES: [(&CStr, &[Property]); 5] = [
    (
        c"Swoole\\Server\\Event",
        &[
            Long(c"reactor_id", 0),
            Long(c"fd", 0),
            Double(c"dispatch_time"),
            Null(c"data"),
        ],
    ),
    (
        c"Swoole\\Server\\Packet",
        &[
            Long(c"server_socket", 0),
            Long(c"server_port", 0),
            Double(c"dispatch_time"),
            Null(c"address"),
            Long(c"port", 0),
        ],
    ),
    (
        c"Swoole\\Server\\PipeMessage",
        &[
            Long(c"source_worker_id", 0),
            Long(c"worker_id", 0),
            Double(c"dispatch_time"),
            Null(c"data"),
        ],
    ),
    (
        c"Swoole\\Server\\StatusInfo",
        &[
            Long(c"worker_id", 0),
            Long(c"worker_pid", 0),
            Long(c"status", 0),
            Long(c"exit_code", 0),
            Long(c"signal", 0),
        ],
    ),
    (
        c"Swoole\\Server\\TaskResult",
        &[
            Long(c"task_id", 0),
            Long(c"task_worker_id", 0),
            Double(c"dispatch_time"),
            Null(c"data"),
        ],
    ),
];

const TASK_PROPERTIES: &[Property] = &[
    Null(c"data"),
    Double(c"dispatch_time"),
    Long(c"id", -1),
    Long(c"worker_id", -1),
    Long(c"flags", 0),
];

const SERVER_PROPERTIES: &[Property] = &[
    Private(c"onStart"),
    Private(c"onBeforeShutdown"),
    Private(c"onShutdown"),
    Private(c"onWorkerStart"),
    Private(c"onWorkerStop"),
    Private(c"onBeforeReload"),
    Private(c"onAfterReload"),
    Private(c"onWorkerExit"),
    Private(c"onWorkerError"),
    Private(c"onTask"),
    Private(c"onFinish"),
    Private(c"onManagerStart"),
    Private(c"onManagerStop"),
    Private(c"onPipeMessage"),
    Null(c"setting"),
    Null(c"connections"),
    Str(c"host"),
    Long(c"port", 0),
    Long(c"type", 0),
    Bool(c"ssl"),
    Long(c"mode", 0),
    Null(c"ports"),
    Long(c"master_pid", 0),
    Long(c"manager_pid", 0),
    Long(c"worker_id", -1),
    Bool(c"taskworker"),
    Long(c"worker_pid", 0),
    Null(c"stats_timer"),
    Null(c"admin_server"),
];

method_table! {
    static SERVER_METHODS = [
        c"__construct" => crate::lifecycle::zim_swoole_server___construct, &server::arginfo_class_Swoole_Server___construct, z::ZEND_ACC_PUBLIC;
        c"__destruct" => crate::lifecycle::zim_swoole_server___destruct, &server::arginfo_class_Swoole_Server___destruct, z::ZEND_ACC_PUBLIC;
        c"listen" => crate::io::zim_swoole_server_listen, &server::arginfo_class_Swoole_Server_listen, z::ZEND_ACC_PUBLIC;
        c"addlistener" => crate::io::zim_swoole_server_listen, &server::arginfo_class_Swoole_Server_listen, z::ZEND_ACC_PUBLIC;
        c"on" => crate::io::zim_swoole_server_on, &server::arginfo_class_Swoole_Server_on, z::ZEND_ACC_PUBLIC;
        c"getCallback" => crate::io::zim_swoole_server_getCallback, &server::arginfo_class_Swoole_Server_getCallback, z::ZEND_ACC_PUBLIC;
        c"set" => crate::settings::zim_swoole_server_set, &server::arginfo_class_Swoole_Server_set, z::ZEND_ACC_PUBLIC;
        c"start" => crate::lifecycle::zim_swoole_server_start, &server::arginfo_class_Swoole_Server_start, z::ZEND_ACC_PUBLIC;
        c"send" => crate::io::zim_swoole_server_send, &server::arginfo_class_Swoole_Server_send, z::ZEND_ACC_PUBLIC;
        c"sendto" => crate::io::zim_swoole_server_sendto, &server::arginfo_class_Swoole_Server_sendto, z::ZEND_ACC_PUBLIC;
        c"sendwait" => zim_swoole_server_sendwait, &server::arginfo_class_Swoole_Server_sendwait, z::ZEND_ACC_PUBLIC;
        c"exists" => zim_swoole_server_exists, &server::arginfo_class_Swoole_Server_exists, z::ZEND_ACC_PUBLIC;
        c"exist" => zim_swoole_server_exists, &server::arginfo_class_Swoole_Server_exists, z::ZEND_ACC_PUBLIC;
        c"protect" => zim_swoole_server_protect, &server::arginfo_class_Swoole_Server_protect, z::ZEND_ACC_PUBLIC;
        c"sendfile" => crate::io::zim_swoole_server_sendfile, &server::arginfo_class_Swoole_Server_sendfile, z::ZEND_ACC_PUBLIC;
        c"close" => zim_swoole_server_close, &server::arginfo_class_Swoole_Server_close, z::ZEND_ACC_PUBLIC;
        c"confirm" => zim_swoole_server_resume, &server::arginfo_class_Swoole_Server_resume, z::ZEND_ACC_PUBLIC;
        c"pause" => zim_swoole_server_pause, &server::arginfo_class_Swoole_Server_pause, z::ZEND_ACC_PUBLIC;
        c"resume" => zim_swoole_server_resume, &server::arginfo_class_Swoole_Server_resume, z::ZEND_ACC_PUBLIC;
        c"task" => crate::task::zim_swoole_server_task, &server::arginfo_class_Swoole_Server_task, z::ZEND_ACC_PUBLIC;
        c"taskwait" => crate::task::zim_swoole_server_taskwait, &server::arginfo_class_Swoole_Server_taskwait, z::ZEND_ACC_PUBLIC;
        c"taskWaitMulti" => crate::task::zim_swoole_server_taskWaitMulti, &server::arginfo_class_Swoole_Server_taskWaitMulti, z::ZEND_ACC_PUBLIC;
        c"taskCo" => crate::task::zim_swoole_server_taskCo, &server::arginfo_class_Swoole_Server_taskCo, z::ZEND_ACC_PUBLIC;
        c"finish" => crate::io::zim_swoole_server_finish, &server::arginfo_class_Swoole_Server_finish, z::ZEND_ACC_PUBLIC;
        c"reload" => zim_swoole_server_reload, &server::arginfo_class_Swoole_Server_reload, z::ZEND_ACC_PUBLIC;
        c"shutdown" => zim_swoole_server_shutdown, &server::arginfo_class_Swoole_Server_shutdown, z::ZEND_ACC_PUBLIC;
        c"stop" => zim_swoole_server_stop, &server::arginfo_class_Swoole_Server_stop, z::ZEND_ACC_PUBLIC;
        c"getLastError" => zif_swoole_last_error, &server::arginfo_class_Swoole_Server_getLastError, 0;
        c"heartbeat" => zim_swoole_server_heartbeat, &server::arginfo_class_Swoole_Server_heartbeat, z::ZEND_ACC_PUBLIC;
        c"getClientInfo" => zim_swoole_server_getClientInfo, &server::arginfo_class_Swoole_Server_getClientInfo, z::ZEND_ACC_PUBLIC;
        c"getClientList" => zim_swoole_server_getClientList, &server::arginfo_class_Swoole_Server_getClientList, z::ZEND_ACC_PUBLIC;
        c"getWorkerId" => zim_swoole_server_getWorkerId, &server::arginfo_class_Swoole_Server_getWorkerId, z::ZEND_ACC_PUBLIC;
        c"getWorkerPid" => zim_swoole_server_getWorkerPid, &server::arginfo_class_Swoole_Server_getWorkerPid, z::ZEND_ACC_PUBLIC;
        c"getWorkerStatus" => zim_swoole_server_getWorkerStatus, &server::arginfo_class_Swoole_Server_getWorkerStatus, z::ZEND_ACC_PUBLIC;
        c"getManagerPid" => zim_swoole_server_getManagerPid, &server::arginfo_class_Swoole_Server_getManagerPid, z::ZEND_ACC_PUBLIC;
        c"getMasterPid" => zim_swoole_server_getMasterPid, &server::arginfo_class_Swoole_Server_getMasterPid, z::ZEND_ACC_PUBLIC;
        c"connection_info" => zim_swoole_server_getClientInfo, &server::arginfo_class_Swoole_Server_getClientInfo, z::ZEND_ACC_PUBLIC;
        c"connection_list" => zim_swoole_server_getClientList, &server::arginfo_class_Swoole_Server_getClientList, z::ZEND_ACC_PUBLIC;
        c"sendMessage" => crate::io::zim_swoole_server_sendMessage, &server::arginfo_class_Swoole_Server_sendMessage, z::ZEND_ACC_PUBLIC;
        c"command" => crate::lifecycle::zim_swoole_server_command, &server::arginfo_class_Swoole_Server_command, z::ZEND_ACC_PUBLIC;
        c"addCommand" => crate::lifecycle::zim_swoole_server_addCommand, &server::arginfo_class_Swoole_Server_addCommand, z::ZEND_ACC_PUBLIC;
        c"addProcess" => crate::lifecycle::zim_swoole_server_addProcess, &server::arginfo_class_Swoole_Server_addProcess, z::ZEND_ACC_PUBLIC;
        c"stats" => zim_swoole_server_stats, &server::arginfo_class_Swoole_Server_stats, z::ZEND_ACC_PUBLIC;
        // SWOOLE_SOCKETS_SUPPORT builds only; MINIT drops it from the others.
        c"getSocket" => zim_swoole_server_getSocket, &server::arginfo_class_Swoole_Server_getSocket, z::ZEND_ACC_PUBLIC;
        c"bind" => zim_swoole_server_bind, &server::arginfo_class_Swoole_Server_bind, z::ZEND_ACC_PUBLIC;
    ];
}

method_table! {
    static ITERATOR_METHODS = [
        c"__construct" => zim_swoole_connection_iterator___construct, &server::arginfo_class_Swoole_Connection_Iterator___construct, z::ZEND_ACC_PUBLIC;
        c"__destruct" => zim_swoole_connection_iterator___destruct, &server::arginfo_class_Swoole_Connection_Iterator___destruct, z::ZEND_ACC_PUBLIC;
        c"rewind" => zim_swoole_connection_iterator_rewind, &server::arginfo_class_Swoole_Connection_Iterator_rewind, z::ZEND_ACC_PUBLIC;
        c"next" => zim_swoole_connection_iterator_next, &server::arginfo_class_Swoole_Connection_Iterator_next, z::ZEND_ACC_PUBLIC;
        c"current" => zim_swoole_connection_iterator_current, &server::arginfo_class_Swoole_Connection_Iterator_current, z::ZEND_ACC_PUBLIC;
        c"key" => zim_swoole_connection_iterator_key, &server::arginfo_class_Swoole_Connection_Iterator_key, z::ZEND_ACC_PUBLIC;
        c"valid" => zim_swoole_connection_iterator_valid, &server::arginfo_class_Swoole_Connection_Iterator_valid, z::ZEND_ACC_PUBLIC;
        c"count" => zim_swoole_connection_iterator_count, &server::arginfo_class_Swoole_Connection_Iterator_count, z::ZEND_ACC_PUBLIC;
        c"offsetExists" => zim_swoole_connection_iterator_offsetExists, &server::arginfo_class_Swoole_Connection_Iterator_offsetExists, z::ZEND_ACC_PUBLIC;
        c"offsetGet" => zim_swoole_connection_iterator_offsetGet, &server::arginfo_class_Swoole_Connection_Iterator_offsetGet, z::ZEND_ACC_PUBLIC;
        c"offsetSet" => zim_swoole_connection_iterator_offsetSet, &server::arginfo_class_Swoole_Connection_Iterator_offsetSet, z::ZEND_ACC_PUBLIC;
        c"offsetUnset" => zim_swoole_connection_iterator_offsetUnset, &server::arginfo_class_Swoole_Connection_Iterator_offsetUnset, z::ZEND_ACC_PUBLIC;
    ];
}

method_table! {
    static TASK_METHODS = [
        c"finish" => crate::io::zim_swoole_server_task_finish, &server::arginfo_class_Swoole_Server_Task_finish, z::ZEND_ACC_PUBLIC;
        c"pack" => crate::task::zim_swoole_server_task_pack, &server::arginfo_class_Swoole_Server_Task_pack, z::ZEND_ACC_PUBLIC | z::ZEND_ACC_STATIC;
        c"unpack" => crate::task::zim_swoole_server_task_unpack, &server::arginfo_class_Swoole_Server_Task_unpack, z::ZEND_ACC_PUBLIC | z::ZEND_ACC_STATIC;
    ];
}

// `swoole_last_error()`, which `getLastError()` aliases.
use swoole_rs_phpswoole::zif_swoole_last_error;

/// `php_swoole_server_minit`: `Swoole\Server` and its classes, then
/// `Swoole\Server\Port`.
pub fn php_swoole_server_minit(minit: &Minit) {
    use swoole_rs_corebase::{Switch, built_with};
    let (thread, sockets) = (built_with(Switch::Thread), built_with(Switch::Sockets));
    THREAD.store(thread, Ordering::Relaxed);
    let methods = if sockets {
        &SERVER_METHODS.0
    } else {
        SERVER_METHODS.without(&[c"getSocket"])
    };
    minit.register_object::<crate::lifecycle::ServerData>(c"Swoole\\Server", methods, 0);
    minit.register_object::<TaskData>(c"Swoole\\Server\\Task", &TASK_METHODS.0, ZEND_ACC_FINAL);
    declare(minit, &TASK, TASK_PROPERTIES);
    for (class, (name, properties)) in DATA.iter().zip(DATA_CLASSES) {
        // `SW_INIT_CLASS_ENTRY_DATA_OBJECT` has no methods.
        minit.register(class, name, &[], |_, _| {});
        declare(minit, class, properties);
    }
    minit.register_object::<Iterator>(c"Swoole\\Connection\\Iterator", &ITERATOR_METHODS.0, 0);
    minit.implements(&ITERATOR, minit.iterator());
    minit.implements(&ITERATOR, minit.array_access());
    minit.implements(&ITERATOR, minit.countable());
    declare(minit, &SERVER, SERVER_PROPERTIES);
    if thread {
        declare(minit, &SERVER, &[Str(c"bootstrap")]);
    }
    for &(name, value) in CONSTANTS {
        if name == c"SWOOLE_THREAD" && !thread {
            continue;
        }
        minit.long_constant(name, value);
    }
    crate::port::minit(minit, sockets);
}

/// The constants `php_swoole_server_minit` registers, in its order (the values
/// C++ asserts beside the call).
const CONSTANTS: &[(&CStr, i64)] = &[
    (c"SWOOLE_BASE", 1),
    (c"SWOOLE_PROCESS", 2),
    (c"SWOOLE_THREAD", 3),
    (c"SWOOLE_IPC_UNSOCK", 1),
    (c"SWOOLE_IPC_MSGQUEUE", 2),
    (c"SWOOLE_IPC_PREEMPTIVE", 3),
    (c"SWOOLE_SERVER_COMMAND_MASTER", 1 << 1),
    (c"SWOOLE_SERVER_COMMAND_MANAGER", 1 << 5),
    (c"SWOOLE_SERVER_COMMAND_REACTOR_THREAD", 1 << 2),
    (c"SWOOLE_SERVER_COMMAND_EVENT_WORKER", 1 << 3),
    (c"SWOOLE_SERVER_COMMAND_WORKER", 1 << 3),
    (c"SWOOLE_SERVER_COMMAND_TASK_WORKER", 1 << 4),
    (c"SWOOLE_DISPATCH_ROUND", 1),
    (c"SWOOLE_DISPATCH_FDMOD", 2),
    (c"SWOOLE_DISPATCH_IDLE_WORKER", 3),
    (c"SWOOLE_DISPATCH_IPMOD", 4),
    (c"SWOOLE_DISPATCH_UIDMOD", 5),
    (c"SWOOLE_DISPATCH_USERFUNC", 6),
    (c"SWOOLE_DISPATCH_STREAM", 7),
    (c"SWOOLE_DISPATCH_CO_CONN_LB", 8),
    (c"SWOOLE_DISPATCH_CO_REQ_LB", 9),
    (c"SWOOLE_DISPATCH_CONCURRENT_LB", 10),
    (c"SWOOLE_DISPATCH_RESULT_DISCARD_PACKET", -1),
    (c"SWOOLE_DISPATCH_RESULT_CLOSE_CONNECTION", -2),
    (c"SWOOLE_DISPATCH_RESULT_USERFUNC_FALLBACK", -3),
    (c"SWOOLE_TASK_TMPFILE", 1),
    (c"SWOOLE_TASK_SERIALIZE", 1 << 1),
    (c"SWOOLE_TASK_NONBLOCK", 1 << 2),
    (c"SWOOLE_TASK_CALLBACK", 1 << 3),
    (c"SWOOLE_TASK_WAITALL", 1 << 4),
    (c"SWOOLE_TASK_COROUTINE", 1 << 5),
    (c"SWOOLE_TASK_PEEK", 1 << 6),
    (c"SWOOLE_TASK_NOREPLY", 1 << 7),
    (c"SWOOLE_WORKER_BUSY", 1),
    (c"SWOOLE_WORKER_IDLE", 2),
    (c"SWOOLE_WORKER_EXIT", 3),
];

pub(crate) use swoole_rs_corebase::log::set_last_error;
pub(crate) use swoole_rs_phpswoole::display_errors;

/// `swoole_get_worker_type()`'s `SW_MASTER`, `SW_EVENT_WORKER`, `SW_MANAGER` and `SW_TASK_WORKER`.
pub(crate) const MASTER: c_int = 1;
pub(crate) const EVENT_WORKER: c_int = 2;
pub(crate) const MANAGER: c_int = 3;
pub(crate) const TASK_WORKER: c_int = 4;
/// `Server::MODE_PROCESS`.
const MODE_PROCESS: u8 = 2;

/// `swoole_get_worker_type()`.
pub(crate) fn worker_type() -> c_int {
    swoole_rs_process::methods::swoole_get_worker_type()
}

/// `Server::is_worker() || Server::is_task_worker()`.
pub(crate) fn in_worker() -> bool {
    matches!(worker_type(), EVENT_WORKER | TASK_WORKER)
}

/// `swoole_get_worker_id()`, as the `int` C++ reads.
pub(crate) fn worker_id() -> i64 {
    i64::from(swoole_rs_process::methods::swoole_get_worker_id().cast_signed())
}

/// `php_swoole_error_ex(E_WARNING, SW_ERROR_NO_PAYLOAD, "the data sent must not be empty")`.
pub(crate) fn no_payload() {
    set_last_error(SW_ERROR_NO_PAYLOAD);
    if display_errors() {
        swoole_rs_zend::bailout::defer(report(E_WARNING, c"the data sent must not be empty"));
    }
}

/// `php_swoole_fatal_error(level, message)`.
pub(crate) fn fatal(level: i32, message: &CStr) {
    set_last_error(SW_ERROR_PHP_FATAL_ERROR);
    // A bailout (an `E_ERROR`, or a user error handler's exit) waits for the
    // handler's last step, so callers return after it.
    swoole_rs_zend::bailout::defer(report(level, message));
}

/// `php_swoole_server_get_and_check_server(ZEND_THIS)`: an `E_ERROR`, which
/// does not return, for a Server never constructed. The frames own nothing.
pub(crate) fn server(call: &Call) -> view::Server<'_> {
    crate::lifecycle::ServerObject::this_server(call).unwrap_or_else(|| {
        invalid(this_class(call));
        // The `E_ERROR` bailed out of this frame.
        std::process::abort()
    })
}

/// A new `Connection\Iterator` over `serv` and `port` (null: every port).
pub(crate) fn new_iterator(serv: SharedServer, port: *mut c_void) -> swoole_rs_zend::Owned {
    swoole_rs_zend::new_object(Iterator {
        serv: OnceCell::from(serv),
        port: Cell::new(port),
        ..Iterator::new()
    })
}

/// `server(call)`, or `None` with a warning and `false` returned when the
/// server is not running.
#[inline]
pub(crate) fn running(call: &Call) -> Option<view::Server<'_>> {
    let serv = server(call);
    if serv.is_started() {
        return Some(serv);
    }
    fatal(E_WARNING, c"server is not running");
    call.set_bool(false);
    None
}

fn close(call: &Call) {
    let Some(serv) = running(call) else { return };
    let parsed = call
        .arity(1, 2)
        .and_then(|()| Ok((call.long(1, 0)?, call.bool(2, false)?)));
    let Ok((fd, reset)) = parsed else {
        return call.set_bool(false);
    };
    call.set_bool(server_methods::close(serv, fd, reset));
}

fn feedback(call: &Call, pause: bool) {
    let Some(serv) = running(call) else { return };
    let Ok(fd) = call.arity(1, 1).and_then(|()| call.long(1, 0)) else {
        return call.set_bool(false);
    };
    let Some(conn) = serv.verified(fd) else {
        set_last_error(SW_ERROR_SESSION_NOT_EXIST);
        return call.set_bool(false);
    };
    let event = if pause {
        EVENT_PAUSE_RECV
    } else {
        EVENT_RESUME_RECV
    };
    call.set_bool(server_methods::feedback(serv, conn, event));
}

fn pause(call: &Call) {
    feedback(call, true);
}

fn resume(call: &Call) {
    feedback(call, false);
}

/// `Server::stats()`'s first keys, each a count of the server's.
const STATS: [&CStr; 19] = [
    c"start_time",
    c"connection_num",
    c"abort_count",
    c"accept_count",
    c"close_count",
    c"worker_num",
    c"task_worker_num",
    c"user_worker_num",
    c"idle_worker_num",
    c"dispatch_count",
    c"request_count",
    c"response_count",
    c"total_recv_bytes",
    c"total_send_bytes",
    c"pipe_packet_msg_id",
    c"concurrency",
    c"session_round",
    c"min_fd",
    c"max_fd",
];
/// `Server::TASK_IPC_UNIXSOCK`.
const TASK_IPC_UNIXSOCK: u8 = 1;

fn stats(call: &Call) {
    let Some(serv) = running(call) else { return };
    let config = serv.config();
    let gs = serv.counters();
    let load = |count: &AtomicU64| count.load(Ordering::Relaxed).cast_signed();
    let values = [
        gs.start_time.load(Ordering::Relaxed),
        i64::try_from(serv.connection_num()).unwrap_or(i64::MAX),
        load(&gs.abort_count),
        load(&gs.accept_count),
        load(&gs.close_count),
        i64::from(config.worker_num),
        i64::from(config.task_worker_num),
        // SAFETY: a live server.
        i64::try_from(serv.user_worker_num()).unwrap_or(i64::MAX),
        i64::from(server_methods::get_idle_worker_num(serv)),
        load(&gs.dispatch_count),
        load(&gs.request_count),
        load(&gs.response_count),
        load(&gs.total_recv_bytes),
        load(&gs.total_send_bytes),
        load(&gs.pipe_packet_msg_id),
        i64::from(gs.concurrency.load(Ordering::Relaxed)),
        gs.session_round.load(Ordering::Relaxed),
        i64::from(gs.min_fd()),
        i64::from(gs.max_fd()),
    ];
    call.set_array();
    for (key, value) in STATS.into_iter().zip(values) {
        call.add_assoc_long(key, value);
    }
    let worker = swoole_rs_process::worker_globals::current_worker();
    if !worker.is_null() {
        // SAFETY: this process's live worker.
        let [request, response, dispatch, concurrency] = unsafe { Worker::new(worker).counters() };
        call.add_assoc_long(c"worker_request_count", request);
        call.add_assoc_long(c"worker_response_count", response);
        call.add_assoc_long(c"worker_dispatch_count", dispatch);
        call.add_assoc_long(c"worker_concurrency", concurrency);
    }
    let pool = serv.task_pool();
    // SAFETY: its live task pool.
    let queue = unsafe { (*pool).queue() };
    if config.task_ipc_mode > TASK_IPC_UNIXSOCK && !queue.is_null() {
        // SAFETY: the pool's live queue.
        if let Some((num, bytes)) = unsafe { &*queue.cast::<QueueRecord>() }.stat() {
            // (zend_long) of each count, as upstream.
            call.add_assoc_long(c"task_queue_num", num.cast_signed() as i64);
            call.add_assoc_long(c"task_queue_bytes", bytes.cast_signed() as i64);
        }
    }
    if config.task_worker_num > 0 {
        let idle = server_methods::get_idle_task_worker_num(serv);
        call.add_assoc_long(c"task_idle_worker_num", i64::from(idle));
        let tasking = server_methods::get_tasking_num(serv);
        call.add_assoc_long(c"tasking_num", i64::from(tasking));
        call.add_assoc_long(c"task_count", load(&gs.task_count));
    }
    let (num, peak) = swoole_rs_phpcoroutines::counts();
    call.add_assoc_long(c"coroutine_num", i64::try_from(num).unwrap_or(i64::MAX));
    call.add_assoc_long(
        c"coroutine_peek_num",
        i64::try_from(peak).unwrap_or(i64::MAX),
    );
}

fn reload(call: &Call) {
    let Some(serv) = running(call) else { return };
    let Ok(only_task_workers) = call.arity(0, 1).and_then(|()| call.bool(1, false)) else {
        return call.set_bool(false);
    };
    call.set_bool(swoole_rs_server::manager_ops::reload(
        serv,
        !only_task_workers,
    ));
}

/// `microtime()`: the wall clock in seconds.
fn microtime() -> f64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0.0, |now| now.as_secs_f64())
}

fn heartbeat(call: &Call) {
    let Some(serv) = running(call) else { return };
    let Ok(close) = call.arity(0, 1).and_then(|()| call.bool(1, false)) else {
        return call.set_bool(false);
    };
    if serv.config().heartbeat_check_interval < 1 {
        return call.set_bool(false);
    }
    call.set_array();
    let now = microtime();
    // `foreach_connection`: `max_fd` is read afresh for each fd.
    // SAFETY: a live server.
    let mut fd = serv.counters().min_fd();
    // SAFETY: as above.
    while fd <= serv.counters().max_fd() {
        if let Some(conn) = serv.iterated(fd) {
            let session_id = conn.session_id();
            if session_id > 0 && !serv.healthy(now, conn) {
                if close {
                    conn.set_close_force(1);
                    server_methods::close(serv, session_id, false);
                }
                call.add_next_index_long(session_id);
            }
        }
        fd += 1;
    }
}

fn bind(call: &Call) {
    let Some(serv) = running(call) else { return };
    let parsed = call
        .arity(2, 2)
        .and_then(|()| Ok((call.long(1, 0)?, call.long(2, 0)?)));
    let Ok((fd, uid)) = parsed else {
        return call.set_bool(false);
    };
    if uid > i64::from(u32::MAX) || uid < i64::from(i32::MIN) {
        fatal(
            E_WARNING,
            c"uid can not be greater than 4294967295 or less than -2147483648",
        );
        return call.set_bool(false);
    }
    #[expect(
        clippy::cast_possible_truncation,
        clippy::cast_sign_loss,
        reason = "(uint32_t) uid, as upstream"
    )]
    let uid = uid as u32;
    let Some(conn) = serv.verified(fd) else {
        return call.set_bool(false);
    };
    let cpus = u32::from(swoole_rs_corebase::swr_v1_core_cpu_num());
    // The connection's lock word, which every holder takes this way (`sw_spinlock`,
    // with `SW_SPINLOCK_LOOP_N`).
    swoole_rs_utilities::swr_v1_utils_spinlock(Some(conn.lock()), cpus, 1024);
    let bound = conn.uid() == 0;
    if bound {
        conn.set_uid(uid);
    }
    conn.lock().store(0, Ordering::Release);
    call.set_bool(bound);
}

fn get_socket(call: &Call) {
    let serv = server(call);
    let Ok(port) = call.arity(0, 1).and_then(|()| call.long(1, 0)) else {
        return call.set_bool(false);
    };
    #[expect(
        clippy::cast_possible_truncation,
        reason = "get_port((int) port), as upstream"
    )]
    match serv.port_number(port as c_int) {
        Some(ls) => crate::port::socket(ls, call),
        None => call.set_bool(false),
    }
}

/// `Server::DISPATCH_UIDMOD` and `DISPATCH_CO_CONN_LB`.
const DISPATCH_UIDMOD: u8 = 5;
const DISPATCH_CO_CONN_LB: u8 = 8;

#[expect(
    clippy::cast_possible_truncation,
    reason = "add_assoc_long of a double and (int) last_recv_time, as upstream"
)]
fn get_client_info(call: &Call) {
    let Some(serv) = running(call) else { return };
    let parsed = call.arity(1, 3).and_then(|()| {
        call.long(1, 0)
            .and_then(|fd| Ok((fd, call.long(2, -1)?, call.bool(3, false)?)))
    });
    let Ok((fd, _, _)) = parsed else {
        return call.set_bool(false);
    };
    let Some(conn) = serv.verified(fd) else {
        return call.set_bool(false);
    };
    let dispatch_mode = serv.config().dispatch_mode;
    call.set_array();
    if conn.uid() > 0 || dispatch_mode == DISPATCH_UIDMOD {
        call.add_assoc_long(c"uid", i64::from(conn.uid()));
    }
    if conn.worker_id() > 0 || dispatch_mode == DISPATCH_CO_CONN_LB {
        call.add_assoc_long(c"worker_id", i64::from(conn.worker_id()));
    }
    if serv
        .port_of_fd(conn.fd())
        .is_some_and(|port| port.open_websocket_protocol)
    {
        call.add_assoc_long(c"websocket_status", i64::from(conn.websocket_status()));
    }
    let cert = conn.ssl_client_cert().cast::<CppString>();
    if !cert.is_null()
        && conn.ssl_client_cert_pid() == swoole_rs_process::methods::swoole_get_worker_pid()
    {
        // SAFETY: the connection's live certificate string.
        let cert = unsafe { &*cert };
        call.add_assoc_stringl(c"ssl_client_cert", cert.view());
    }
    if let Some(server_socket) = serv.at(conn.server_fd()) {
        let port = swr_v1_clients_address_port(&server_socket.info());
        call.add_assoc_long(c"server_port", i64::from(port));
    }
    call.add_assoc_long(c"server_fd", i64::from(conn.server_fd()));
    call.add_assoc_long(c"socket_fd", i64::from(conn.fd()));
    call.add_assoc_long(c"socket_type", i64::from(conn.socket_type()));
    let remote_port = swr_v1_clients_address_port(&conn.info());
    call.add_assoc_long(c"remote_port", i64::from(remote_port));
    with_host(&conn.info(), |ip| {
        call.add_assoc_stringl(c"remote_ip", ip.to_bytes());
    });
    call.add_assoc_long(c"reactor_id", i64::from(conn.reactor_id()));
    call.add_assoc_long(c"connect_time", conn.connect_time() as i64);
    call.add_assoc_long(c"last_time", i64::from(conn.last_recv_time() as i32));
    call.add_assoc_double(c"last_recv_time", conn.last_recv_time());
    call.add_assoc_double(c"last_send_time", conn.last_send_time());
    call.add_assoc_double(c"last_dispatch_time", conn.last_dispatch_time());
    call.add_assoc_long(c"close_errno", i64::from(conn.close_errno()));
    let recv_queued = conn.recv_queued_bytes().load(Ordering::Relaxed);
    call.add_assoc_long(c"recv_queued_bytes", i64::from(recv_queued));
    call.add_assoc_long(c"send_queued_bytes", i64::from(conn.send_queued_bytes()));
}

fn get_client_list(call: &Call) {
    let Some(serv) = running(call) else { return };
    let parsed = call
        .arity(0, 2)
        .and_then(|()| Ok((call.long(1, 0)?, call.long(2, 10)?)));
    let Ok((start_session_id, mut find_count)) = parsed else {
        return call.set_bool(false);
    };
    if find_count > MAX_FIND_COUNT {
        fatal(E_WARNING, c"swoole connection list max_find_count=100");
        return call.set_bool(false);
    }
    let counters = serv.counters();
    let start_fd = if start_session_id == 0 {
        counters.min_fd()
    } else {
        match serv.verified(start_session_id) {
            None => return call.set_bool(false),
            Some(conn) => conn.fd(),
        }
    };
    let max_fd = counters.max_fd();
    if start_fd >= max_fd {
        return call.set_bool(false);
    }
    call.set_array();
    for fd in start_fd + 1..=max_fd {
        if let Some(conn) = serv.iterated(fd) {
            let session_id = conn.session_id();
            if session_id <= 0 {
                continue;
            }
            call.add_next_index_long(session_id);
            find_count -= 1;
        }
        if find_count <= 0 {
            break;
        }
    }
}

fn sendwait(call: &Call) {
    let Some(serv) = running(call) else { return };
    let parsed = call
        .arity(2, 2)
        .and_then(|()| Ok((call.long(1, 0)?, call.string(2, b"")?)));
    let Ok((fd, data)) = parsed else {
        return call.set_bool(false);
    };
    if data.is_empty() {
        no_payload();
        return call.set_bool(false);
    }
    if serv.config().mode_ == MODE_PROCESS || worker_type() == TASK_WORKER {
        fatal(
            E_WARNING,
            c"can only be used with base mode and must be within worker process",
        );
        return call.set_bool(false);
    }
    call.set_bool(server_methods::sendwait(serv, fd, data));
}

fn exists(call: &Call) {
    let Some(serv) = running(call) else { return };
    let Ok(session_id) = call.arity(1, 1).and_then(|()| call.long(1, 0)) else {
        return call.set_bool(false);
    };
    let conn = serv.verified(session_id);
    call.set_bool(conn.is_some_and(|conn| !closed(conn) && conn.closing() == 0));
}

fn protect(call: &Call) {
    let Some(serv) = running(call) else { return };
    let parsed = call
        .arity(1, 2)
        .and_then(|()| Ok((call.long(1, 0)?, call.bool(2, true)?)));
    let Ok((session_id, value)) = parsed else {
        return call.set_bool(false);
    };
    match serv.verified(session_id) {
        Some(conn) if !closed(conn) => {
            conn.set_protect(u8::from(value));
            call.set_bool(true);
        }
        _ => call.set_bool(false),
    }
}

fn get_worker_id(call: &Call) {
    server(call);
    if in_worker() {
        call.set_long(worker_id());
    } else {
        call.set_bool(false);
    }
}

/// The worker `get*` asks about: the argument, or this process's worker.
fn worker<'a>(call: &Call, serv: view::Server<'a>) -> Option<Worker<'a>> {
    let Ok(worker_id) = call.arity(0, 1).and_then(|()| call.long(1, -1)) else {
        call.set_bool(false);
        return None;
    };
    let worker_id = if worker_id < 0 {
        self::worker_id()
    } else {
        worker_id
    };
    #[expect(
        clippy::cast_possible_truncation,
        clippy::cast_sign_loss,
        reason = "get_worker's uint16_t id, as upstream"
    )]
    let worker_id = worker_id as u16;
    let worker = serv.worker_record(worker_id);
    if worker.is_none() {
        call.set_bool(false);
    }
    worker
}

fn get_worker_status(call: &Call) {
    let Some(serv) = running(call) else { return };
    if let Some(worker) = worker(call, serv) {
        call.set_long(i64::from(worker.status()));
    }
}

fn get_worker_pid(call: &Call) {
    let serv = server(call);
    if let Some(worker) = worker(call, serv) {
        call.set_long(i64::from(worker.pid()));
    }
}

fn get_manager_pid(call: &Call) {
    let serv = server(call);
    call.set_long(i64::from(
        serv.counters()
            .manager_pid
            .load(core::sync::atomic::Ordering::Relaxed),
    ));
}

fn get_master_pid(call: &Call) {
    let serv = server(call);
    call.set_long(i64::from(
        serv.counters()
            .master_pid
            .load(core::sync::atomic::Ordering::Relaxed),
    ));
}

fn shutdown(call: &Call) {
    let serv = server(call);
    call.set_bool(server_methods::shutdown(serv));
}

fn stop(call: &Call) {
    let Some(serv) = running(call) else { return };
    let Ok(worker_id) = call.arity(0, 1).and_then(|()| call.long(1, -1)) else {
        return call.set_bool(false);
    };
    #[expect(
        clippy::cast_possible_truncation,
        reason = "Server::kill_worker's int worker id, as upstream"
    )]
    let worker_id = worker_id as c_int;
    call.set_bool(swoole_rs_server::event_worker_ops::kill_worker(
        serv, worker_id,
    ));
}

/// `php_swoole_connection_iterator_get_and_check_ptr(ZEND_THIS)`.
fn iterator(call: &Call) -> (&Iterator, view::Server<'_>) {
    let bound = call.custom::<Iterator>();
    let bound = bound.and_then(|iterator| Some((iterator, iterator.serv.get()?.view())));
    bound.unwrap_or_else(|| {
        // An `E_ERROR`, which does not return; the frames own nothing.
        invalid(this_class(call));
        std::process::abort()
    })
}

fn iterator_construct(call: &Call) {
    call.throw_error(c"please use the Swoole\\Server->connections");
}

fn rewind(call: &Call) {
    let (iterator, server) = iterator(call);
    iterator.index.set(0);
    iterator.current_fd.set(server.counters().min_fd());
}

fn valid(call: &Call) {
    let (iterator, server) = iterator(call);
    let max_fd = server.counters().max_fd();
    let port = iterator.port();
    for fd in iterator.current_fd.get()..=max_fd {
        let Some(conn) = server.iterated(fd) else {
            continue;
        };
        let (session_id, server_fd) = (conn.session_id(), conn.server_fd());
        if session_id <= 0 {
            continue;
        }
        if let Some(port) = port {
            let port_fd = port.fd();
            if port_fd < 0 || server_fd != port_fd {
                continue;
            }
        }
        iterator.session_id.set(session_id);
        iterator.current_fd.set(fd);
        iterator.index.set(iterator.index.get() + 1);
        return call.set_bool(true);
    }
    call.set_bool(false);
}

fn current(call: &Call) {
    call.set_long(iterator(call).0.session_id.get());
}

fn next(call: &Call) {
    let fd = &iterator(call).0.current_fd;
    fd.set(fd.get() + 1);
}

fn key(call: &Call) {
    call.set_long(i64::from(iterator(call).0.index.get()));
}

fn count(call: &Call) {
    let (iterator, server) = iterator(call);
    let num = match iterator.port() {
        None => server.connection_num(),
        Some(port) => server.port_connection_num(port),
    };
    call.set_long(i64::try_from(num).unwrap_or(i64::MAX));
}

/// `offsetExists`/`offsetGet`: `Swoole\Server`'s own method, called on the
/// server object as PHP would.
fn delegate(call: &Call, method: &[u8]) {
    iterator(call);
    if call.arity(1, 1).is_err() {
        return call.set_bool(false);
    }
    // The running server's object, held for the call.
    let Some(object) = crate::lifecycle::ServerObject::running() else {
        return call.set_bool(false);
    };
    let args: Vec<&Value> = call.value(1).into_iter().collect();
    let called = object.call_method(Some(&SERVER), method, &args);
    drop((args, object));
    let retval = match called {
        Ok(retval) => retval,
        // Nothing left here to release: the bailout jumps over this frame.
        Err(bailout) => return swoole_rs_zend::bailout::raise(Err(bailout)),
    };
    if method == b"exists" {
        // RETVAL_BOOL(Z_BVAL_P(&retval)): only `true` is true.
        call.set_bool(retval.kind() == swoole_rs_zend::sys::IS_TRUE);
    } else {
        // RETVAL_ZVAL(&retval, 0, 0): the return slot takes the result.
        call.set_owned(retval);
    }
}

fn offset_exists(call: &Call) {
    delegate(call, b"exists");
}

fn offset_get(call: &Call) {
    delegate(call, b"getClientInfo");
}

fn nothing(_: &Call) {}

swoole_rs_zend::handlers! {
    zim_swoole_server_close => close;
    zim_swoole_server_pause => pause;
    zim_swoole_server_resume => resume;
    zim_swoole_server_stats => stats;
    zim_swoole_server_reload => reload;
    zim_swoole_server_heartbeat => heartbeat;
    zim_swoole_server_bind => bind;
    zim_swoole_server_getSocket => get_socket;
    zim_swoole_server_getClientInfo => get_client_info;
    zim_swoole_server_getClientList => get_client_list;
    zim_swoole_server_sendwait => sendwait;
    zim_swoole_server_exists => exists;
    zim_swoole_server_protect => protect;
    zim_swoole_server_getWorkerId => get_worker_id;
    zim_swoole_server_getWorkerStatus => get_worker_status;
    zim_swoole_server_getWorkerPid => get_worker_pid;
    zim_swoole_server_getManagerPid => get_manager_pid;
    zim_swoole_server_getMasterPid => get_master_pid;
    zim_swoole_server_shutdown => shutdown;
    zim_swoole_server_stop => stop;
    zim_swoole_connection_iterator___construct => iterator_construct;
    zim_swoole_connection_iterator___destruct => nothing;
    zim_swoole_connection_iterator_rewind => rewind;
    zim_swoole_connection_iterator_valid => valid;
    zim_swoole_connection_iterator_current => current;
    zim_swoole_connection_iterator_next => next;
    zim_swoole_connection_iterator_key => key;
    zim_swoole_connection_iterator_count => count;
    zim_swoole_connection_iterator_offsetExists => offset_exists;
    zim_swoole_connection_iterator_offsetGet => offset_get;
    zim_swoole_connection_iterator_offsetSet => nothing;
    zim_swoole_connection_iterator_offsetUnset => nothing;
}
