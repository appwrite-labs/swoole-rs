//! `Swoole\Server`'s classes, its connection and worker methods, and
//! `Swoole\Connection\Iterator`. C++ keeps the Server and its `Server\Task`
//! objects and answers through the host table
//! (`native/process/swoole_rs_phpserver_server.h`); lifecycle.rs owns the Server
//! objects.
use std::{
    cell::RefCell,
    ffi::{CStr, c_char, c_int, c_void},
    ptr,
    sync::atomic::{AtomicBool, AtomicPtr, Ordering},
};
use swoole_rs_phpstubs::server;
use swoole_rs_zend::{
    Call, Class, E_WARNING, Object, declare_bool, declare_long, declare_property_null,
    declare_string, php_error, register_long_constant,
    sys::{
        _zend_bailout, ZEND_ACC_FINAL, ZEND_ACC_NOT_SERIALIZABLE, ZEND_ACC_PRIVATE,
        ZEND_ACC_PUBLIC, add_assoc_double_ex, add_assoc_stringl_ex, add_next_index_long,
        zend_ce_arrayaccess, zend_ce_countable, zend_ce_iterator, zend_class_entry,
        zend_class_implements, zend_declare_property_double, zend_object, zend_object_handlers,
        zval,
    },
    throw_error, unset_property_deny,
};
use swoole_rs_zend::{method_table, sys as z};

/// Swoole error codes (`include/swoole_error.h`).
pub(crate) const SW_ERROR_PHP_FATAL_ERROR: c_int = 503;
const SW_ERROR_NO_PAYLOAD: c_int = 550;
const SW_ERROR_SESSION_NOT_EXIST: c_int = 1005;
/// `SW_MAX_FIND_COUNT`.
const MAX_FIND_COUNT: i64 = 100;
/// `SWR_PHPSERVER_STATS_*`.
const STATS_WORKER: usize = 19;
const STATS_QUEUE: usize = 24;
const STATS_TASKS: usize = 27;
const STATS_COUNT: usize = 33;

/// `swr_phpserver_client`.
#[repr(C)]
pub struct ServerClient {
    uid: i64,
    worker_id: i64,
    websocket_status: i64,
    server_port: i64,
    server_fd: i64,
    socket_fd: i64,
    socket_type: i64,
    remote_port: i64,
    reactor_id: i64,
    connect_time: i64,
    last_time: i64,
    close_errno: i64,
    recv_queued_bytes: i64,
    send_queued_bytes: i64,
    last_recv_time: f64,
    last_send_time: f64,
    last_dispatch_time: f64,
    remote_ip: *const c_char,
    ssl_client_cert: *const c_char,
    ssl_client_cert_length: usize,
    uid_mode: bool,
    co_conn_lb: bool,
    websocket: bool,
}

type Each = unsafe extern "C" fn(*mut c_void, i64);

/// `swr_v1_phpserver_host`.
#[repr(C)]
pub struct ServerHost {
    invalid: unsafe extern "C" fn(*mut zend_object),
    set_last_error: unsafe extern "C" fn(c_int),
    display_errors: unsafe extern "C" fn() -> bool,
    started: unsafe extern "C" fn(*mut c_void) -> bool,
    role: unsafe extern "C" fn(*mut c_void, c_int) -> bool,
    worker_id: unsafe extern "C" fn() -> i64,
    pid: unsafe extern "C" fn(*mut c_void, c_int) -> i64,
    worker: unsafe extern "C" fn(*mut c_void, i64, *mut i64, *mut i64) -> bool,
    close: unsafe extern "C" fn(*mut c_void, i64, bool) -> bool,
    feedback: unsafe extern "C" fn(*mut c_void, i64, bool) -> c_int,
    exists: unsafe extern "C" fn(*mut c_void, i64) -> c_int,
    protect: unsafe extern "C" fn(*mut c_void, i64, bool) -> bool,
    bind: unsafe extern "C" fn(*mut c_void, i64, u32) -> c_int,
    client: unsafe extern "C" fn(*mut c_void, i64, *mut ServerClient) -> bool,
    sendwait: unsafe extern "C" fn(*mut c_void, i64, *const c_char, usize) -> bool,
    reload: unsafe extern "C" fn(*mut c_void, bool) -> bool,
    shutdown: unsafe extern "C" fn(*mut c_void) -> bool,
    kill_worker: unsafe extern "C" fn(*mut c_void, i64) -> bool,
    stats: unsafe extern "C" fn(*mut c_void, *mut i64),
    heartbeat_interval: unsafe extern "C" fn(*mut c_void) -> i64,
    heartbeat: unsafe extern "C" fn(*mut c_void, bool, Each, *mut c_void),
    socket: unsafe extern "C" fn(*mut c_void, i64, *mut zval),
    min_fd: unsafe extern "C" fn(*mut c_void) -> c_int,
    max_fd: unsafe extern "C" fn(*mut c_void) -> c_int,
    session_fd: unsafe extern "C" fn(*mut c_void, i64) -> c_int,
    connection_at: unsafe extern "C" fn(*mut c_void, c_int, *mut i64, *mut c_int) -> bool,
    port_fd: unsafe extern "C" fn(*mut c_void) -> c_int,
    connection_num: unsafe extern "C" fn(*mut c_void, *mut c_void) -> i64,
    call: unsafe extern "C" fn(*mut c_void, c_int, *mut zval, *mut zval) -> bool,
}

