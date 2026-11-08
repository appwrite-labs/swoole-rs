//! `Swoole\Server`'s object state and lifecycle: what upstream's
//! `ServerProperty` held (the Port objects, user processes, server callbacks and
//! command callables), the object's creation and release, `__construct`,
//! `start()` with `register_callback` and `on_before_start`, `addProcess`,
//! `addCommand` and `command`. C++ keeps the Server, its ports and the C++
//! callbacks it installs (`native/process/swoole_rs_phpserver_lifecycle.h`).
use crate::server::{SERVER, ServerData, fatal, running, server};
use std::{
    cell::RefCell,
    collections::{HashMap, VecDeque},
    ffi::{CStr, c_char, c_int, c_void},
    ptr,
    rc::Rc,
    sync::atomic::{AtomicPtr, Ordering},
};
use swoole_rs_server::{
    CommandFn, CommandReply as Reply, server_methods, swr_v1_server_add_command,
    swr_v1_server_command, view,
};
use swoole_rs_zend::{
    Call, E_ERROR, E_WARNING, Message, Object, bailout, class_name, message, object_of, php_error,
    string::bytes,
    sys::{
        IS_NULL, IS_OBJECT, add_assoc_bool_ex, add_assoc_long_ex, swr_zend_efree,
        swr_zend_instanceof, swr_zend_json_decode, swr_zend_property_array, swr_zend_property_slot,
        swr_zend_separate_array, swr_zend_string_release, swr_zend_try_json_encode, swr_zval_copy,
        swr_zval_stringl, zend_class_entry, zend_hash_next_index_insert, zend_hash_str_find,
        zend_object, zend_object_std_dtor, zend_update_property, zend_update_property_bool,
        zend_update_property_long, zend_update_property_stringl, zval, zval_ptr_dtor,
    },
    throw_constructed_twice, throw_error, throw_exception, type_of,
    value::table,
};

/// `SWR_PHPSERVER_FACT_*`.
mod fact {
    pub(super) const MODE: u32 = 1;
    pub(super) const STARTED: u32 = 2;
    pub(super) const SHUTDOWN: u32 = 3;
    pub(super) const WORKER_THREAD: u32 = 4;
    pub(super) const MASTER: u32 = 6;
    pub(super) const THREAD_MODE: u32 = 7;
    pub(super) const REACTOR: u32 = 8;
    pub(super) const ENABLE_COROUTINE: u32 = 9;
    pub(super) const SEND_YIELD: u32 = 10;
    pub(super) const HAS_ON_CLOSE: u32 = 11;
    pub(super) const UNSAFE_EVENTS: u32 = 12;
    pub(super) const HASH_DISPATCH: u32 = 13;
    pub(super) const CACHE_SESSION_ADDR: u32 = 14;
    pub(super) const WORKER_NUM: u32 = 15;
    pub(super) const TASK_WORKER_NUM: u32 = 16;
    pub(super) const OUTPUT_BUFFER_SIZE: u32 = 17;
    pub(super) const MAX_CONNECTION: u32 = 18;
}

/// `SWR_PHPSERVER_INSTALL_*`.
pub(crate) mod install {
    pub(crate) const CONTROL: u32 = 1;
    pub(crate) const TASK: u32 = 2;
    pub(crate) const PIPE_MESSAGE: u32 = 3;
    pub(crate) const BUFFER_EMPTY: u32 = 4;
    pub(crate) const CLOSE: u32 = 5;
    pub(crate) const HTTP_CLOSE: u32 = 6;
    pub(crate) const RECEIVE: u32 = 7;
    pub(crate) const HTTP_RECEIVE: u32 = 8;
    pub(crate) const REDIS_RECEIVE: u32 = 9;
    pub(crate) const USER_WORKER_START: u32 = 10;
    pub(crate) const ALLOCATORS: u32 = 11;
    pub(crate) const ADMIN_COMMANDS: u32 = 12;
}

/// Protocol flags (`protocols`, `PortFacts`).
const HTTP: u32 = 1;
const WEBSOCKET: u32 = 2;
const HTTP2: u32 = 4;
const REDIS: u32 = 8;

/// `Server::MODE_*`.
const MODE_BASE: i64 = 1;
const MODE_PROCESS: i64 = 2;
pub(crate) const MODE_THREAD: i64 = 3;
/// `SW_SOCK_TCP`.
const SOCK_TCP: i64 = 1;
/// `Server::Command::REACTOR_THREAD`.
const COMMAND_REACTOR_THREAD: i64 = 1 << 2;
/// Callback types (`php_swoole_server_callback_type`, `..._port_callback_type`).
const ON_TASK: usize = 7;
const ON_PIPE_MESSAGE: usize = 13;
const CALLBACKS: usize = 14;
const ON_RECEIVE: c_int = 1;
const ON_PACKET: c_int = 3;
const ON_REQUEST: c_int = 4;
const ON_MESSAGE: c_int = 8;
/// Error codes.
const SW_ERROR_INVALID_PARAMS: c_int = 505;
const SW_ERROR_PHP_RUNTIME_NOTICE: c_int = 510;
const SW_ERROR_OPERATION_NOT_SUPPORT: c_int = 507;
const SW_ERROR_SERVER_UNRELATED_THREAD: c_int = 9016;
/// `host.kind`.
const KIND_HTTP: c_int = 1;
const KIND_WEBSOCKET: c_int = 2;
const KIND_REDIS: c_int = 3;

/// What `start()` checks of a `ListenPort`.
struct PortFacts {
    port: *mut c_void,
    protocols: u32,
    dgram: bool,
    stream: bool,
    ssl: bool,
    ssl_verify_without_ca: bool,
}

/// What the Server and Port objects show of a `ListenPort`.
struct PortInfo {
    host: swoole_rs_corebase::CoreStringView,
    port: i64,
    kind: i64,
    fd: i64,
    ssl: bool,
}

impl PortInfo {
    /// # Safety
    /// `port` is a live `ListenPort`.
    unsafe fn of(port: *mut c_void) -> Self {
        // SAFETY: per the caller.
        let config = unsafe { view::port(port) };
        Self {
            host: config.host(),
            port: config.port.into(),
            kind: config.r#type.into(),
            // SAFETY: per the caller.
            fd: unsafe { view::port_fd(port) }.into(),
            ssl: config.ssl != 0,
        }
    }
}

/// `swr_v1_phpserver_lifecycle_host`.
#[repr(C)]
pub struct LifecycleHost {
    install: unsafe extern "C" fn(*mut c_void, u32),
    create: unsafe extern "C" fn(i64) -> *mut c_void,
    start: unsafe extern "C" fn(*mut c_void, *mut *const c_char) -> c_int,
    release: unsafe extern "C" fn(*mut c_void),
    add_process: unsafe extern "C" fn(*mut c_void, *mut zval) -> i64,
}

