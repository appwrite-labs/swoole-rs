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
use swoole_rs_server::view;
use swoole_rs_zend::{
    Call, E_ERROR, E_WARNING, Message, Object, bailout, class_name, message, object_of, php_error,
    string::bytes,
    sys::{
        IS_NULL, add_assoc_bool_ex, add_assoc_long_ex, swr_zend_efree, swr_zend_property_array,
        swr_zend_property_slot, swr_zend_separate_array, swr_zend_string_release, swr_zval_copy,
        swr_zval_stringl, zend_class_entry, zend_hash_next_index_insert, zend_hash_str_find,
        zend_object, zend_object_std_dtor, zend_string, zend_update_property,
        zend_update_property_bool, zend_update_property_long, zend_update_property_stringl, zval,
        zval_ptr_dtor,
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
mod install {
    pub(super) const CONTROL: u32 = 1;
    pub(super) const TASK: u32 = 2;
    pub(super) const PIPE_MESSAGE: u32 = 3;
    pub(super) const BUFFER_EMPTY: u32 = 4;
    pub(super) const CLOSE: u32 = 5;
    pub(super) const HTTP_CLOSE: u32 = 6;
    pub(super) const RECEIVE: u32 = 7;
    pub(super) const HTTP_RECEIVE: u32 = 8;
    pub(super) const REDIS_RECEIVE: u32 = 9;
    pub(super) const USER_WORKER_START: u32 = 10;
    pub(super) const ALLOCATORS: u32 = 11;
    pub(super) const ADMIN_COMMANDS: u32 = 12;
}

/// `SWR_PHPSERVER_PROTOCOL_*`.
const HTTP: u32 = 1;
const WEBSOCKET: u32 = 2;
const HTTP2: u32 = 4;
const REDIS: u32 = 8;

/// `Server::MODE_*`.
const MODE_BASE: i64 = 1;
const MODE_PROCESS: i64 = 2;
const MODE_THREAD: i64 = 3;
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

/// `swr_phpserver_port_facts`.
#[repr(C)]
struct PortFacts {
    port: *mut c_void,
    protocols: u32,
    dgram: bool,
    stream: bool,
    ssl: bool,
    ssl_verify_without_ca: bool,
}

/// `swr_phpserver_port_info`.
#[repr(C)]
struct PortInfo {
    host: *const c_char,
    port: i64,
    kind: i64,
    fd: i64,
    ssl: bool,
}

type Reply = unsafe extern "C" fn(*mut c_void, *const c_char, usize);
type Done = unsafe extern "C" fn(*mut c_void, *const c_char, usize) -> *mut c_void;

/// `swr_v1_phpserver_lifecycle_host`.
#[repr(C)]
pub struct LifecycleHost {
    fact: unsafe extern "C" fn(*mut c_void, u32) -> i64,
    install: unsafe extern "C" fn(*mut c_void, u32),
    cli: unsafe extern "C" fn() -> bool,
    last_error: unsafe extern "C" fn() -> c_int,
    strerror: unsafe extern "C" fn(c_int) -> *const c_char,
    admit: unsafe extern "C" fn(*mut *mut c_void) -> c_int,
    create: unsafe extern "C" fn(i64) -> *mut c_void,
    add_systemd_socket: unsafe extern "C" fn(*mut c_void) -> c_int,
    add_port: unsafe extern "C" fn(*mut c_void, i64, *const c_char, i64) -> *mut c_void,
    port_at: unsafe extern "C" fn(*mut c_void, usize) -> *mut c_void,
    port_info: unsafe extern "C" fn(*mut c_void, *mut PortInfo),
    port_object: unsafe extern "C" fn(*mut c_void, *mut c_void) -> *mut zval,
    port_facts: unsafe extern "C" fn(*mut zval, *mut PortFacts),
    set_protocols: unsafe extern "C" fn(*mut c_void, u32, bool),
    isset_callback: unsafe extern "C" fn(*mut c_void, *mut c_void, c_int) -> bool,
    kind: unsafe extern "C" fn(*mut zend_object) -> c_int,
    create_server: unsafe extern "C" fn(*mut c_void) -> *const c_char,
    start: unsafe extern "C" fn(*mut c_void, *mut *const c_char) -> c_int,
    thread_worker: unsafe extern "C" fn(*mut zend_object) -> c_int,
    thread_prepare: unsafe extern "C" fn(*mut zend_object, *mut *mut c_void) -> bool,
    thread_release: unsafe extern "C" fn(*mut c_void),
    copy_setting: unsafe extern "C" fn(*mut c_void, *mut zval),
    release: unsafe extern "C" fn(*mut c_void, bool),
    port_release: unsafe extern "C" fn(*mut zval),
    callable_create: unsafe extern "C" fn(*mut zval) -> *mut c_void,
    callable_free: unsafe extern "C" fn(*mut c_void),
    is_process: unsafe extern "C" fn(*mut zval) -> bool,
    add_process: unsafe extern "C" fn(*mut c_void, *mut zval) -> i64,
    add_command: unsafe extern "C" fn(*mut c_void, *const c_char, usize, i64, *mut c_void) -> bool,
    json_encode: unsafe extern "C" fn(*mut zval, *mut bool) -> *mut zend_string,
    json_decode: unsafe extern "C" fn(*const c_char, usize, *mut zval),
    command: unsafe extern "C" fn(
        *mut c_void,
        i64,
        i64,
        *const c_char,
        usize,
        *const c_char,
        usize,
        Done,
        *mut c_void,
    ) -> bool,
    call: unsafe extern "C" fn(*mut c_void, u32, *mut zval, *mut zval) -> c_int,
    co_current: unsafe extern "C" fn(*mut bool) -> *mut c_void,
    co_suspending: unsafe extern "C" fn(*mut c_void) -> bool,
    co_yield: unsafe extern "C" fn(*mut c_void),
    co_yield_ex: unsafe extern "C" fn(*mut c_void, f64) -> bool,
    send: unsafe extern "C" fn(*mut c_void, i64, *const c_char, usize) -> bool,
    send_timeout: unsafe extern "C" fn(*mut c_void) -> f64,
    process_class: unsafe extern "C" fn() -> *mut zend_class_entry,
    exception_class: unsafe extern "C" fn() -> *mut zend_class_entry,
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
        fact::UNSAFE_EVENTS => i64::from(hash_dispatch || c.enable_unsafe_event),
        fact::HASH_DISPATCH => i64::from(hash_dispatch),
        fact::WORKER_NUM => i64::from(c.worker_num),
        fact::TASK_WORKER_NUM => i64::from(c.task_worker_num),
        fact::OUTPUT_BUFFER_SIZE => i64::from(c.output_buffer_size),
        // SAFETY: a live server.
        _ => unsafe { (host().fact)(serv, id) },
    }
}

fn put(serv: *mut c_void, which: u32) {
    // SAFETY: a live server.
    unsafe { (host().install)(serv, which) };
}

/// The Server object's data.
/// # Safety
/// `object` is a live Server object.
unsafe fn data<'a>(object: *mut zend_object) -> &'a mut ServerData {
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
    let host = host();
    if !slot.is_null() {
        // SAFETY: a callable this property owns.
        unsafe { (host.callable_free)(*slot) };
        *slot = ptr::null_mut();
    }
    // SAFETY: a live argument.
    let callable = unsafe { (host.callable_create)(callable) };
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

/// The user process at `index`, or null.
/// # Safety
/// `object` is a live Server object.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn swr_v1_phpserver_user_process(
    object: *mut zend_object,
    index: usize,
) -> *mut zval {
    // SAFETY: per the caller.
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
        // SAFETY: a live server: its dispatch_func callable.
        unsafe { (host.release)(serv, false) };
        for &callable in property.callbacks.iter().filter(|cb| !cb.is_null()) {
            // SAFETY: a callable the property owned.
            unsafe { (host.callable_free)(callable) };
        }
        for &process in &property.user_processes {
            // SAFETY: `sw_zval_free`: an emalloc'd zval holding a reference.
            unsafe { zval_ptr_dtor(process) };
            // SAFETY: as above.
            unsafe { swr_zend_efree(process.cast()) };
        }
        for &port in &property.ports {
            // SAFETY: a Port zval `port_object` allocated.
            unsafe { (host.port_release)(port) };
        }
        data.serv = ptr::null_mut();
    }
    for &callable in &property.commands {
        // SAFETY: a callable the property owned.
        unsafe { (host.callable_free)(callable) };
    }
    drop(property);
    // SAFETY: the object Zend is freeing.
    unsafe { zend_object_std_dtor(object) };
    if !serv.is_null() && fact(serv, fact::MASTER) {
        // SAFETY: the master owns the Server; nothing refers to it now.
        unsafe { (host.release)(serv, true) };
    }
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
    let host = host();
    // SAFETY: per the caller.
    let serv = unsafe { data(object) }.serv;
    // SAFETY: a live server and port.
    let zport = unsafe { (host.port_object)(serv, port) };
    // SAFETY: per the caller.
    unsafe { property(object) }.ports.push(zport);
    // SAFETY: C++ wrote every field.
    let mut info: PortInfo = unsafe { std::mem::zeroed() };
    // SAFETY: a live port and a writable view.
    unsafe { (host.port_info)(port, &raw mut info) };
    // SAFETY: a new Port object.
    let port_object = unsafe { object_of(zport) };
    // SAFETY: as above.
    let port_ce = unsafe { (*port_object).ce };
    // SAFETY: the port's NUL-terminated host.
    let address = unsafe { CStr::from_ptr(info.host) };
    // SAFETY: the Port class and object, Zend copies the values.
    unsafe {
        zend_update_property_stringl(
            port_ce,
            port_object,
            c"host".as_ptr(),
            4,
            address.as_ptr(),
            address.count_bytes(),
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
    let host = host();
    // *php_swoole_server_zval_ptr(serv) = *zserv
    crate::dispatch::set_server_zval(serv, call.this_zval());
    // SAFETY: `$this` is a Server.
    unsafe { data(object) }.serv = serv;
    let mut index = 0;
    // SAFETY: a live server.
    while let port = unsafe { (host.port_at)(serv, index) }
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
    let primary = unsafe { (host.port_at)(serv, 0) };
    // SAFETY: C++ writes every field.
    let mut info: PortInfo = unsafe { std::mem::zeroed() };
    // SAFETY: a live port and a writable view.
    unsafe { (host.port_info)(primary, &raw mut info) };
    // SAFETY: the port's NUL-terminated host.
    let address = unsafe { CStr::from_ptr(info.host) };
    // SAFETY: the Server class and `$this`; Zend copies the values.
    unsafe { zend_update_property_long(ce, object, c"mode".as_ptr(), 4, number(serv, fact::MODE)) };
    // SAFETY: as above.
    unsafe {
        zend_update_property_stringl(
            ce,
            object,
            c"host".as_ptr(),
            4,
            address.as_ptr(),
            address.count_bytes(),
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
    let ce = unsafe { (host().exception_class)() };
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
    // SAFETY: reads a global.
    if !unsafe { (host.cli)() } {
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
    // SAFETY: a writable out-parameter.
    match unsafe { (host.admit)(&raw mut existing) } {
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
        if unsafe { (host.add_systemd_socket)(serv) } <= 0 {
            // SAFETY: from a handler.
            unsafe { throw_error(c"failed to add systemd socket") };
            return call.set_bool(false);
        }
    // SAFETY: a new server and the NUL-terminated `Z_PARAM_STRING` bytes.
    } else if unsafe { (host.add_port)(serv, kind, address.as_ptr().cast(), port) }.is_null() {
        // SAFETY: plain state.
        let code = unsafe { (host.last_error)() };
        // SAFETY: plain state.
        let reason = unsafe { (host.strerror)(code) };
        // SAFETY: Swoole's static error strings.
        let reason = unsafe { CStr::from_ptr(reason) };
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
    if !unsafe { (host.is_process)(process) } {
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
            // SAFETY: registered at MINIT.
            let process_ce = unsafe { (host.process_class)() };
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
    let host = host();
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
    let callable = unsafe { (host.callable_create)(call.zval(3)) };
    if callable.is_null() {
        return call.set_bool(false);
    }
    // SAFETY: a live server, the name's bytes and the new callable.
    if !unsafe { (host.add_command)(serv, name.as_ptr().cast(), name.len(), types, callable) } {
        // SAFETY: the callable created above.
        unsafe { (host.callable_free)(callable) };
        return call.set_bool(false);
    }
    // SAFETY: `$this` is a Server.
    unsafe { property(call.this()) }.commands.push(callable);
    call.set_bool(true);
}

/// A command handler's call (`Server::Command::Handler`): `callable($server,
/// $message)`'s string, or upstream's JSON errors. True if it bailed out.
/// # Safety
/// `callable` is a command callable and `server` the running server; `reply`
/// writes into `out`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn swr_v1_phpserver_command_run(
    callable: *mut c_void,
    server: *mut c_void,
    message: *const c_char,
    length: usize,
    reply: Reply,
    out: *mut c_void,
) -> bool {
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
    // SAFETY: a live callable and two arguments.
    let called = unsafe { (host().call)(callable, 2, args.as_mut_ptr(), &raw mut retval) };
    // SAFETY: the message string made above.
    unsafe { zval_ptr_dtor(&raw mut args[1]) };
    match called {
        2 => true,
        0 => {
            set_last_error(crate::server::SW_ERROR_PHP_FATAL_ERROR);
            // SAFETY: a warning, from a callback C++ runs.
            if unsafe { php_error(E_WARNING, c"Swoole\\Server: command handler error") }.is_err() {
                return true;
            }
            send(br#"{"data": "failed to call function", "code": -1}"#);
            false
        }
        _ => {
            // SAFETY: the call's result.
            match unsafe { swoole_rs_zend::string_of(&raw const retval) } {
                Some(text) => send(text),
                None => send(br#"{"data": "wrong return type", "code": -2}"#),
            }
            // SAFETY: as above.
            unsafe { zval_ptr_dtor(&raw mut retval) };
            false
        }
    }
}

/// `command()`'s wait: the reply lands in `return_value`.
struct Wait {
    return_value: *mut zval,
    json_decode: bool,
    coroutine: *mut c_void,
    replied: bool,
}

/// The command's reply: into `return_value`, then the coroutine for C++ to
/// resume (null: it has not yielded yet).
unsafe extern "C" fn command_done(
    context: *mut c_void,
    message: *const c_char,
    length: usize,
) -> *mut c_void {
    // SAFETY: `command` passes its live `Wait`.
    let wait = unsafe { &mut *context.cast::<Wait>() };
    let host = host();
    if wait.json_decode {
        // SAFETY: the reply's bytes and the call's return slot.
        unsafe { (host.json_decode)(message, length, wait.return_value) };
    } else {
        // SAFETY: as above.
        unsafe { swr_zval_stringl(wait.return_value, message, length) };
    }
    // SAFETY: the waiting coroutine.
    if unsafe { (host.co_suspending)(wait.coroutine) } {
        return wait.coroutine;
    }
    wait.replied = true;
    ptr::null_mut()
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
    let host = host();
    let mut bailed = false;
    // SAFETY: a passed argument and a writable flag.
    let encoded = unsafe { (host.json_encode)(call.zval(4), &raw mut bailed) };
    if bailed {
        return bailout::defer(Err(bailout::Bailout));
    }
    if encoded.is_null() {
        return call.set_bool(false);
    }
    // SAFETY: a writable flag.
    let coroutine = unsafe { (host.co_current)(&raw mut bailed) };
    if bailed {
        // SAFETY: the string encoded above.
        unsafe { swr_zend_string_release(encoded) };
        return bailout::defer(Err(bailout::Bailout));
    }
    let mut wait = Wait {
        return_value: call.return_value(),
        json_decode,
        coroutine,
        replied: false,
    };
    // SAFETY: the encoded string, live until released.
    let data = unsafe { bytes(encoded) };
    // SAFETY: a live server, the name's and data's bytes, and `wait`, which
    // outlives the command (its callback runs before the yield below returns).
    let sent = unsafe {
        (host.command)(
            serv,
            process_id,
            process_type,
            name.as_ptr().cast(),
            name.len(),
            data.as_ptr().cast(),
            data.len(),
            command_done,
            (&raw mut wait).cast(),
        )
    };
    // SAFETY: the string encoded above.
    unsafe { swr_zend_string_release(encoded) };
    if !sent {
        return call.set_bool(false);
    }
    if !wait.replied {
        // SAFETY: this call's coroutine; `command_done` resumes it.
        unsafe { (host.co_yield)(coroutine) };
    }
}

/// `php_swoole_error(E_WARNING, ...)`: a notice-coded warning, shown when
/// `display_errors` is on.
fn runtime_warning(text: &CStr) {
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
    // SAFETY: C++ writes every field.
    let mut facts: PortFacts = unsafe { std::mem::zeroed() };
    // SAFETY: a live Port object and a writable view.
    unsafe { (host().port_facts)(zport, &raw mut facts) };
    facts
}

fn isset(serv: *mut c_void, port: *mut c_void, kind: c_int) -> bool {
    // SAFETY: a live server and one of its ports.
    unsafe { (host().isset_callback)(serv, port, kind) }
}

fn protocols(port: *mut c_void, flags: u32, clear: bool) {
    // SAFETY: a live port.
    unsafe { (host().set_protocols)(port, flags, clear) };
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
    let host = host();
    // SAFETY: a live server.
    let error = unsafe { (host.create_server)(serv) };
    if !error.is_null() {
        // SAFETY: `sw_error`, NUL-terminated.
        let reason = unsafe { CStr::from_ptr(error) };
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
    let kind = unsafe { (host.kind)(object) };
    let http_server = kind == KIND_HTTP || kind == KIND_WEBSOCKET;
    // SAFETY: `$this` is a Server.
    let ports = unsafe { property(object) }.ports.clone();
    let primary = port_facts(ports[0]);
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
    for &zport in &ports[1..] {
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
    if crate::server::THREAD.load(Ordering::Relaxed) && fact(serv, fact::THREAD_MODE) {
        // SAFETY: a live server and the settings array.
        unsafe { (host.copy_setting)(serv, setting) };
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
    // SAFETY: `$this`.
    match unsafe { (host.thread_worker)(object) } {
        1 => return call.set_bool(true),
        2 => return bailout::defer(Err(bailout::Bailout)),
        _ => {}
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
    let mut state = ptr::null_mut();
    // SAFETY: `$this` and a writable out-parameter.
    if unsafe { (host.thread_prepare)(object, &raw mut state) } {
        return bailout::defer(Err(bailout::Bailout));
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
    // SAFETY: what `thread_prepare` made, or null.
    unsafe { (host.thread_release)(state) };
    call.set_bool(true);
}

/// `SW_ERROR_OUTPUT_SEND_YIELD`.
const SW_ERROR_OUTPUT_SEND_YIELD: c_int = 1010;

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
    let host = host();
    let mut bailed = false;
    // SAFETY: a writable flag.
    let co = unsafe { (host.co_current)(&raw mut bailed) };
    if bailed {
        return 2;
    }
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
        let timeout = unsafe { (host.send_timeout)(server) };
        // SAFETY: this call's coroutine; `onBufferEmpty` or `onClose` wakes it.
        if !unsafe { (host.co_yield_ex)(co, timeout) } {
            waiters.borrow_mut().retain(|&(waiter, _)| waiter != token);
            return 0;
        }
        // SAFETY: a live server and the caller's bytes.
        let sent = unsafe { (host.send)(server, session_id, data, length) };
        // SAFETY: plain state.
        let yielded = !sent && unsafe { (host.last_error)() } == SW_ERROR_OUTPUT_SEND_YIELD;
        if yielded && fact(server, fact::SEND_YIELD) {
            continue;
        }
        return c_int::from(sent);
    }
}

/// Take the session's waiting coroutines out of the server for C++ to wake
/// (`onBufferEmpty` resumes each, `onClose` cancels each): null when none
/// wait. Release with `swr_v1_phpserver_send_waiters_free`.
/// # Safety
/// `server` is the running server.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn swr_v1_phpserver_send_waiters(
    server: *mut c_void,
    session_id: i64,
) -> *mut c_void {
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
/// # Safety
/// `waiters` is from `swr_v1_phpserver_send_waiters` and not yet freed.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn swr_v1_phpserver_send_next(
    waiters: *mut c_void,
    pop: bool,
) -> *mut c_void {
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

/// Release what `swr_v1_phpserver_send_waiters` took.
/// # Safety
/// `waiters` is from `swr_v1_phpserver_send_waiters`, freed once.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn swr_v1_phpserver_send_waiters_free(waiters: *mut c_void) {
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

    #[test]
    fn layouts_mirror_cxx() {
        assert_eq!(size_of::<PortFacts>(), 16);
        assert_eq!(size_of::<PortInfo>(), 40);
    }
}