static HOST: AtomicPtr<ServerHost> = AtomicPtr::new(ptr::null_mut());

fn host() -> &'static ServerHost {
    // SAFETY: MINIT stores C++'s static table before any object or method exists.
    unsafe { &*HOST.load(Ordering::Acquire) }
}

pub(crate) static SERVER: Class = Class::new();
pub(crate) static TASK: Class = Class::new();
/// `Event`, `Packet`, `PipeMessage`, `StatusInfo` and `TaskResult`.
pub(crate) static DATA: [Class; 5] = [
    Class::new(),
    Class::new(),
    Class::new(),
    Class::new(),
    Class::new(),
];
static ITERATOR: Class = Class::new();

/// `ServerObject` before its `zend_object`: `Server *serv; void *property;
/// zval init_arguments;`, where `property` is lifecycle.rs's `Property`.
#[repr(C)]
pub(crate) struct ServerData {
    pub(crate) serv: *mut c_void,
    pub(crate) property: *mut c_void,
    pub(crate) init_arguments: zval,
}

/// `SW_THREAD`, as MINIT was told.
pub(crate) static THREAD: AtomicBool = AtomicBool::new(false);

thread_local! {
    /// Each thread's `ServerPortProperty` per port `object_id` (`SW_THREAD`):
    /// every worker thread has its own port objects for the shared ports.
    static PORT_PROPERTIES: RefCell<Vec<*mut c_void>> = const { RefCell::new(Vec::new()) };
}

/// `php_swoole_server_{get,set}_port_property`: the port's `ServerPortProperty`,
/// kept per thread in thread mode and in the port's own `slot` (`port->ptr`)
/// otherwise. `set` stores `property` first; a get returns null for a port this
/// thread has none for.
/// # Safety
/// `slot` is the port's `ptr` field.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn swr_v1_phpserver_port_property(
    object_id: u16,
    slot: *mut *mut c_void,
    property: *mut c_void,
    set: bool,
) -> *mut c_void {
    if !THREAD.load(Ordering::Relaxed) {
        if set {
            // SAFETY: the port's field, per the contract.
            unsafe { *slot = property };
        }
        // SAFETY: as above.
        return unsafe { *slot };
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

/// `ConnectionIterator`.
#[repr(C)]
struct Iterator {
    current_fd: c_int,
    session_id: i64,
    serv: *mut c_void,
    port: *mut c_void,
    index: c_int,
}

type IteratorObject = Object<Iterator>;

/// `ServerTaskObject` before its `zend_object`: `Server *serv; DataHead info;`
/// (C++ binds and reads them).
#[repr(C)]
struct TaskData {
    serv: *mut c_void,
    info: [u64; 5],
}

type TaskObject = Object<TaskData>;

/// A `Server\Task`'s `info`, as the `EventData *` `finish` reads its head from.
pub(crate) fn task_info(object: *mut zend_object) -> *mut c_void {
    // SAFETY: a `Server\Task` object, which this class creates.
    let data = unsafe { TaskObject::data(object) };
    data.info.as_mut_ptr().cast()
}

unsafe extern "C" fn task_create(ce: *mut zend_class_entry) -> *mut zend_object {
    // SAFETY: Zend creates a task of this class; zeroes are an unbound one.
    unsafe { TaskObject::create(&TASK, ce) }
}

unsafe extern "C" fn task_free(object: *mut zend_object) {
    // SAFETY: Zend frees an object this class created.
    unsafe { TaskObject::dtor(object) };
}

fn deny(handlers: &mut zend_object_handlers) {
    handlers.clone_obj = None;
    handlers.unset_property = Some(unset_property_deny);
}

unsafe extern "C" fn iterator_create(ce: *mut zend_class_entry) -> *mut zend_object {
    // SAFETY: Zend creates an iterator of this class; zeroes are an unbound one.
    unsafe { IteratorObject::create(&ITERATOR, ce) }
}

unsafe extern "C" fn iterator_free(object: *mut zend_object) {
    // SAFETY: Zend frees an object this class created.
    unsafe { IteratorObject::dtor(object) };
}

#[expect(
    clippy::cast_possible_wrap,
    reason = "ZEND_ACC_* visibilities are small"
)]
const PUBLIC: c_int = ZEND_ACC_PUBLIC as c_int;