unsafe extern "C" {
    /// `Swoole\Process` (crates/phpprocess).
    static swoole_process_ce: *mut zend_class_entry;
    /// `Swoole\Http\Server` and `Swoole\WebSocket\Server` (crates/phphttp).
    static swoole_http_server_ce: *mut zend_class_entry;
    static swoole_websocket_server_ce: *mut zend_class_entry;
    /// `Swoole\Redis\Server` (crates/phpredis).
    static swoole_redis_server_ce: *mut zend_class_entry;
    /// crates/phphttp's `swoole_http_server_can_cache_session_addr(serv)`.
    #[link_name = "_Z41swoole_http_server_can_cache_session_addrPKN6swoole6ServerE"]
    fn can_cache_session_addr(serv: *const c_void) -> bool;
    /// native/error/error.cc's `swoole_get_last_error_msg()`: `sw_error`.
    #[link_name = "_Z25swoole_get_last_error_msgv"]
    fn swoole_get_last_error_msg() -> *const c_char;
}

/// `instanceof_function(ce, parent)`.
fn instance_of(ce: *const zend_class_entry, parent: *const zend_class_entry) -> bool {
    // SAFETY: two registered classes.
    unsafe { swr_zend_instanceof(ce, parent) }
}

/// What kind of Server `object` is: the most derived of `KIND_*`, else 0.
/// # Safety
/// `object` is a live object.
unsafe fn kind(object: *mut zend_object) -> c_int {
    // SAFETY: per the caller.
    let ce = unsafe { (*object).ce };
    // SAFETY: classes registered at MINIT, read-only since.
    let (redis, websocket, http) = unsafe {
        (
            swoole_redis_server_ce,
            swoole_websocket_server_ce,
            swoole_http_server_ce,
        )
    };
    if instance_of(ce, redis) {
        KIND_REDIS
    } else if instance_of(ce, websocket) {
        KIND_WEBSOCKET
    } else if instance_of(ce, http) {
        KIND_HTTP
    } else {
        0
    }
}

static HOST: AtomicPtr<LifecycleHost> = AtomicPtr::new(ptr::null_mut());

fn host() -> &'static LifecycleHost {
    // SAFETY: MINIT installs C++'s static table before any object exists.
    unsafe { &*HOST.load(Ordering::Acquire) }
}

/// MINIT: the table the lifecycle calls.
/// # Safety
/// `table` lives for the process.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn swr_v1_phpserver_lifecycle_init(table: *const LifecycleHost) {
    HOST.store(table.cast_mut(), Ordering::Release);
}

/// `send_yield`'s coroutines waiting on one session, each with its token.
type Waiters = Rc<RefCell<VecDeque<(u64, *mut c_void)>>>;

/// What upstream's `ServerProperty` held.
#[derive(Default)]
pub(crate) struct Property {
    /// The Port objects (`sw_malloc_zval`, borrowed from `$server->ports`).
    ports: Vec<*mut zval>,
    /// `addProcess`'s processes (`emalloc`ed copies holding a reference).
    user_processes: Vec<*mut zval>,
    /// Server-level callbacks (`zend::Callable *`) by type.
    callbacks: [*mut c_void; CALLBACKS],
    /// `addCommand`'s callables.
    commands: Vec<*mut c_void>,
    /// `send_coroutine_map`: `send_yield`'s waiting coroutines by session.
    send_waiters: HashMap<i64, Waiters>,
    /// The next waiter's token.
    send_token: u64,
}

fn fact(serv: *mut c_void, id: u32) -> bool {
    number(serv, id) != 0
}

/// The Server fact `id`: from its records, or C++ for what only it knows.
fn number(serv: *mut c_void, id: u32) -> i64 {
    // SAFETY: a live server.
    let c = unsafe { view::config(serv) };
    // `is_hash_dispatch_mode()`: DISPATCH_FDMOD, DISPATCH_IPMOD, DISPATCH_CO_CONN_LB.
    let hash_dispatch = matches!(c.dispatch_mode, 2 | 4 | 8);
    match id {
        fact::MODE => i64::from(c.mode_),
        // SAFETY: a live server.
        fact::WORKER_THREAD => i64::from(unsafe { view::is_worker_thread(serv) }),
        // SAFETY: a live server.
        fact::STARTED => i64::from(unsafe { view::is_started(serv) }),
        // SAFETY: a live server.
        fact::SHUTDOWN => i64::from(
            unsafe { view::counters(serv) }
                .shutdown
                .load(Ordering::Relaxed)
                != 0,
        ),
        fact::MASTER => i64::from(crate::server::worker_type() == crate::server::MASTER),
        fact::THREAD_MODE => i64::from(i64::from(c.mode_) == MODE_THREAD),
        fact::ENABLE_COROUTINE => i64::from(c.enable_coroutine),
        fact::SEND_YIELD => i64::from(c.send_yield),
        fact::HAS_ON_CLOSE => {
            // SAFETY: a live server.
            i64::from(unsafe { view::has_callback(serv, swoole_rs_server::callbacks::CB_CLOSE) })
        }
        fact::UNSAFE_EVENTS => i64::from(hash_dispatch || c.enable_unsafe_event),
        fact::HASH_DISPATCH => i64::from(hash_dispatch),
        fact::WORKER_NUM => i64::from(c.worker_num),
        fact::TASK_WORKER_NUM => i64::from(c.task_worker_num),
        fact::OUTPUT_BUFFER_SIZE => i64::from(c.output_buffer_size),
        fact::REACTOR => i64::from(!swoole_rs_corebase::thread_reactor().is_null()),
        // SAFETY: a live server.
        fact::MAX_CONNECTION => i64::from(unsafe { view::max_connection(serv) }),
        // SAFETY: a live server.
        fact::CACHE_SESSION_ADDR => i64::from(unsafe { can_cache_session_addr(serv) }),
        _ => 0,
    }
}

fn put(serv: *mut c_void, which: u32) {
    if !crate::dispatch::install(serv, which) {
        // SAFETY: a live server.
        unsafe { (host().install)(serv, which) };
    }
}

/// `swoole_process_ce`.
pub(crate) fn process_class() -> *mut zend_class_entry {
    // SAFETY: reads a class registered at MINIT.
    unsafe { swoole_process_ce }
}

/// The Server object's data.
/// # Safety
/// `object` is a live Server object.
pub(crate) unsafe fn data<'a>(object: *mut zend_object) -> &'a mut ServerData {
    // SAFETY: per the caller; C++ and Rust allocate Servers as `Object<ServerData>`.
    unsafe { Object::<ServerData>::data(object) }
}

/// The Server object's property.
/// # Safety
/// `object` is a live Server object.
unsafe fn property<'a>(object: *mut zend_object) -> &'a mut Property {
    // SAFETY: per the caller.
    let property = unsafe { data(object) }.property.cast::<Property>();
    // SAFETY: `create` set it and only `free` clears it.
    unsafe { &mut *property }
}

