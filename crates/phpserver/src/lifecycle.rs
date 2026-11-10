//! `Swoole\Server`'s object state and lifecycle: what upstream's
//! `ServerProperty` held (the Port objects, user processes, server callbacks and
//! command callables), the object's creation and release, `__construct`,
//! `start()` with `register_callback` and `on_before_start`, `addProcess`,
//! `addCommand` and `command`.
use crate::server::{SERVER, ServerData, fatal, running, server};
use std::{
    cell::RefCell,
    collections::{HashMap, VecDeque},
    ffi::{CStr, c_char, c_int, c_void},
    marker::PhantomData,
    ptr,
    rc::Rc,
    sync::atomic::Ordering,
};
use swoole_rs_process::worker_record::WorkerRecord;
use swoole_rs_server::{
    CommandFn, CommandReply as Reply, server_methods, swr_v1_server_add_command,
    swr_v1_server_command, view,
};
use swoole_rs_zend::{
    Call, Class, E_ERROR, E_WARNING, Message, Object, Value, ZString, bailout, class_name,
    init_object, message, object_of, php_error,
    sys::{
        IS_NULL, IS_OBJECT, add_assoc_bool_ex, add_assoc_long_ex, swr_zend_efree,
        swr_zend_instanceof, swr_zend_json_decode, swr_zend_property_array, swr_zend_property_slot,
        swr_zend_separate_array, swr_zend_try_json_encode, swr_zval_copy, swr_zval_stringl,
        zend_class_entry, zend_hash_next_index_insert, zend_hash_str_find, zend_object,
        zend_object_std_dtor, zend_update_property, zend_update_property_bool,
        zend_update_property_double, zend_update_property_long, zend_update_property_string,
        zend_update_property_stringl, zval, zval_ptr_dtor,
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
pub(crate) const MODE_PROCESS: i64 = 2;
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

unsafe extern "C" {
    /// crates/server's `new Server(mode)`, `delete serv` and the admin commands.
    fn swr_v1_server_new(mode: c_int) -> *mut c_void;
    fn swr_v1_server_delete(serv: *mut c_void);
    fn swr_v1_server_admin_register(serv: *mut c_void, php_memory: extern "C" fn() -> usize);
    /// `Swoole\Process` (crates/phpprocess), and its object's `Worker`; an
    /// `E_ERROR` for an unconstructed one.
    static swoole_process_ce: *mut zend_class_entry;
    fn swr_v1_phpprocess_worker(object: *const zval, check: bool) -> *mut c_void;
    /// `Swoole\Redis\Server` (crates/phpredis).
    static swoole_redis_server_ce: *mut zend_class_entry;
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
fn kind(object: ServerObject) -> c_int {
    let ce = object.class();
    // SAFETY: a class registered at MINIT, read-only since.
    let redis = unsafe { swoole_redis_server_ce };
    let http = swoole_rs_phpswoole::families().map(|families| &families.http);
    let websocket = http.map_or(ptr::null_mut(), |http| (http.websocket_class)());
    let http = http.map_or(ptr::null_mut(), |http| (http.server_class)());
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

/// `serv->start()` inside `zend_try`: the server runs PHP's callbacks, and one
/// may bail out. The trap's `setjmp` sits in C just below `body`, whose frame
/// owns nothing, so a bailout skips no destructor.
fn start_server(serv: *mut c_void) -> Result<c_int, bailout::Bailout> {
    struct Start(*mut c_void, c_int);
    unsafe extern "C" fn body(start: *mut c_void) {
        // SAFETY: `start_server`'s record, live across the trap.
        let start = unsafe { &mut *start.cast::<Start>() };
        // SAFETY: a live server.
        start.1 = unsafe { server_methods::start(start.0) };
    }
    let mut start = Start(serv, 0);
    // SAFETY: as above.
    unsafe { bailout::trap(body, (&raw mut start).cast()) }?;
    Ok(start.1)
}

/// `addProcess`'s worker: -3 bailed out, -2 a thread that is not a user
/// worker, -1 `add_worker` failed, else the id.
fn add_worker(serv: view::Server<'_>, process: *mut zval) -> i64 {
    /// `SW_USER_WORKER`.
    const USER_WORKER: c_int = 5;
    struct Add(*mut c_void, *mut zval, i64);
    unsafe extern "C" fn body(add: *mut c_void) {
        // SAFETY: `add_worker`'s record, live across the trap. A bailout
        // (an unconstructed process) jumps past this frame, which owns nothing.
        let add = unsafe { &mut *add.cast::<Add>() };
        // SAFETY: a live process object.
        let worker = unsafe { swr_v1_phpprocess_worker(add.1, true) }.cast::<WorkerRecord>();
        // SAFETY: a live server that has not started, and the process's worker.
        let id = unsafe { swoole_rs_server::worker_methods::add_worker(add.0, worker) };
        add.2 = if id >= 0 {
            // SAFETY: the process's live worker.
            unsafe { &mut *worker }.set_user_data(add.1.cast());
            id.into()
        } else {
            -1
        };
    }
    if fact(serv, fact::WORKER_THREAD) {
        if crate::server::worker_type() != USER_WORKER {
            return -2;
        }
        let id = swoole_rs_process::methods::swoole_get_worker_id();
        #[expect(clippy::cast_possible_truncation, reason = "get_worker(uint16_t)")]
        let worker = serv.worker(id as u16);
        // SAFETY: the server's live record of this worker.
        let worker = unsafe { &mut *worker };
        worker.redirect_stdin = false;
        worker.redirect_stdout = false;
        worker.redirect_stderr = false;
        return i64::from(id)
            - number(serv, fact::WORKER_NUM)
            - number(serv, fact::TASK_WORKER_NUM);
    }
    let mut add = Add(serv.raw(), process, -3);
    // SAFETY: as above.
    match unsafe { bailout::trap(body, (&raw mut add).cast()) } {
        Ok(()) => add.2,
        Err(_) => -3,
    }
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
    /// Server-level callbacks (callables) by type.
    callbacks: [*mut c_void; CALLBACKS],
    /// `addCommand`'s callables.
    commands: Vec<*mut c_void>,
    /// `send_coroutine_map`: `send_yield`'s waiting coroutines by session.
    send_waiters: HashMap<i64, Waiters>,
    /// The next waiter's token.
    send_token: u64,
}

fn fact(serv: view::Server<'_>, id: u32) -> bool {
    number(serv, id) != 0
}

/// The Server fact `id`: from its records, or C++ for what only it knows.
fn number(serv: view::Server<'_>, id: u32) -> i64 {
    let c = serv.config();
    // `is_hash_dispatch_mode()`: DISPATCH_FDMOD, DISPATCH_IPMOD, DISPATCH_CO_CONN_LB.
    let hash_dispatch = matches!(c.dispatch_mode, 2 | 4 | 8);
    match id {
        fact::MODE => i64::from(c.mode_),
        fact::WORKER_THREAD => i64::from(serv.is_worker_thread()),
        fact::STARTED => i64::from(serv.is_started()),
        // SAFETY: a live server.
        fact::SHUTDOWN => i64::from(serv.counters().shutdown.load(Ordering::Relaxed) != 0),
        fact::MASTER => i64::from(crate::server::worker_type() == crate::server::MASTER),
        fact::THREAD_MODE => i64::from(i64::from(c.mode_) == MODE_THREAD),
        fact::ENABLE_COROUTINE => i64::from(c.enable_coroutine),
        fact::SEND_YIELD => i64::from(c.send_yield),
        fact::HAS_ON_CLOSE => {
            // SAFETY: a live server.
            i64::from(serv.has_callback(swoole_rs_server::callbacks::CB_CLOSE))
        }
        fact::UNSAFE_EVENTS => i64::from(hash_dispatch || c.enable_unsafe_event),
        fact::HASH_DISPATCH => i64::from(hash_dispatch),
        fact::WORKER_NUM => i64::from(c.worker_num),
        fact::TASK_WORKER_NUM => i64::from(c.task_worker_num),
        fact::OUTPUT_BUFFER_SIZE => i64::from(c.output_buffer_size),
        fact::REACTOR => i64::from(!swoole_rs_corebase::thread_reactor().is_null()),
        // SAFETY: a live server.
        fact::MAX_CONNECTION => i64::from(serv.max_connection()),
        fact::CACHE_SESSION_ADDR => i64::from(swoole_rs_phpswoole::families().is_some_and(
            // SAFETY: a live server.
            |families| unsafe { (families.http.can_cache_session_addr)(serv.raw()) },
        )),
        _ => 0,
    }
}

fn put(serv: view::Server<'_>, which: u32) {
    match which {
        install::ALLOCATORS => crate::dispatch::use_zend_strings(serv),
        // SAFETY: a live server that has not started.
        install::ADMIN_COMMANDS => unsafe { swr_v1_server_admin_register(serv.raw(), php_memory) },
        _ => crate::dispatch::install(serv, which),
    }
}

/// `zend_memory_usage(true)`, the admin commands' PHP memory figure.
extern "C" fn php_memory() -> usize {
    // SAFETY: Zend's allocator statistics, read on a PHP thread.
    unsafe { swoole_rs_zend::sys::zend_memory_usage(true) }
}

/// `swoole_process_ce`.
pub(crate) fn process_class() -> *mut zend_class_entry {
    // SAFETY: reads a class registered at MINIT.
    unsafe { swoole_process_ce }
}

/// A live Server object, which `create` made, for as long as the handle is used:
/// its `ServerData` and the `Property` `create` boxed. Each method borrows them
/// only for its own body, and `property`'s closures call nothing that reaches them.
#[derive(Clone, Copy)]
pub(crate) struct ServerObject(*mut zend_object);

impl ServerObject {
    /// # Safety
    /// `object` is a live Server object for as long as the handle is used.
    pub(crate) unsafe fn new(object: *mut zend_object) -> Self {
        Self(object)
    }

    /// A Server method's `$this`.
    pub(crate) fn this(call: &Call) -> Self {
        // SAFETY: the Server class's methods run on its live objects.
        unsafe { Self::new(call.this()) }
    }

    /// The running server's object (`php_swoole_server_zval_ptr`).
    pub(crate) fn running() -> Self {
        let zserv = crate::dispatch::zserv();
        // SAFETY: the server's object zval; the object outlives its running server.
        Self(unsafe { object_of(&raw const zserv) })
    }

    pub(crate) fn raw(self) -> *mut zend_object {
        self.0
    }

    /// The object's class.
    pub(crate) fn class(self) -> *mut zend_class_entry {
        // SAFETY: a live object.
        unsafe { (*self.0).ce }
    }

    fn data(self) -> *mut ServerData {
        // SAFETY: a live Server object; C++ and Rust allocate Servers as `Object<ServerData>`.
        unsafe { Object::<ServerData>::data(self.0) }
    }

    /// The bound `Server`, or null before `__construct`.
    pub(crate) fn serv(self) -> *mut c_void {
        // SAFETY: the live data, read once.
        unsafe { (*self.data()).serv }
    }

    fn set_serv(self, serv: *mut c_void) {
        // SAFETY: the live data, written once.
        unsafe { (*self.data()).serv = serv };
    }

    /// `init_arguments` as the object holds it (no reference taken).
    pub(crate) fn init_arguments(self) -> zval {
        // SAFETY: the live data, read once.
        unsafe { (*self.data()).init_arguments }
    }

    pub(crate) fn set_init_arguments(self, value: zval) {
        // SAFETY: the live data, written once.
        unsafe { (*self.data()).init_arguments = value };
    }

    /// `f` over the property; `f` calls nothing that reaches the object.
    fn property<R>(self, f: impl FnOnce(&mut Property) -> R) -> R {
        // SAFETY: the live data, read once.
        let property = unsafe { (*self.data()).property }.cast::<Property>();
        // SAFETY: `create` set the property and only `free` clears it; nothing
        // else borrows it while `f` runs.
        f(unsafe { &mut *property })
    }

    /// `get_callback(kind)`: a callable, or null.
    fn callback(self, kind: usize) -> *mut c_void {
        self.property(|p| p.callbacks.get(kind).copied())
            .unwrap_or(ptr::null_mut())
    }

    /// `on()`'s server-level callback: replace callback `kind` with `callable`;
    /// false if it is not callable.
    pub(crate) fn set_callback(self, kind: c_int, callable: &Value) -> bool {
        let Ok(kind) = usize::try_from(kind) else {
            return false;
        };
        let taken = self.property(|p| {
            p.callbacks
                .get_mut(kind)
                .map(|slot| std::mem::replace(slot, ptr::null_mut()))
        });
        let Some(old) = taken else {
            return false;
        };
        let free = |unowned: *mut c_void| {
            // SAFETY: a callable no slot holds, or null, which is ignored.
            unsafe { swoole_rs_phpruntime::callable_free(unowned) };
        };
        free(old);
        // SAFETY: a live value.
        let callable = unsafe { swoole_rs_phpruntime::callable_create(callable.as_ptr()) };
        if callable.is_null() {
            return false;
        }
        // A nested on() (from the old callable's destructor) stored its callable,
        // and the property, after this one's write: keep that pair, drop ours.
        free(self.property(|p| match p.callbacks.get_mut(kind) {
            Some(slot) if slot.is_null() => std::mem::replace(slot, callable),
            _ => callable,
        }));
        true
    }

    /// The Port object at `index` (`property->ports.at(index)`), or null.
    pub(crate) fn port_at(self, index: usize) -> *mut zval {
        self.property(|p| p.ports.get(index).copied())
            .unwrap_or(ptr::null_mut())
    }

    /// The user process at `index`, or null.
    pub(crate) fn user_process(self, index: usize) -> *mut zval {
        self.property(|p| p.user_processes.get(index).copied())
            .unwrap_or(ptr::null_mut())
    }
}

/// `server_object->get_callback(kind)`: a callable, or null.
pub(crate) fn callback(kind: c_int) -> *mut c_void {
    ServerObject::running().callback(usize::try_from(kind).unwrap_or(CALLBACKS))
}

/// The Server class's `create_object`.
/// # Safety
/// Zend creates an object of the Server class or a subclass.
pub(crate) unsafe extern "C" fn create(ce: *mut zend_class_entry) -> *mut zend_object {
    // SAFETY: per the caller; all-zero `ServerData` is an unbound server.
    let object = unsafe { Object::<ServerData>::create(&SERVER, ce) };
    // SAFETY: the object just created.
    let data = unsafe { Object::<ServerData>::data(object) };
    data.property = Box::into_raw(Box::<Property>::default()).cast();
    // ZVAL_NULL(&init_arguments)
    data.init_arguments.u1.type_info = IS_NULL;
    object
}

/// The Server class's `free_obj`.
/// # Safety
/// Zend frees a Server object.
pub(crate) unsafe extern "C" fn free(object: *mut zend_object) {
    // SAFETY: per the caller.
    let data = unsafe { Object::<ServerData>::data(object) };
    // SAFETY: `create` boxed it; nothing else frees it.
    let property = unsafe { Box::from_raw(data.property.cast::<Property>()) };
    data.property = ptr::null_mut();
    // SAFETY: the object's live server; the master deletes it below.
    let server = (!data.serv.is_null()).then(|| unsafe { view::Server::from_raw(data.serv) });
    if let Some(server) = server {
        // SAFETY: freeing the object; nothing else holds the config.
        let dispatch = std::mem::replace(
            &mut unsafe { server.config_mut() }.private_data_3,
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
            unsafe { crate::port::deref(object) };
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
    if let Some(server) = server.filter(|&server| fact(server, fact::MASTER)) {
        crate::thread::free_setting(server);
        // SAFETY: the master owns the Server; nothing refers to it now.
        unsafe { swr_v1_server_delete(server.raw()) };
    }
    bailout::finish();
}

/// A live object and a class declaring the properties written, for as long as
/// the handle is used; one [`Props::init`] makes borrows the zval holding it.
/// Zend copies every name and value.
#[derive(Clone, Copy)]
pub(crate) struct Props<'a>(
    *mut zend_class_entry,
    *mut zend_object,
    PhantomData<&'a zval>,
);

impl Props<'static> {
    /// A live object, under its own class.
    /// # Safety
    /// `object` is live for as long as the handle is used.
    pub(crate) unsafe fn of(object: *mut zend_object) -> Self {
        // SAFETY: per the caller.
        Self(unsafe { (*object).ce }, object, PhantomData)
    }

    /// A Server object, under the Server class.
    pub(crate) fn server(object: ServerObject) -> Self {
        Self(SERVER.entry(), object.raw(), PhantomData)
    }

    /// A Server object, under its own class.
    fn own(object: ServerObject) -> Self {
        Self(object.class(), object.raw(), PhantomData)
    }
}

impl<'a> Props<'a> {
    /// `object_init_ex(target, class)`: a new object in `target`, under its class;
    /// the handle borrows `target`, so it can't outlive the object.
    pub(crate) fn init(target: &'a mut zval, class: &Class) -> Self {
        let ce = class.entry();
        // SAFETY: a writable zval and a registered data class.
        unsafe { init_object(target, ce) };
        // SAFETY: the object zval just made.
        Self(ce, unsafe { object_of(target) }, PhantomData)
    }

    pub(crate) fn object(self) -> *mut zend_object {
        self.1
    }

    pub(crate) fn long(self, name: &CStr, value: i64) {
        // SAFETY: the handle's live object and class.
        unsafe {
            zend_update_property_long(self.0, self.1, name.as_ptr(), name.count_bytes(), value);
        }
    }

    pub(crate) fn double(self, name: &CStr, value: f64) {
        // SAFETY: as above.
        unsafe {
            zend_update_property_double(self.0, self.1, name.as_ptr(), name.count_bytes(), value);
        }
    }

    pub(crate) fn string(self, name: &CStr, value: &CStr) {
        // SAFETY: as above.
        unsafe {
            zend_update_property_string(
                self.0,
                self.1,
                name.as_ptr(),
                name.count_bytes(),
                value.as_ptr(),
            );
        }
    }

    pub(crate) fn bytes(self, name: &CStr, value: &[u8]) {
        // SAFETY: as above.
        unsafe {
            zend_update_property_stringl(
                self.0,
                self.1,
                name.as_ptr(),
                name.count_bytes(),
                value.as_ptr().cast(),
                value.len(),
            );
        }
    }

    pub(crate) fn bool(self, name: &CStr, value: bool) {
        // SAFETY: as above.
        unsafe {
            zend_update_property_bool(
                self.0,
                self.1,
                name.as_ptr(),
                name.count_bytes(),
                value.into(),
            );
        }
    }

    /// The property takes a copy of a live value.
    pub(crate) fn set(self, name: &CStr, value: &Value) {
        // SAFETY: as above.
        unsafe {
            zend_update_property(
                self.0,
                self.1,
                name.as_ptr(),
                name.count_bytes(),
                value.as_ptr(),
            );
        }
    }

    pub(crate) fn value(self, name: &CStr, value: &mut zval) {
        // SAFETY: as above, and a live value.
        unsafe { zend_update_property(self.0, self.1, name.as_ptr(), name.count_bytes(), value) };
    }

    /// `host`, `port`, `type`, `sock` (with `fd`) and `ssl` from `info`.
    fn port_info(self, info: &PortInfo, fd: Option<i64>) {
        // SAFETY: as above; the view's bytes live with its port.
        unsafe {
            zend_update_property_stringl(
                self.0,
                self.1,
                c"host".as_ptr(),
                4,
                info.host.data,
                info.host.length,
            );
        }
        self.long(c"port", info.port);
        self.long(c"type", info.kind);
        if let Some(fd) = fd {
            self.long(c"sock", fd);
        }
        self.bool(c"ssl", info.ssl);
    }

    /// A `Connection\Iterator` over `serv` (and `port`, or all of them) in the
    /// object's `connections` property.
    fn connections(self, serv: view::Server<'_>, port: *mut c_void) {
        let mut iterator = crate::dispatch::undef();
        crate::server::new_iterator(&mut iterator, serv.raw(), port);
        self.value(c"connections", &mut iterator);
        // SAFETY: the reference taken above.
        unsafe { zval_ptr_dtor(&raw mut iterator) };
    }
}

/// `php_swoole_server_add_port`: a Port object for `port`, kept by the Server
/// object and appended to `$server->ports`.
/// # Safety
/// `object` is bound to its server, and `port` is one of its ports.
pub(crate) unsafe fn add_port(object: ServerObject, port: *mut c_void) -> *mut zval {
    // SAFETY: per the caller, the object's live server.
    let serv = unsafe { view::Server::from_raw(object.serv()) };
    // SAFETY: a live server and port.
    let zport = unsafe { crate::port::new_object(serv.raw(), port) };
    object.property(|p| p.ports.push(zport));
    // SAFETY: a new Port object zval.
    let port_object = unsafe { object_of(zport) };
    // SAFETY: the Port object, live with the Server object that keeps it.
    let props = unsafe { Props::of(port_object) };
    // SAFETY: a live port.
    let info = unsafe { PortInfo::of(port) };
    props.port_info(&info, Some(info.fd));
    // SAFETY: the Server object; its class declares `ports`.
    let zports =
        unsafe { swr_zend_property_array(object.class(), object.raw(), c"ports".as_ptr(), 5) };
    // SAFETY: an array.
    let zports = unsafe { table(zports) };
    // SAFETY: `add_next_index_zval`: the array takes the Port's reference;
    // `ports` borrows it.
    unsafe { zend_hash_next_index_insert(zports, zport) };
    props.connections(serv, port);
    zport
}

/// `server_ctor`: bind `object` to `serv` and describe its primary port.
fn bind(call: &Call, serv: view::Server<'_>) {
    let object = ServerObject::this(call);
    // *php_swoole_server_zval_ptr(serv) = *zserv
    crate::dispatch::set_server_zval(call.this_zval());
    object.set_serv(serv.raw());
    let mut index = 0;
    while let port = serv.port_at(index)
        && !port.is_null()
    {
        // SAFETY: `$this`, bound above, and one of its ports.
        unsafe { add_port(object, port) };
        index += 1;
    }
    let props = Props::server(object);
    props.connections(serv, ptr::null_mut());
    props.long(c"mode", number(serv, fact::MODE));
    // SAFETY: the primary port.
    props.port_info(&unsafe { PortInfo::of(serv.port_at(0)) }, None);
}

/// `zend_throw_exception_ex(swoole_exception_ce, code, message)`.
fn exception(message: &CStr, code: i64) {
    let ce = swoole_rs_phpswoole::exception_class();
    // SAFETY: from a handler.
    unsafe { throw_exception(ce, message, code) };
}

use crate::server::set_last_error;

fn class(object: ServerObject) -> String {
    // SAFETY: a live object.
    unsafe { class_name(object.raw()) }
        .to_string_lossy()
        .into_owned()
}

fn construct(call: &Call) {
    let object = ServerObject::this(call);
    if !object.serv().is_null() {
        // SAFETY: from a handler, with `$this`.
        unsafe { throw_constructed_twice(object.raw()) };
        return call.set_bool(false);
    }
    if !swoole_rs_phpswoole::globals().cli.get() {
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
    let mut existing = None;
    match crate::thread::admit(&mut existing) {
        0 => {}
        1 => {
            if let Some(server) = existing {
                bind(call, server);
            }
            return;
        }
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
    let serv = unsafe { swr_v1_server_new(c_int::try_from(mode).unwrap_or_default()) };
    // SAFETY: the server just made.
    let server = unsafe { view::Server::from_raw(serv) };
    if mode == MODE_BASE {
        // SAFETY: nothing else holds the new server's config yet.
        let config = unsafe { server.config_mut() };
        config.reactor_num = 1;
        config.worker_num = 1;
    }
    if port == 0 && address.eq_ignore_ascii_case(b"SYSTEMD") {
        if server_methods::add_systemd_socket(server) <= 0 {
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
    bind(call, server);
}

fn add_process(call: &Call) {
    let serv = server(call);
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
    ServerObject::this(call).property(|p| p.user_processes.push(copy));
    match add_worker(serv, copy) {
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
    if !unsafe {
        swr_v1_server_add_command(serv.raw(), name.as_ptr().cast(), name.len(), types, handler)
    } {
        // SAFETY: the callable created above.
        unsafe { swoole_rs_phpruntime::callable_free(callable) };
        return call.set_bool(false);
    }
    ServerObject::this(call).property(|p| p.commands.push(callable));
    call.set_bool(true);
}

/// A command handler (`Server::Command::Handler`): `callable($server, $message)`'s
/// string, or upstream's JSON errors. A bailout is raised last, from this frame.
/// # Safety
/// `callable` is a command callable and `server` the running server; `reply`
/// writes into `out`.
unsafe extern "C" fn command_run(
    callable: *mut c_void,
    _server: *mut c_void,
    message: *const c_char,
    length: usize,
    reply: Reply,
    out: *mut c_void,
) {
    let send = |text: &[u8]| {
        // SAFETY: per the caller.
        unsafe { reply(out, text.as_ptr().cast(), text.len()) };
    };
    let mut args = [crate::dispatch::undef(), crate::dispatch::undef()];
    args[0] = crate::dispatch::zserv();
    // SAFETY: a writable zval and the message's bytes.
    unsafe { swr_zval_stringl(&raw mut args[1], message, length) };
    let mut retval = crate::dispatch::undef();
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
    // SAFETY: the new string the encoder returned.
    let encoded = unsafe { ZString::from_raw(encoded) };
    if let Err(bailout) = swoole_rs_phpruntime::coroutine_check() {
        return bailout.defer();
    }
    let coroutine = swoole_rs_phpcoroutines::current();
    let mut wait = Wait {
        return_value: call.return_value(),
        json_decode,
        coroutine,
        replied: false,
    };
    let data = encoded.bytes();
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
            serv.raw(),
            process_id,
            process_type,
            name.as_ptr().cast(),
            name.len(),
            data.as_ptr().cast(),
            data.len(),
            callback,
        )
    };
    drop(encoded);
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
fn register_callback(object: ServerObject, serv: view::Server<'_>) {
    put(serv, install::CONTROL);
    if !object.callback(ON_TASK).is_null() {
        put(serv, install::TASK);
    }
    if !object.callback(ON_PIPE_MESSAGE).is_null() {
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
    let port = unsafe { crate::port::bound_port(object) };
    // SAFETY: its live ListenPort.
    let c = unsafe { view::port(port) };
    let owner = c.ssl_context;
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

pub(crate) fn isset(serv: view::Server<'_>, port: *mut c_void, kind: c_int) -> bool {
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
fn before_start(object: ServerObject, serv: view::Server<'_>) -> Result<(), Stop> {
    // SAFETY: a live server.
    if unsafe { server_methods::create(serv.raw()) } < 0 {
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
    let ce = object.class();
    Props::own(object).long(c"master_pid", i64::from(std::process::id()));
    // SAFETY: `$this` and its class, which declares `setting`.
    let setting = unsafe { swr_zend_property_array(ce, object.raw(), c"setting".as_ptr(), 7) };
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
    let kind = kind(object);
    let http_server = kind == KIND_HTTP || kind == KIND_WEBSOCKET;
    let ports = object.property(|p| p.ports.clone());
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
    let mut zserv = [crate::dispatch::zserv()];
    // SAFETY: the server's object zval.
    let called = unsafe {
        swoole_rs_phphelpers::named_call(br"\Swoole\Server\Helper::onBeforeStart", &mut zserv)
    };
    trapped(called.is_err())
}

fn start(call: &Call) {
    let serv = server(call);
    let object = ServerObject::this(call);
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
    match start_server(serv.raw()) {
        Err(bailout) => return bailout.defer(),
        Ok(..0) => {
            let error = server_methods::get_startup_error_message(serv.raw());
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
/// # Errors
/// A trapped bailout, for the caller to raise.
pub fn php_swoole_server_send_yield(
    server: view::Server<'_>,
    session_id: i64,
    data: &[u8],
) -> Result<bool, bailout::Bailout> {
    swoole_rs_phpruntime::coroutine_check()?;
    let co = swoole_rs_phpcoroutines::current();
    if data.is_empty() {
        return Ok(false);
    }
    loop {
        let (waiters, token) = ServerObject::running().property(|p| {
            p.send_token += 1;
            (
                Rc::clone(p.send_waiters.entry(session_id).or_default()),
                p.send_token,
            )
        });
        waiters.borrow_mut().push_back((token, co));
        let timeout = server.config().send_timeout;
        // SAFETY: this call's coroutine; `onBufferEmpty` or `onClose` wakes it.
        if !unsafe { swoole_rs_phpcoroutines::suspend_for(co, timeout) } {
            waiters.borrow_mut().retain(|&(waiter, _)| waiter != token);
            return Ok(false);
        }
        #[expect(
            clippy::cast_possible_truncation,
            reason = "Server::send takes a uint32_t length, as upstream's ZSTR_LEN narrowing"
        )]
        let narrowed = data.len() as u32;
        // SAFETY: a live server and the caller's bytes.
        let sent = unsafe {
            server_methods::send(server.raw(), session_id, data.as_ptr().cast(), narrowed)
        };
        let yielded = !sent && swoole_rs_corebase::log::last_error() == SW_ERROR_OUTPUT_SEND_YIELD;
        if yielded && fact(server, fact::SEND_YIELD) {
            continue;
        }
        return Ok(sent);
    }
}

/// Take the session's waiting coroutines out of the running server to wake
/// (`onBufferEmpty` resumes each, `onClose` cancels each): null when none
/// wait. Release with `send_waiters_free`.
pub(crate) fn send_waiters(session_id: i64) -> *mut c_void {
    ServerObject::running()
        .property(|p| p.send_waiters.remove(&session_id))
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