/// `zend_declare_property_double(ce, name, value, ZEND_ACC_PUBLIC)`.
/// # Safety
/// MINIT, with a registered class.
unsafe fn declare_double(ce: *mut zend_class_entry, name: &CStr) {
    // SAFETY: per the caller; Zend copies the name.
    unsafe { zend_declare_property_double(ce, name.as_ptr(), name.count_bytes(), 0.0, PUBLIC) };
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

/// Declare `properties` on `ce`, in order.
/// # Safety
/// MINIT, with a registered class.
unsafe fn declare(ce: *mut zend_class_entry, properties: &[Property]) {
    for property in properties {
        match *property {
            // SAFETY: MINIT, with a registered class.
            Property::Null(name) => unsafe { declare_property_null(ce, name, ZEND_ACC_PUBLIC) },
            // SAFETY: as above.
            Property::Private(name) => unsafe {
                declare_property_null(ce, name, ZEND_ACC_PRIVATE);
            },
            // SAFETY: as above.
            Property::Long(name, value) => unsafe {
                declare_long(ce, name, value, ZEND_ACC_PUBLIC);
            },
            // SAFETY: as above.
            Property::Bool(name) => unsafe { declare_bool(ce, name, false, ZEND_ACC_PUBLIC) },
            // SAFETY: as above.
            Property::Str(name) => unsafe { declare_string(ce, name, c"", ZEND_ACC_PUBLIC) },
            // SAFETY: as above.
            Property::Double(name) => unsafe { declare_double(ce, name) },
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

// `swoole_last_error()` (crates/phpswoole), which `getLastError()` aliases.
unsafe extern "C" {
    fn zif_swoole_last_error(frame: *mut z::zend_execute_data, value: *mut z::zval);
}

/// MINIT, as `php_swoole_server_minit` did.
/// # Safety
/// Called once, at MINIT, with a host table that lives for the process and a
/// writable `handlers`; `sockets` is `SWOOLE_SOCKETS_SUPPORT`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn swr_v1_phpserver_minit(
    table: *const ServerHost,
    thread: bool,
    sockets: bool,
    module: c_int,
    handlers: *mut *const zend_object_handlers,
) -> *mut zend_class_entry {
    HOST.store(table.cast_mut(), Ordering::Release);
    THREAD.store(thread, Ordering::Relaxed);
    let methods = if sockets {
        SERVER_METHODS.as_ptr()
    } else {
        SERVER_METHODS.without(&[c"getSocket"])
    };
    // SAFETY: MINIT, with tables that live for the process.
    unsafe {
        SERVER.register(c"Swoole\\Server", methods, |entry, handlers| {
            entry.ce_flags |= ZEND_ACC_NOT_SERIALIZABLE;
            deny(handlers);
            Object::<ServerData>::install(
                entry,
                handlers,
                crate::lifecycle::swr_v1_phpserver_create,
                crate::lifecycle::swr_v1_phpserver_free,
            );
        });
    }
    // SAFETY: as above.
    unsafe {
        TASK.register(
            c"Swoole\\Server\\Task",
            TASK_METHODS.as_ptr(),
            |entry, handlers| {
                entry.ce_flags |= ZEND_ACC_FINAL | ZEND_ACC_NOT_SERIALIZABLE;
                deny(handlers);
                TaskObject::install(entry, handlers, task_create, task_free);
            },
        );
    }
    // SAFETY: MINIT, with the class just registered.
    unsafe { declare(TASK.entry(), TASK_PROPERTIES) };
    for (class, (name, properties)) in DATA.iter().zip(DATA_CLASSES) {
        // SAFETY: MINIT; `SW_INIT_CLASS_ENTRY_DATA_OBJECT` has no methods.
        unsafe { class.register(name, ptr::null(), |_, _| {}) };
        // SAFETY: MINIT, with the class just registered.
        unsafe { declare(class.entry(), properties) };
    }
    // SAFETY: MINIT, with a static table.
    unsafe {
        ITERATOR.register(
            c"Swoole\\Connection\\Iterator",
            ITERATOR_METHODS.as_ptr(),
            |entry, handlers| {
                entry.ce_flags |= ZEND_ACC_NOT_SERIALIZABLE;
                deny(handlers);
                IteratorObject::install(entry, handlers, iterator_create, iterator_free);
            },
        );
    }
    let iterator = ITERATOR.entry();
    // SAFETY: MINIT; Zend's own interfaces are registered before extensions
    // and not written after.
    let iterator_ce = unsafe { zend_ce_iterator };
    // SAFETY: as above.
    let array_access = unsafe { zend_ce_arrayaccess };
    // SAFETY: as above.
    let countable = unsafe { zend_ce_countable };
    // SAFETY: MINIT, with the class registered above.
    unsafe { zend_class_implements(iterator, 2, iterator_ce, array_access) };
    // SAFETY: as above.
    unsafe { zend_class_implements(iterator, 1, countable) };
    let server = SERVER.entry();
    // SAFETY: MINIT, with the class registered above.
    unsafe { declare(server, SERVER_PROPERTIES) };
    if thread {
        // SAFETY: as above.
        unsafe { declare(server, &[Str(c"bootstrap")]) };
    }
    for &(name, value) in CONSTANTS {
        if name == c"SWOOLE_THREAD" && !thread {
            continue;
        }
        // SAFETY: MINIT of `module`.
        unsafe { register_long_constant(name, value, module) };
    }
    // SAFETY: the caller's writable out-parameter.
    unsafe { handlers.write(SERVER.handlers()) };
    server
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

/// `php_swoole_error_ex(E_WARNING, SW_ERROR_NO_PAYLOAD, "the data sent must not be empty")`.
pub(crate) fn no_payload() {
    let host = host();
    // SAFETY: plain state.
    unsafe { (host.set_last_error)(SW_ERROR_NO_PAYLOAD) };
    // SAFETY: reads a setting.
    if unsafe { (host.display_errors)() } {
        // SAFETY: from a handler; a warning.
        swoole_rs_zend::bailout::defer(unsafe {
            php_error(E_WARNING, c"the data sent must not be empty")
        });
    }
}

/// `php_swoole_fatal_error(level, message)`.
pub(crate) fn fatal(level: i32, message: &CStr) {
    // SAFETY: plain state, then a warning.
    unsafe { (host().set_last_error)(SW_ERROR_PHP_FATAL_ERROR) };
    // SAFETY: from a handler; a bailout (an `E_ERROR`, or a user error handler's
    // exit) waits for the handler's last step, so callers return after it.
    swoole_rs_zend::bailout::defer(unsafe { php_error(level, message) });
}

/// `php_swoole_server_get_and_check_server(ZEND_THIS)`: an `E_ERROR`, which
/// does not return, for a Server never constructed. The frames own nothing.
pub(crate) fn server(call: &Call) -> *mut c_void {
    // SAFETY: `$this` is a Server, which C++ allocated as a `ServerObject`.
    let serv = unsafe { Object::<ServerData>::data(call.this()) }.serv;
    if serv.is_null() {
        // SAFETY: `$this` of this call.
        unsafe { (host().invalid)(call.this()) };
    }
    serv
}

/// `SWOOLE_G(display_errors)`.
pub(crate) fn display_errors() -> bool {
    // SAFETY: reads a setting.
    unsafe { (host().display_errors)() }
}

/// A new `Connection\Iterator` over `serv` and `port` (null: every port) in `target`.
pub(crate) fn new_iterator(target: &mut zval, serv: *mut c_void, port: *mut c_void) {
    // SAFETY: a writable zval and the registered iterator class.
    unsafe { swoole_rs_zend::init_object(target, ITERATOR.entry()) };
    // SAFETY: an object zval.
    let object = unsafe { swoole_rs_zend::object_of(target) };
    // SAFETY: an iterator object just created.
    let iterator = unsafe { IteratorObject::data(object) };
    iterator.serv = serv;
    iterator.port = port;
}

/// `server(call)`, or `None` with a warning and `false` returned when the
/// server is not running.
pub(crate) fn running(call: &Call) -> Option<*mut c_void> {
    let serv = server(call);
    // SAFETY: a live server.
    if unsafe { (host().started)(serv) } {
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
    // SAFETY: a live server.
    call.set_bool(unsafe { (host().close)(serv, fd, reset) });
}

fn feedback(call: &Call, pause: bool) {
    let Some(serv) = running(call) else { return };
    let Ok(fd) = call.arity(1, 1).and_then(|()| call.long(1, 0)) else {
        return call.set_bool(false);
    };
    // SAFETY: a live server.
    match unsafe { (host().feedback)(serv, fd, pause) } {
        -1 => {
            // SAFETY: plain state.
            unsafe { (host().set_last_error)(SW_ERROR_SESSION_NOT_EXIST) };
            call.set_bool(false);
        }
        result => call.set_bool(result != 0),
    }
}

fn pause(call: &Call) {
    feedback(call, true);
}

fn resume(call: &Call) {
    feedback(call, false);
}

/// `stats()` keys, by `SWR_PHPSERVER_STATS_*` index (flags hold no key).
const STATS: [&CStr; STATS_COUNT] = [
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
    c"",
    c"worker_request_count",
    c"worker_response_count",
    c"worker_dispatch_count",
    c"worker_concurrency",
    c"",
    c"task_queue_num",
    c"task_queue_bytes",
    c"",
    c"task_idle_worker_num",
    c"tasking_num",
    c"task_count",
    c"coroutine_num",
    c"coroutine_peek_num",
];

fn stats(call: &Call) {
    let Some(serv) = running(call) else { return };
    let mut values = [0i64; STATS_COUNT];
    // SAFETY: a live server and room for every value.
    unsafe { (host().stats)(serv, values.as_mut_ptr()) };
    call.set_array();
    let mut index = 0;
    while index < STATS_COUNT {
        let group = matches!(index, STATS_WORKER | STATS_QUEUE | STATS_TASKS);
        if group {
            let length = match index {
                STATS_WORKER => 4,
                STATS_QUEUE => 2,
                _ => 3,
            };
            if values[index] == 0 {
                index += length;
            }
            index += 1;
            continue;
        }
        call.add_assoc_long(STATS[index], values[index]);
        index += 1;
    }
}

fn reload(call: &Call) {
    let Some(serv) = running(call) else { return };
    let Ok(only_task_workers) = call.arity(0, 1).and_then(|()| call.bool(1, false)) else {
        return call.set_bool(false);
    };
    // SAFETY: a live server.
    call.set_bool(unsafe { (host().reload)(serv, !only_task_workers) });
}

unsafe extern "C" fn push_session(array: *mut c_void, session_id: i64) {
    // SAFETY: `heartbeat` passes its return value, an array.
    unsafe { add_next_index_long(array.cast(), session_id) };
}

fn heartbeat(call: &Call) {
    let Some(serv) = running(call) else { return };
    let Ok(close) = call.arity(0, 1).and_then(|()| call.bool(1, false)) else {
        return call.set_bool(false);
    };
    // SAFETY: a live server.
    if unsafe { (host().heartbeat_interval)(serv) } < 1 {
        return call.set_bool(false);
    }
    call.set_array();
    // SAFETY: a live server; each session goes into the return array.
    unsafe { (host().heartbeat)(serv, close, push_session, call.return_value().cast()) };
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
    // SAFETY: a live server.
    call.set_bool(unsafe { (host().bind)(serv, fd, uid) } == 1);
}

fn get_socket(call: &Call) {
    let serv = server(call);
    let Ok(port) = call.arity(0, 1).and_then(|()| call.long(1, 0)) else {
        return call.set_bool(false);
    };
    // SAFETY: a live server and this call's return slot.
    unsafe { (host().socket)(serv, port, call.return_value()) };
}

/// `add_assoc_*(return_value, key, ...)`.
fn add_double(call: &Call, key: &CStr, value: f64) {
    // SAFETY: the return slot holds an array.
    unsafe { add_assoc_double_ex(call.return_value(), key.as_ptr(), key.count_bytes(), value) };
}

fn add_bytes(call: &Call, key: &CStr, value: *const c_char, length: usize) {
    // SAFETY: the return slot holds an array; Zend copies the bytes.
    unsafe {
        add_assoc_stringl_ex(
            call.return_value(),
            key.as_ptr(),
            key.count_bytes(),
            value,
            length,
        );
    }
}

fn get_client_info(call: &Call) {
    let Some(serv) = running(call) else { return };
    let parsed = call.arity(1, 3).and_then(|()| {
        call.long(1, 0)
            .and_then(|fd| Ok((fd, call.long(2, -1)?, call.bool(3, false)?)))
    });
    let Ok((fd, _, _)) = parsed else {
        return call.set_bool(false);
    };
    // SAFETY: all-zero is a valid view for C++ to fill.
    let mut client: ServerClient = unsafe { std::mem::zeroed() };
    // SAFETY: a live server and a writable view.
    if !unsafe { (host().client)(serv, fd, &raw mut client) } {
        return call.set_bool(false);
    }
    call.set_array();
    if client.uid > 0 || client.uid_mode {
        call.add_assoc_long(c"uid", client.uid);
    }
    if client.worker_id > 0 || client.co_conn_lb {
        call.add_assoc_long(c"worker_id", client.worker_id);
    }
    if client.websocket {
        call.add_assoc_long(c"websocket_status", client.websocket_status);
    }
    if !client.ssl_client_cert.is_null() {
        add_bytes(
            call,
            c"ssl_client_cert",
            client.ssl_client_cert,
            client.ssl_client_cert_length,
        );
    }
    if client.server_port >= 0 {
        call.add_assoc_long(c"server_port", client.server_port);
    }
    call.add_assoc_long(c"server_fd", client.server_fd);
    call.add_assoc_long(c"socket_fd", client.socket_fd);
    call.add_assoc_long(c"socket_type", client.socket_type);
    call.add_assoc_long(c"remote_port", client.remote_port);
    // SAFETY: the address C++ formatted, NUL-terminated.
    let ip = unsafe { CStr::from_ptr(client.remote_ip) };
    add_bytes(call, c"remote_ip", ip.as_ptr(), ip.count_bytes());
    call.add_assoc_long(c"reactor_id", client.reactor_id);
    call.add_assoc_long(c"connect_time", client.connect_time);
    call.add_assoc_long(c"last_time", client.last_time);
    add_double(call, c"last_recv_time", client.last_recv_time);
    add_double(call, c"last_send_time", client.last_send_time);
    add_double(call, c"last_dispatch_time", client.last_dispatch_time);
    call.add_assoc_long(c"close_errno", client.close_errno);
    call.add_assoc_long(c"recv_queued_bytes", client.recv_queued_bytes);
    call.add_assoc_long(c"send_queued_bytes", client.send_queued_bytes);
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
    let host = host();
    // SAFETY: a live server.
    let max_fd = unsafe { (host.max_fd)(serv) };
    let start_fd = if start_session_id == 0 {
        // SAFETY: as above.
        unsafe { (host.min_fd)(serv) }
    } else {
        // SAFETY: as above.
        match unsafe { (host.session_fd)(serv, start_session_id) } {
            -1 => return call.set_bool(false),
            fd => fd,
        }
    };
    if start_fd >= max_fd {
        return call.set_bool(false);
    }
    call.set_array();
    for fd in start_fd + 1..=max_fd {
        let (mut session_id, mut server_fd) = (0, 0);
        // SAFETY: a live server and writable locals.
        if unsafe { (host.connection_at)(serv, fd, &raw mut session_id, &raw mut server_fd) } {
            if session_id <= 0 {
                continue;
            }
            // SAFETY: the return slot holds an array.
            unsafe { add_next_index_long(call.return_value(), session_id) };
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
    let host = host();
    // SAFETY: a live server.
    if unsafe { (host.role)(serv, 1) } {
        fatal(
            E_WARNING,
            c"can only be used with base mode and must be within worker process",
        );
        return call.set_bool(false);
    }
    // SAFETY: a live server and the argument's bytes.
    call.set_bool(unsafe { (host.sendwait)(serv, fd, data.as_ptr().cast(), data.len()) });
}

fn exists(call: &Call) {
    let Some(serv) = running(call) else { return };
    let Ok(session_id) = call.arity(1, 1).and_then(|()| call.long(1, 0)) else {
        return call.set_bool(false);
    };
    // SAFETY: a live server.
    call.set_bool(unsafe { (host().exists)(serv, session_id) } == 1);
}

fn protect(call: &Call) {
    let Some(serv) = running(call) else { return };
    let parsed = call
        .arity(1, 2)
        .and_then(|()| Ok((call.long(1, 0)?, call.bool(2, true)?)));
    let Ok((session_id, value)) = parsed else {
        return call.set_bool(false);
    };
    // SAFETY: a live server.
    call.set_bool(unsafe { (host().protect)(serv, session_id, value) });
}

fn get_worker_id(call: &Call) {
    let serv = server(call);
    // SAFETY: a live server.
    if unsafe { (host().role)(serv, 0) } {
        // SAFETY: reads the process's worker id.
        call.set_long(unsafe { (host().worker_id)() });
    } else {
        call.set_bool(false);
    }
}

/// The worker `get*` asks about: the argument, or this process's worker.
fn worker(call: &Call, serv: *mut c_void) -> Option<(i64, i64)> {
    let Ok(worker_id) = call.arity(0, 1).and_then(|()| call.long(1, -1)) else {
        call.set_bool(false);
        return None;
    };
    let worker_id = if worker_id < 0 {
        // SAFETY: reads the process's worker id.
        unsafe { (host().worker_id)() }
    } else {
        worker_id
    };
    let (mut pid, mut status) = (0, 0);
    // SAFETY: a live server and writable locals.
    if unsafe { (host().worker)(serv, worker_id, &raw mut pid, &raw mut status) } {
        Some((pid, status))
    } else {
        call.set_bool(false);
        None
    }
}

fn get_worker_status(call: &Call) {
    let Some(serv) = running(call) else { return };
    if let Some((_, status)) = worker(call, serv) {
        call.set_long(status);
    }
}

fn get_worker_pid(call: &Call) {
    let serv = server(call);
    if let Some((pid, _)) = worker(call, serv) {
        call.set_long(pid);
    }
}

fn get_manager_pid(call: &Call) {
    let serv = server(call);
    // SAFETY: a live server.
    call.set_long(unsafe { (host().pid)(serv, 1) });
}

fn get_master_pid(call: &Call) {
    let serv = server(call);
    // SAFETY: a live server.
    call.set_long(unsafe { (host().pid)(serv, 0) });
}

fn shutdown(call: &Call) {
    let serv = server(call);
    // SAFETY: a live server.
    call.set_bool(unsafe { (host().shutdown)(serv) });
}

fn stop(call: &Call) {
    let Some(serv) = running(call) else { return };
    let Ok(worker_id) = call.arity(0, 1).and_then(|()| call.long(1, -1)) else {
        return call.set_bool(false);
    };
    // SAFETY: a live server.
    call.set_bool(unsafe { (host().kill_worker)(serv, worker_id) });
}

/// `php_swoole_connection_iterator_get_and_check_ptr(ZEND_THIS)`.
fn iterator<'a>(call: &Call) -> &'a mut Iterator {
    // SAFETY: the class's methods are non-static, so `$this` is an iterator.
    let iterator = unsafe { IteratorObject::data(call.this()) };
    if iterator.serv.is_null() {
        // SAFETY: an `E_ERROR`, which does not return; the frames own nothing.
        unsafe { (host().invalid)(call.this()) };
    }
    iterator
}

fn iterator_construct(_: &Call) {
    // SAFETY: in a handler.
    unsafe { throw_error(c"please use the Swoole\\Server->connections") };
}

fn rewind(call: &Call) {
    let iterator = iterator(call);
    iterator.index = 0;
    // SAFETY: a live server.
    iterator.current_fd = unsafe { (host().min_fd)(iterator.serv) };
}

fn valid(call: &Call) {
    let iterator = iterator(call);
    let host = host();
    // SAFETY: a live server.
    let max_fd = unsafe { (host.max_fd)(iterator.serv) };
    for fd in iterator.current_fd..=max_fd {
        let (mut session_id, mut server_fd) = (0, 0);
        // SAFETY: a live server and writable locals.
        if !unsafe {
            (host.connection_at)(iterator.serv, fd, &raw mut session_id, &raw mut server_fd)
        } {
            continue;
        }
        if session_id <= 0 {
            continue;
        }
        if !iterator.port.is_null() {
            // SAFETY: the iterator's live port.
            let port_fd = unsafe { (host.port_fd)(iterator.port) };
            if port_fd < 0 || server_fd != port_fd {
                continue;
            }
        }
        iterator.session_id = session_id;
        iterator.current_fd = fd;
        iterator.index += 1;
        return call.set_bool(true);
    }
    call.set_bool(false);
}

fn current(call: &Call) {
    call.set_long(iterator(call).session_id);
}

fn next(call: &Call) {
    iterator(call).current_fd += 1;
}

fn key(call: &Call) {
    call.set_long(i64::from(iterator(call).index));
}

fn count(call: &Call) {
    let iterator = iterator(call);
    // SAFETY: a live server and its port, or null.
    call.set_long(unsafe { (host().connection_num)(iterator.serv, iterator.port) });
}

/// `offsetExists`/`offsetGet`: the server's own method, called as PHP would.
fn delegate(call: &Call, which: c_int) {
    let serv = iterator(call).serv;
    if call.arity(1, 1).is_err() {
        return call.set_bool(false);
    }
    // SAFETY: a zval is plain data; all-zero bytes are `IS_UNDEF`.
    let mut retval: zval = unsafe { std::mem::zeroed() };
    // SAFETY: a live server, the argument and a writable result.
    let bailed = unsafe { (host().call)(serv, which, call.zval(1), &raw mut retval) };
    if bailed {
        // SAFETY: re-raise the trapped bailout from the handler, which owns nothing.
        unsafe {
            _zend_bailout(
                c"crates/phpserver/src/server.rs".as_ptr().cast_mut(),
                line!(),
            )
        };
    }
    if which == 0 {
        // RETVAL_BOOL(Z_BVAL_P(&retval)): only `true` is true.
        // SAFETY: C++ wrote the method's result.
        call.set_bool(
            unsafe { swoole_rs_zend::type_of(&raw const retval) } == swoole_rs_zend::sys::IS_TRUE,
        );
    } else {
        // RETVAL_ZVAL(&retval, 0, 0): the return slot takes the result.
        call.set_value(retval);
    }
}

fn offset_exists(call: &Call) {
    delegate(call, 0);
}

fn offset_get(call: &Call) {
    delegate(call, 1);
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn layouts_mirror_cxx() {
        // ServerObject { Server *; Property *; zval init_arguments; zend_object std; }
        assert_eq!(Object::<ServerData>::OFFSET, 32);
        // ConnectionIterator { int; SessionId; Server *; ListenPort *; int; }
        assert_eq!(size_of::<Iterator>(), 40);
        assert_eq!(IteratorObject::OFFSET, 40);
        // ServerTaskObject { Server *; DataHead info; zend_object std; }
        assert_eq!(TaskObject::OFFSET, 48);
    }

    #[test]
    fn stats_groups_cover_their_keys() {
        for group in [STATS_WORKER, STATS_QUEUE, STATS_TASKS] {
            assert!(STATS[group].is_empty());
            assert!(!STATS[group + 1].is_empty());
        }
        assert_eq!(STATS.iter().filter(|key| key.is_empty()).count(), 3);
    }
}