/// The running server's object (`php_swoole_server_zval_ptr`).
fn instance(server: *mut c_void) -> *mut zend_object {
    let zserv = crate::dispatch::server_zval(server);
    // SAFETY: the server's object zval.
    unsafe { object_of(&raw const zserv) }
}

/// `server_object->get_callback(kind)`: a `zend::Callable *`, or null.
pub(crate) fn callback(server: *mut c_void, kind: c_int) -> *mut c_void {
    let kind = usize::try_from(kind).unwrap_or(CALLBACKS);
    // SAFETY: the running server's live object.
    unsafe { property(instance(server)) }
        .callbacks
        .get(kind)
        .copied()
        .unwrap_or(ptr::null_mut())
}

/// `on()`'s server-level callback: replace callback `kind` with `callable`;
/// false if it is not callable.
/// # Safety
/// `object` is a live Server object; `callable` a live argument.
pub(crate) unsafe fn set_callback(
    object: *mut zend_object,
    kind: c_int,
    callable: *mut zval,
) -> bool {
    let Some(slot) = usize::try_from(kind)
        .ok()
        // SAFETY: per the caller.
        .and_then(|kind| unsafe { property(object) }.callbacks.get_mut(kind))
    else {
        return false;
    };
    if !slot.is_null() {
        // SAFETY: a callable this property owns.
        unsafe { swoole_rs_phpruntime::callable_free(*slot) };
        *slot = ptr::null_mut();
    }
    // SAFETY: a live argument.
    let callable = unsafe { swoole_rs_phpruntime::callable_create(callable) };
    if callable.is_null() {
        return false;
    }
    *slot = callable;
    true
}

/// The Port object at `index` (`property->ports.at(index)`), or null.
/// # Safety
/// `object` is a live Server object.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn swr_v1_phpserver_port(
    object: *mut zend_object,
    index: usize,
) -> *mut zval {
    // SAFETY: per the caller.
    let ports = &unsafe { property(object) }.ports;
    ports.get(index).copied().unwrap_or(ptr::null_mut())
}

/// The user process at `index` of a live Server object, or null.
pub(crate) fn user_process(object: *mut zend_object, index: usize) -> *mut zval {
    // SAFETY: a live Server object.
    let processes = &unsafe { property(object) }.user_processes;
    processes.get(index).copied().unwrap_or(ptr::null_mut())
}

/// The Server class's `create_object`.
/// # Safety
/// Zend creates an object of the Server class or a subclass.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn swr_v1_phpserver_create(ce: *mut zend_class_entry) -> *mut zend_object {
    // SAFETY: per the caller; all-zero `ServerData` is an unbound server.
    let object = unsafe { Object::<ServerData>::create(&SERVER, ce) };
    // SAFETY: the object just created.
    let data = unsafe { data(object) };
    data.property = Box::into_raw(Box::<Property>::default()).cast();
    // ZVAL_NULL(&init_arguments)
    data.init_arguments.u1.type_info = IS_NULL;
    object
}

/// The Server class's `free_obj`.
/// # Safety
/// Zend frees a Server object.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn swr_v1_phpserver_free(object: *mut zend_object) {
    let host = host();
    // SAFETY: per the caller.
    let data = unsafe { data(object) };
    // SAFETY: `create` boxed it; nothing else frees it.
    let property = unsafe { Box::from_raw(data.property.cast::<Property>()) };
    data.property = ptr::null_mut();
    let serv = data.serv;
    if !serv.is_null() {
        // SAFETY: a live server.
        let dispatch = std::mem::replace(
            &mut unsafe { view::config(serv) }.private_data_3,
            ptr::null_mut(),
        );
        bailout::defer(crate::dispatch::release_dispatch_func(dispatch));
        for &callable in property.callbacks.iter().filter(|cb| !cb.is_null()) {
            // SAFETY: a callable the property owned.
            unsafe { swoole_rs_phpruntime::callable_free(callable) };
        }
        for &process in &property.user_processes {
            // SAFETY: `sw_zval_free`: an emalloc'd zval holding a reference.
            unsafe { zval_ptr_dtor(process) };
            // SAFETY: as above.
            unsafe { swr_zend_efree(process.cast()) };
        }
        for &port in &property.ports {
            // SAFETY: a Port zval `port_object` allocated.
            let object = unsafe { swoole_rs_zend::object_of(port) };
            // SAFETY: its Port object.
            unsafe { crate::port::swr_v1_phpserver_port_deref(object) };
            // SAFETY: the emalloc'd zval, freed once.
            unsafe { swr_zend_efree(port.cast()) };
        }
        data.serv = ptr::null_mut();
    }
    for &callable in &property.commands {
        // SAFETY: a callable the property owned.
        unsafe { swoole_rs_phpruntime::callable_free(callable) };
    }
    drop(property);
    // SAFETY: the object Zend is freeing.
    unsafe { zend_object_std_dtor(object) };
    if !serv.is_null() && fact(serv, fact::MASTER) {
        crate::thread::free_setting(serv);
        // SAFETY: the master owns the Server; nothing refers to it now.
        unsafe { (host.release)(serv) };
    }
    bailout::finish();
}

/// `php_swoole_server_add_port`: a Port object for `port`, kept by the Server
/// object and appended to `$server->ports`.
/// # Safety
/// `object` is a live Server object bound to its server; `port` one of its ports.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn swr_v1_phpserver_add_port(
    object: *mut zend_object,
    port: *mut c_void,
) -> *mut zval {
    // SAFETY: per the caller.
    let serv = unsafe { data(object) }.serv;
    // SAFETY: a live server and port.
    let zport = unsafe { crate::port::new_object(serv, port) };
    // SAFETY: per the caller.
    unsafe { property(object) }.ports.push(zport);
    // SAFETY: a live port.
    let info = unsafe { PortInfo::of(port) };
    // SAFETY: a new Port object.
    let port_object = unsafe { object_of(zport) };
    // SAFETY: as above.
    let port_ce = unsafe { (*port_object).ce };
    // SAFETY: the Port class and object, Zend copies the values.
    unsafe {
        zend_update_property_stringl(
            port_ce,
            port_object,
            c"host".as_ptr(),
            4,
            info.host.data,
            info.host.length,
        );
    }
    for (name, value) in [
        (c"port", info.port),
        (c"type", info.kind),
        (c"sock", info.fd),
    ] {
        // SAFETY: as above.
        unsafe {
            zend_update_property_long(
                port_ce,
                port_object,
                name.as_ptr(),
                name.count_bytes(),
                value,
            );
        }
    }
    // SAFETY: as above.
    unsafe { zend_update_property_bool(port_ce, port_object, c"ssl".as_ptr(), 3, info.ssl.into()) };
    // SAFETY: the live Server object.
    let ce = unsafe { (*object).ce };
    // SAFETY: the Server object; its class declares `ports`.
    let zports = unsafe { swr_zend_property_array(ce, object, c"ports".as_ptr(), 5) };
    // SAFETY: an array.
    let zports = unsafe { table(zports) };
    // SAFETY: `add_next_index_zval`: the array takes the Port's reference;
    // `ports` borrows it.
    unsafe { zend_hash_next_index_insert(zports, zport) };
    // SAFETY: as above.
    unsafe { connections(port_ce, port_object, serv, port) };
    zport
}

/// A `Connection\Iterator` over `serv` (and `port`, or all of them) in the
/// `connections` property of `object`.
/// # Safety
/// `ce` declares `connections` and `object` is a live instance.
unsafe fn connections(
    ce: *mut zend_class_entry,
    object: *mut zend_object,
    serv: *mut c_void,
    port: *mut c_void,
) {
    // SAFETY: a zval is plain data; all-zero bytes are `IS_UNDEF`.
    let mut iterator: zval = unsafe { std::mem::zeroed() };
    crate::server::new_iterator(&mut iterator, serv, port);
    // SAFETY: per the caller; Zend copies the value.
    unsafe { zend_update_property(ce, object, c"connections".as_ptr(), 11, &raw mut iterator) };
    // SAFETY: the reference taken above.
    unsafe { zval_ptr_dtor(&raw mut iterator) };
}

/// `server_ctor`: bind `object` to `serv` and describe its primary port.
fn bind(call: &Call, serv: *mut c_void) {
    let object = call.this();
    // *php_swoole_server_zval_ptr(serv) = *zserv
    crate::dispatch::set_server_zval(serv, call.this_zval());
    // SAFETY: `$this` is a Server.
    unsafe { data(object) }.serv = serv;
    let mut index = 0;
    // SAFETY: a live server.
    while let port = unsafe { view::port_at(serv, index) }
        && !port.is_null()
    {
        // SAFETY: `$this`, bound above, and one of its ports.
        unsafe { swr_v1_phpserver_add_port(object, port) };
        index += 1;
    }
    let ce = SERVER.entry();
    // SAFETY: the Server class and `$this`.
    unsafe { connections(ce, object, serv, ptr::null_mut()) };
    // SAFETY: a live server; the primary port is the first.
    let primary = unsafe { view::port_at(serv, 0) };
    // SAFETY: as above.
    let info = unsafe { PortInfo::of(primary) };
    // SAFETY: the Server class and `$this`; Zend copies the values.
    unsafe { zend_update_property_long(ce, object, c"mode".as_ptr(), 4, number(serv, fact::MODE)) };
    // SAFETY: as above.
    unsafe {
        zend_update_property_stringl(
            ce,
            object,
            c"host".as_ptr(),
            4,
            info.host.data,
            info.host.length,
        );
    }
    // SAFETY: as above.
    unsafe { zend_update_property_long(ce, object, c"port".as_ptr(), 4, info.port) };
    // SAFETY: as above.
    unsafe { zend_update_property_long(ce, object, c"type".as_ptr(), 4, info.kind) };
    // SAFETY: as above.
    unsafe { zend_update_property_bool(ce, object, c"ssl".as_ptr(), 3, info.ssl.into()) };
}

/// `zend_throw_exception_ex(swoole_exception_ce, code, message)`.
fn exception(message: &CStr, code: i64) {
    // SAFETY: Swoole's exception class, registered at MINIT.
    let ce = unsafe { swoole_rs_phpswoole::swoole_exception_ce };
    // SAFETY: from a handler.
    unsafe { throw_exception(ce, message, code) };
}

use crate::server::set_last_error;

fn class(object: *mut zend_object) -> String {
    // SAFETY: a live object.
    unsafe { class_name(object) }.to_string_lossy().into_owned()
}

fn construct(call: &Call) {
    let object = call.this();
    let host = host();
    // SAFETY: `$this` is a Server.
    if !unsafe { data(object) }.serv.is_null() {
        // SAFETY: from a handler, with `$this`.
        unsafe { throw_constructed_twice(object) };
        return call.set_bool(false);
    }
    // SAFETY: this thread's live globals.
    if !unsafe { (*swoole_rs_phpswoole::globals()).cli } {
        let text = message!("{} can only be used in CLI mode", class(object));
        exception(text.as_cstr(), -1);
        return call.set_bool(false);
    }
    let parsed = call.arity(1, 4).and_then(|()| {
        Ok((
            call.string(1, b"")?,
            call.long(2, 0)?,
            call.long(3, MODE_BASE)?,
            call.long(4, SOCK_TCP)?,
        ))
    });
    let Ok((address, port, mode, kind)) = parsed else {
        return call.set_bool(false);
    };
    let thread = crate::server::THREAD.load(Ordering::Relaxed);
    if mode != MODE_BASE && mode != MODE_PROCESS && !(thread && mode == MODE_THREAD) {
        set_last_error(SW_ERROR_INVALID_PARAMS);
        #[expect(
            clippy::cast_possible_truncation,
            reason = "(int) serv_mode, as upstream"
        )]
        let text = message!("invalid $mode parameters {}", mode as i32);
        // SAFETY: from a handler.
        unsafe { throw_error(text.as_cstr()) };
        return call.set_bool(false);
    }
    let mut existing = ptr::null_mut();
    match crate::thread::admit(&mut existing) {
        0 => {}
        1 => return bind(call, existing),
        2 => {
            set_last_error(SW_ERROR_OPERATION_NOT_SUPPORT);
            exception(c"This operation is only allowed in the main thread", -2);
            return call.set_bool(false);
        }
        _ => {
            set_last_error(SW_ERROR_OPERATION_NOT_SUPPORT);
            let text = message!("server is running. unable to create {}", class(object));
            exception(text.as_cstr(), -3);
            return call.set_bool(false);
        }
    }
    // SAFETY: a supported mode.
    let serv = unsafe { (host.create)(mode) };
    if port == 0 && address.eq_ignore_ascii_case(b"SYSTEMD") {
        // SAFETY: a new server.
        if unsafe { server_methods::add_systemd_socket(serv) } <= 0 {
            // SAFETY: from a handler.
            unsafe { throw_error(c"failed to add systemd socket") };
            return call.set_bool(false);
        }
    // SAFETY: a new server and the NUL-terminated `Z_PARAM_STRING` bytes.
    } else if unsafe {
        #[expect(
            clippy::cast_possible_truncation,
            reason = "add_port((SocketType) type, host, (int) port), as upstream"
        )]
        server_methods::add_port(serv, kind as c_int, address.as_ptr().cast(), port as c_int)
    }
    .is_null()
    {
        let code = swoole_rs_corebase::log::last_error();
        let reason = swoole_rs_phpruntime::strerror(code);
        let mut text = Message::new();
        text.push(b"failed to listen server port[");
        text.push(address);
        text.push(format!(":{port}], Error: ").as_bytes());
        text.push(reason.to_bytes());
        text.push(format!("[{code}]").as_bytes());
        exception(text.as_cstr(), code.into());
        return call.set_bool(false);
    }
    bind(call, serv);
}

fn add_process(call: &Call) {
    let serv = server(call);
    let host = host();
    if !fact(serv, fact::WORKER_THREAD) && fact(serv, fact::STARTED) {
        fatal(E_WARNING, c"server is running, cannot add process");
        return call.set_bool(false);
    }
    if call.arity(1, 1).is_err() {
        return call.set_bool(false);
    }
    let process = call.zval(1);
    // SAFETY: a passed argument.
    let is_process = unsafe { type_of(process) } == IS_OBJECT && {
        // SAFETY: an object zval.
        let object = unsafe { object_of(process) };
        // SAFETY: its live object.
        instance_of(unsafe { (*object).ce }, process_class())
    };
    if !is_process {
        fatal(E_ERROR, c"object is not instanceof swoole_process");
        return call.set_bool(false);
    }
    put(serv, install::USER_WORKER_START);
    // SAFETY: an emalloc'd copy holding its own reference, as upstream.
    let copy = unsafe { swoole_rs_zend::sys::swr_zend_emalloc(size_of::<zval>()) }.cast::<zval>();
    // SAFETY: a writable zval and the live argument.
    unsafe { swr_zval_copy(copy, process) };
    // SAFETY: `$this` is a Server.
    unsafe { property(call.this()) }.user_processes.push(copy);
    // SAFETY: a live server and the process.
    let worker_id = unsafe { (host.add_process)(serv, copy) };
    match worker_id {
        -3 => bailout::defer(Err(bailout::Bailout)),
        -2 => {
            set_last_error(SW_ERROR_SERVER_UNRELATED_THREAD);
            call.set_bool(false);
        }
        -1 => {
            fatal(E_WARNING, c"failed to add worker");
            call.set_bool(false);
        }
        id => {
            let process_ce = process_class();
            // SAFETY: a live process object.
            let process = unsafe { object_of(copy) };
            // SAFETY: `Swoole\Process` and its object.
            unsafe { zend_update_property_long(process_ce, process, c"id".as_ptr(), 2, id) };
            call.set_long(id);
        }
    }
}

fn add_command(call: &Call) {
    let serv = server(call);
    if fact(serv, fact::STARTED) {
        fatal(E_WARNING, c"server is running, can't add command");
        return call.set_bool(false);
    }
    let parsed = call
        .arity(3, 3)
        .and_then(|()| Ok((call.string(1, b"")?, call.long(2, 0)?)));
    let Ok((name, types)) = parsed else {
        return call.set_bool(false);
    };
    if types & COMMAND_REACTOR_THREAD != 0 {
        fatal(E_WARNING, c"only support worker/task_worker process");
        return call.set_bool(false);
    }
    // SAFETY: a passed argument.
    let callable = unsafe { swoole_rs_phpruntime::callable_create(call.zval(3)) };
    if callable.is_null() {
        return call.set_bool(false);
    }
    let handler = CommandFn {
        invoke: command_run as *const c_void,
        ctx: callable,
    };
    // Upstream's `(int) types`.
    let types = c_int::from_le_bytes(types.to_le_bytes()[..4].try_into().unwrap_or_default());
    // SAFETY: a live server, the name's bytes and the new callable, which the
    // property keeps for the server's life.
    if !unsafe { swr_v1_server_add_command(serv, name.as_ptr().cast(), name.len(), types, handler) }
    {
        // SAFETY: the callable created above.
        unsafe { swoole_rs_phpruntime::callable_free(callable) };
        return call.set_bool(false);
    }
    // SAFETY: `$this` is a Server.
    unsafe { property(call.this()) }.commands.push(callable);
    call.set_bool(true);
}

/// A command handler (`Server::Command::Handler`): `callable($server, $message)`'s
/// string, or upstream's JSON errors. A bailout is raised last, from this frame.
/// # Safety
/// `callable` is a command callable and `server` the running server; `reply`
/// writes into `out`.
unsafe extern "C" fn command_run(
    callable: *mut c_void,
    server: *mut c_void,
    message: *const c_char,
    length: usize,
    reply: Reply,
    out: *mut c_void,
) {
    let send = |text: &[u8]| {
        // SAFETY: per the caller.
        unsafe { reply(out, text.as_ptr().cast(), text.len()) };
    };
    // SAFETY: a zval is plain data; all-zero bytes are `IS_UNDEF`.
    let mut args: [zval; 2] = unsafe { std::mem::zeroed() };
    args[0] = crate::dispatch::server_zval(server);
    // SAFETY: a writable zval and the message's bytes.
    unsafe { swr_zval_stringl(&raw mut args[1], message, length) };
    // SAFETY: as above.
    let mut retval: zval = unsafe { std::mem::zeroed() };
    let called = crate::dispatch::invoke(callable, &mut args, &raw mut retval, false);
    // SAFETY: the message string made above.
    unsafe { zval_ptr_dtor(&raw mut args[1]) };
    let bailed = match called {
        Err(_) => true,
        Ok(false) => {
            set_last_error(crate::server::SW_ERROR_PHP_FATAL_ERROR);
            // SAFETY: a warning, from a callback C++ runs.
            let warned = unsafe { php_error(E_WARNING, c"Swoole\\Server: command handler error") };
            if warned.is_ok() {
                send(br#"{"data": "failed to call function", "code": -1}"#);
            }
            warned.is_err()
        }
        Ok(true) => {
            // SAFETY: the call's result.
            match unsafe { swoole_rs_zend::string_of(&raw const retval) } {
                Some(text) => send(text),
                None => send(br#"{"data": "wrong return type", "code": -2}"#),
            }
            // SAFETY: as above.
            unsafe { zval_ptr_dtor(&raw mut retval) };
            false
        }
    };
    if bailed {
        bailout::raise(Err(bailout::Bailout));
    }
}

/// `command()`'s wait: the reply lands in `return_value`.
struct Wait {
    return_value: *mut zval,
    json_decode: bool,
    coroutine: *mut c_void,
    replied: bool,
}

/// The command's reply (`Server::Command::Callback`): into `return_value`, then
/// the waiting coroutine resumes, if it has yielded yet.
unsafe extern "C" fn command_done(
    context: *mut c_void,
    _server: *mut c_void,
    message: *const c_char,
    length: usize,
) {
    // SAFETY: `command` passes its live `Wait`.
    let wait = unsafe { &mut *context.cast::<Wait>() };
    if wait.json_decode {
        // SAFETY: the reply's bytes and the call's return slot.
        unsafe { swr_zend_json_decode(message, length, wait.return_value) };
    } else {
        // SAFETY: as above.
        unsafe { swr_zval_stringl(wait.return_value, message, length) };
    }
    // SAFETY: the waiting coroutine.
    if unsafe { swoole_rs_phpcoroutines::is_suspending(wait.coroutine) } {
        // SAFETY: it parked in `command`; this frame owns nothing that needs a drop.
        unsafe { swoole_rs_phpcoroutines::wake(wait.coroutine, false) };
    } else {
        wait.replied = true;
    }
}

fn command(call: &Call) {
    let Some(serv) = running(call) else { return };
    let parsed = call.arity(4, 5).and_then(|()| {
        Ok((
            call.string(1, b"")?,
            call.long(2, 0)?,
            call.long(3, 0)?,
            call.bool(5, true)?,
        ))
    });
    let Ok((name, process_id, process_type, json_decode)) = parsed else {
        return call.set_bool(false);
    };
    let mut encoded = ptr::null_mut();
    // SAFETY: a passed argument and a writable slot.
    if unsafe { swr_zend_try_json_encode(call.zval(4), &raw mut encoded) } != 0 {
        return bailout::defer(Err(bailout::Bailout));
    }
    if encoded.is_null() {
        return call.set_bool(false);
    }
    if let Err(bailout) = swoole_rs_phpruntime::coroutine_check() {
        // SAFETY: the string encoded above.
        unsafe { swr_zend_string_release(encoded) };
        return bailout.defer();
    }
    let coroutine = swoole_rs_phpcoroutines::current();
    let mut wait = Wait {
        return_value: call.return_value(),
        json_decode,
        coroutine,
        replied: false,
    };
    // SAFETY: the encoded string, live until released.
    let data = unsafe { bytes(encoded) };
    let callback = CommandFn {
        invoke: command_done as *const c_void,
        ctx: (&raw mut wait).cast(),
    };
    // Upstream's `(uint16_t) process_id` and `(ProcessType) process_type`.
    let [low, high, ..] = process_id.to_le_bytes();
    let process_id = u32::from(u16::from_le_bytes([low, high]));
    let process_type = c_int::from_le_bytes(
        process_type.to_le_bytes()[..4]
            .try_into()
            .unwrap_or_default(),
    );
    // SAFETY: a live server, the name's and data's bytes, and `wait`, which
    // outlives the command (its callback runs before the yield below returns).
    let sent = unsafe {
        swr_v1_server_command(
            serv,
            process_id,
            process_type,
            name.as_ptr().cast(),
            name.len(),
            data.as_ptr().cast(),
            data.len(),
            callback,
        )
    };
    // SAFETY: the string encoded above.
    unsafe { swr_zend_string_release(encoded) };
    if !sent {
        return call.set_bool(false);
    }
    if !wait.replied {
        // SAFETY: this call's coroutine; `command_done` resumes it.
        unsafe { swoole_rs_phpcoroutines::suspend(coroutine) };
    }
}

/// `php_swoole_error(E_WARNING, ...)`: a notice-coded warning, shown when
/// `display_errors` is on.
pub(crate) fn runtime_warning(text: &CStr) {
    set_last_error(SW_ERROR_PHP_RUNTIME_NOTICE);
    if crate::server::display_errors() {
        // SAFETY: from a handler; a pending bailout waits for its last step.
        bailout::defer(unsafe { php_error(E_WARNING, text) });
    }
}

/// Stop: an `E_ERROR` or a trapped bailout is pending.
struct Stop;

fn stop(level: i32, text: &CStr) -> Result<(), Stop> {
    fatal(level, text);
    Err(Stop)
}

fn trapped(bailed: bool) -> Result<(), Stop> {
    if bailed {
        bailout::defer(Err(bailout::Bailout));
        return Err(Stop);
    }
    Ok(())
}

/// `ServerObject::register_callback`.
fn register_callback(object: *mut zend_object, serv: *mut c_void) {
    put(serv, install::CONTROL);
    // SAFETY: `$this` is a Server.
    let callbacks = &unsafe { property(object) }.callbacks;
    if !callbacks[ON_TASK].is_null() {
        put(serv, install::TASK);
    }
    if !callbacks[ON_PIPE_MESSAGE].is_null() {
        put(serv, install::PIPE_MESSAGE);
    }
    if fact(serv, fact::SEND_YIELD) && fact(serv, fact::UNSAFE_EVENTS) {
        put(serv, install::BUFFER_EMPTY);
    }
}

fn port_facts(zport: *mut zval) -> PortFacts {
    // SAFETY: a live Port object.
    let object = unsafe { object_of(zport) };
    // SAFETY: a bound Port object.
    let port = unsafe { crate::port::swr_v1_phpserver_port_port(object) };
    // SAFETY: its live ListenPort.
    let c = unsafe { view::port(port) };
    // SAFETY: as above.
    let owner = unsafe { (crate::port::host().ssl_owner)(port) };
    PortFacts {
        port,
        protocols: [
            (c.open_http_protocol, HTTP),
            (c.open_websocket_protocol, WEBSOCKET),
            (c.open_http2_protocol, HTTP2),
            (c.open_redis_protocol, REDIS),
        ]
        .iter()
        .filter(|(on, _)| *on)
        .fold(0, |flags, (_, flag)| flags | flag),
        dgram: crate::port_settings::is_dgram(c.r#type),
        // `Socket::is_stream(type)`: TCP, TCP6 or a Unix stream socket.
        stream: matches!(c.r#type, 1 | 3 | 5),
        ssl: c.ssl != 0,
        // SAFETY: null, or the port's live TLS context.
        ssl_verify_without_ca: !owner.is_null()
            && unsafe { swoole_rs_tls::verify_without_ca(owner) },
    }
}

pub(crate) fn isset(serv: *mut c_void, port: *mut c_void, kind: c_int) -> bool {
    // SAFETY: a live server and one of its ports.
    unsafe { crate::instance::php_swoole_server_isset_callback(serv, port, kind) }
}

/// Optionally `clear_protocol()`, then turn on each protocol in `flags`.
fn protocols(port: *mut c_void, flags: u32, clear: bool) {
    if clear {
        // SAFETY: a live port, which only start() touches meanwhile.
        unsafe { swoole_rs_ports::methods::clear_protocol(port) };
    }
    // SAFETY: as above.
    let c = unsafe { swoole_rs_server::port_config::of(port) };
    c.open_http_protocol |= flags & HTTP != 0;
    c.open_websocket_protocol |= flags & WEBSOCKET != 0;
    c.open_http2_protocol |= flags & HTTP2 != 0;
    c.open_redis_protocol |= flags & REDIS != 0;
}

/// `add_assoc_bool(setting, key, value)`.
fn add_bool(setting: *mut zval, key: &CStr, value: bool) {
    // SAFETY: a live, separated array.
    unsafe { add_assoc_bool_ex(setting, key.as_ptr(), key.count_bytes(), value) };
}

fn has_key(setting: *mut zval, key: &CStr) -> bool {
    // SAFETY: a live array.
    let setting = unsafe { table(setting) };
    // SAFETY: a live table and a NUL-terminated key.
    !unsafe { zend_hash_str_find(setting, key.as_ptr(), key.count_bytes()) }.is_null()
}

/// `ServerObject::on_before_start`.
#[expect(clippy::too_many_lines, reason = "upstream's checks, in its order")]
fn before_start(object: *mut zend_object, serv: *mut c_void) -> Result<(), Stop> {
    // SAFETY: a live server.
    if unsafe { server_methods::create(serv) } < 0 {
        // SAFETY: the calling thread's `sw_error`.
        let reason = unsafe { swoole_get_last_error_msg() };
        // SAFETY: always NUL-terminated.
        let reason = unsafe { CStr::from_ptr(reason) };
        let mut text = Message::new();
        text.push(b"failed to create the server. Error: ");
        text.push(reason.to_bytes());
        return stop(E_ERROR, text.as_cstr());
    }
    if fact(serv, fact::ENABLE_COROUTINE) {
        crate::settings::set_reload_async(serv);
    }
    if fact(serv, fact::SEND_YIELD)
        && !fact(serv, fact::HAS_ON_CLOSE)
        && fact(serv, fact::UNSAFE_EVENTS)
    {
        put(serv, install::CLOSE);
    }
    put(serv, install::ALLOCATORS);
    // SAFETY: `$this`.
    let ce = unsafe { (*object).ce };
    let pid = i64::from(std::process::id());
    // SAFETY: `$this` and its class.
    unsafe { zend_update_property_long(ce, object, c"master_pid".as_ptr(), 10, pid) };
    // SAFETY: as above; `setting` is declared.
    let setting = unsafe { swr_zend_property_array(ce, object, c"setting".as_ptr(), 7) };
    // SAFETY: the property's array.
    unsafe { swr_zend_separate_array(setting) };
    for (key, id) in [
        (c"worker_num", fact::WORKER_NUM),
        (c"task_worker_num", fact::TASK_WORKER_NUM),
        (c"output_buffer_size", fact::OUTPUT_BUFFER_SIZE),
        (c"max_connection", fact::MAX_CONNECTION),
    ] {
        if !has_key(setting, key) {
            let value = number(serv, id);
            // SAFETY: a separated array.
            unsafe { add_assoc_long_ex(setting, key.as_ptr(), key.count_bytes(), value) };
        }
    }
    if has_key(setting, c"admin_server") {
        put(serv, install::ADMIN_COMMANDS);
    }
    // SAFETY: `$this`.
    let kind = unsafe { kind(object) };
    let http_server = kind == KIND_HTTP || kind == KIND_WEBSOCKET;
    // SAFETY: `$this` is a Server.
    let ports = unsafe { property(object) }.ports.clone();
    let Some((&first, others)) = ports.split_first() else {
        return Ok(());
    };
    let primary = port_facts(first);
    let mut find_http_port = false;
    if kind == KIND_REDIS {
        add_bool(setting, c"open_redis_protocol", true);
        add_bool(setting, c"open_http_protocol", false);
        add_bool(setting, c"open_mqtt_protocol", false);
        add_bool(setting, c"open_eof_check", false);
        add_bool(setting, c"open_length_check", false);
        protocols(primary.port, REDIS, true);
        put(serv, install::REDIS_RECEIVE);
    } else if http_server {
        if kind == KIND_WEBSOCKET {
            if !isset(serv, primary.port, ON_MESSAGE) {
                return stop(E_ERROR, c"require onMessage callback");
            }
        } else if !isset(serv, primary.port, ON_REQUEST) {
            return stop(E_ERROR, c"require onRequest callback");
        }
        add_bool(setting, c"open_http_protocol", true);
        add_bool(setting, c"open_mqtt_protocol", false);
        add_bool(setting, c"open_eof_check", false);
        add_bool(setting, c"open_length_check", false);
        let mut flags = HTTP;
        if primary.protocols & HTTP2 != 0 {
            add_bool(setting, c"open_http2_protocol", true);
            flags |= HTTP2;
        }
        if primary.protocols & WEBSOCKET != 0 || kind == KIND_WEBSOCKET {
            add_bool(setting, c"open_websocket_protocol", true);
            flags |= WEBSOCKET;
        }
        protocols(primary.port, flags, true);
        find_http_port = true;
        put(serv, install::HTTP_RECEIVE);
    } else {
        if primary.dgram && !primary.ssl && !isset(serv, primary.port, ON_PACKET) {
            return stop(E_ERROR, c"require onPacket callback");
        }
        if ((primary.dgram && primary.ssl) || primary.stream)
            && !isset(serv, primary.port, ON_RECEIVE)
        {
            return stop(E_ERROR, c"require onReceive callback");
        }
        put(serv, install::RECEIVE);
    }
    for &zport in others {
        // SAFETY: a live Port object.
        let port_object = unsafe { object_of(zport) };
        // SAFETY: as above; the Port class declares `setting`.
        let port_setting = unsafe { swr_zend_property_slot(port_object, c"setting".as_ptr(), 7) };
        // SAFETY: a declared property's slot.
        if port_setting.is_null() || unsafe { type_of(port_setting) } == IS_NULL {
            // `$port->set($setting)`, Port's own method.
            // SAFETY: the live Port object, its class and the server's settings.
            let set = unsafe {
                bailout::call_method(
                    port_object,
                    crate::PORT.entry(),
                    b"set",
                    ptr::null_mut(),
                    &[setting],
                )
            };
            trapped(set.is_err())?;
        }
    }
    for &zport in &ports {
        let port = port_facts(zport);
        if port.dgram && !port.ssl && !isset(serv, port.port, ON_PACKET) {
            return stop(E_ERROR, c"require onPacket callback");
        }
        if port.ssl_verify_without_ca {
            return stop(
                E_ERROR,
                c"server open verify peer require `ssl_client_cert_file` or `ssl_capath` or `ssl_cafile` config",
            );
        }
        if port.protocols & HTTP2 != 0 && !fact(serv, fact::HASH_DISPATCH) {
            return stop(
                E_ERROR,
                c"server dispatch mode should be FDMOD(2) or IPMOD(4) if open_http2_protocol is true",
            );
        }
        let mut http = port.protocols & HTTP != 0;
        if !http && port.protocols & (WEBSOCKET | HTTP2) != 0 {
            protocols(port.port, HTTP, false);
            http = true;
        }
        if http {
            find_http_port = true;
            let receive = isset(serv, port.port, ON_RECEIVE);
            if port.protocols & WEBSOCKET != 0 {
                if !isset(serv, port.port, ON_MESSAGE) && !receive {
                    return stop(E_ERROR, c"require onMessage callback");
                }
            } else if !isset(serv, port.port, ON_REQUEST) && !receive {
                return stop(E_ERROR, c"require onRequest callback");
            }
            if !http_server && isset(serv, port.port, ON_REQUEST) {
                let text = message!(
                    "use {} class and open http related protocols may lead to some errors (inconsistent class type)",
                    class(object)
                );
                runtime_warning(text.as_cstr());
                if bailout::pending() {
                    return Err(Stop);
                }
            }
        } else if port.protocols & REDIS == 0 && port.stream && !isset(serv, port.port, ON_RECEIVE)
        {
            return stop(E_ERROR, c"require onReceive callback");
        }
    }
    if find_http_port {
        put(serv, install::HTTP_RECEIVE);
        if fact(serv, fact::CACHE_SESSION_ADDR) {
            put(serv, install::HTTP_CLOSE);
        }
    }
    if crate::server::THREAD.load(Ordering::Relaxed)
        && fact(serv, fact::THREAD_MODE)
        && let Err(bailout) = crate::thread::copy_setting(serv, setting)
    {
        bailout.defer();
        return Err(Stop);
    }
    if !swoole_rs_phpswoole::enable_library() {
        return Ok(());
    }
    let mut zserv = [crate::dispatch::server_zval(serv)];
    // SAFETY: the server's object zval.
    let called = unsafe {
        swoole_rs_phphelpers::named_call(br"\Swoole\Server\Helper::onBeforeStart", &mut zserv)
    };
    trapped(called.is_err())
}

fn start(call: &Call) {
    let serv = server(call);
    let object = call.this();
    let host = host();
    match crate::thread::worker(object, serv) {
        Some(Ok(())) => return call.set_bool(true),
        Some(Err(bailout)) => return bailout.defer(),
        None => {}
    }
    let refusal = if fact(serv, fact::STARTED) {
        Some("The server is running, unable to execute {}->start()")
    } else if fact(serv, fact::SHUTDOWN) {
        Some("The server have been shutdown, unable to execute {}->start()")
    } else if fact(serv, fact::REACTOR) {
        Some("The event-loop has already been created, unable to start {}")
    } else {
        None
    };
    if let Some(format) = refusal {
        let text = message!("{}", format.replace("{}", &class(object)));
        fatal(E_WARNING, text.as_cstr());
        return call.set_bool(false);
    }
    if let Err(bailout) = crate::thread::prepare(object, serv) {
        return bailout.defer();
    }
    register_callback(object, serv);
    if before_start(object, serv).is_err() {
        return;
    }
    let mut error = ptr::null();
    // SAFETY: a live server and a writable out-parameter.
    match unsafe { (host.start)(serv, &raw mut error) } {
        1 => return bailout::defer(Err(bailout::Bailout)),
        -1 => {
            // SAFETY: the startup error message, NUL-terminated.
            let reason = unsafe { CStr::from_ptr(error) };
            let mut text = Message::new();
            text.push(b"failed to start server. Error: ");
            text.push(reason.to_bytes());
            fatal(E_ERROR, text.as_cstr());
            if bailout::pending() {
                return;
            }
        }
        _ => {}
    }
    crate::thread::release();
    call.set_bool(true);
}

/// `SW_ERROR_OUTPUT_SEND_YIELD`.
pub(crate) const SW_ERROR_OUTPUT_SEND_YIELD: c_int = 1010;

/// `php_swoole_server_send_yield`: wait in the session's queue until its
/// output buffer drains, then send; repeat while the send asks to yield again.
/// 0: false, 1: true, 2: a trapped bailout for the C++ caller to re-raise.
/// # Safety
/// `server` is the running server and `data` holds `length` bytes, live while
/// the coroutine waits.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn swr_v1_phpserver_send_yield(
    server: *mut c_void,
    session_id: i64,
    data: *const c_char,
    length: usize,
) -> c_int {
    if swoole_rs_phpruntime::coroutine_check().is_err() {
        return 2;
    }
    let co = swoole_rs_phpcoroutines::current();
    if length == 0 {
        return 0;
    }
    loop {
        // SAFETY: the running server's live object.
        let property = unsafe { property(instance(server)) };
        let waiters = Rc::clone(property.send_waiters.entry(session_id).or_default());
        property.send_token += 1;
        let token = property.send_token;
        waiters.borrow_mut().push_back((token, co));
        // SAFETY: a live server.
        let timeout = unsafe { view::config(server) }.send_timeout;
        // SAFETY: this call's coroutine; `onBufferEmpty` or `onClose` wakes it.
        if !unsafe { swoole_rs_phpcoroutines::suspend_for(co, timeout) } {
            waiters.borrow_mut().retain(|&(waiter, _)| waiter != token);
            return 0;
        }
        #[expect(
            clippy::cast_possible_truncation,
            reason = "Server::send takes a uint32_t length, as upstream's ZSTR_LEN narrowing"
        )]
        let narrowed = length as u32;
        // SAFETY: a live server and the caller's bytes.
        let sent = unsafe { server_methods::send(server, session_id, data.cast(), narrowed) };
        let yielded = !sent && swoole_rs_corebase::log::last_error() == SW_ERROR_OUTPUT_SEND_YIELD;
        if yielded && fact(server, fact::SEND_YIELD) {
            continue;
        }
        return c_int::from(sent);
    }
}

/// Take the session's waiting coroutines out of the running server to wake
/// (`onBufferEmpty` resumes each, `onClose` cancels each): null when none
/// wait. Release with `send_waiters_free`.
pub(crate) fn send_waiters(server: *mut c_void, session_id: i64) -> *mut c_void {
    // SAFETY: the running server's live object.
    let property = unsafe { property(instance(server)) };
    property
        .send_waiters
        .remove(&session_id)
        .map_or(ptr::null_mut(), |waiters| {
            Box::into_raw(Box::new(waiters)).cast()
        })
}

/// The next coroutine to wake, or null: `pop` takes it off the queue (a
/// resume), else it stays first until its `send_yield` leaves (a cancel).
/// `waiters` is from `send_waiters` and not yet freed.
pub(crate) fn send_next(waiters: *mut c_void, pop: bool) -> *mut c_void {
    // SAFETY: per the caller.
    let waiters = unsafe { &*waiters.cast::<Waiters>() };
    let mut queue = waiters.borrow_mut();
    let next = if pop {
        queue.pop_front()
    } else {
        queue.front().copied()
    };
    next.map_or(ptr::null_mut(), |(_, co)| co)
}

/// Release what `send_waiters` took, once.
pub(crate) fn send_waiters_free(waiters: *mut c_void) {
    // SAFETY: per the caller.
    drop(unsafe { Box::from_raw(waiters.cast::<Waiters>()) });
}

fn nothing(_: &Call) {}

swoole_rs_zend::handlers! {
    zim_swoole_server___construct => construct;
    zim_swoole_server___destruct => nothing;
    zim_swoole_server_addProcess => add_process;
    zim_swoole_server_addCommand => add_command;
    zim_swoole_server_start => start;
    zim_swoole_server_command => command;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn property_starts_empty() {
        let property = Property::default();
        assert!(property.callbacks.iter().all(|cb| cb.is_null()));
        assert!(property.ports.is_empty() && property.user_processes.is_empty());
        assert_eq!(property.callbacks.len(), ON_PIPE_MESSAGE + 1);
    }
}
