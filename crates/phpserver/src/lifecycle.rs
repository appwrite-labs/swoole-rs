//! `Swoole\Server`'s object state and lifecycle: what upstream's
//! `ServerProperty` held (the Port objects, user processes, server callbacks and
//! command callables), the object's creation and release, `__construct`,
//! `start()` with `register_callback` and `on_before_start`, `addProcess`,
//! `addCommand` and `command`.
use crate::{
    callable::Callable,
    server::{SERVER, fatal, running, server},
};
use std::{
    cell::{Cell, OnceCell, RefCell},
    collections::{HashMap, VecDeque},
    ffi::{CStr, CString, c_int, c_void},
    ptr,
    rc::{Rc, Weak},
    sync::atomic::Ordering,
};
use swoole_rs_corebase::CCast;
use swoole_rs_phpcoroutines::Coroutine;
use swoole_rs_process::worker_record::Worker;
use swoole_rs_server::{server_methods, swr_v1_server_add_command, swr_v1_server_command, view};
use swoole_rs_zend::host::callable::Callable as Kept;
use swoole_rs_zend::{
    ArrayMut, Call, Class, Custom, CustomClass, E_ERROR, E_WARNING, Message, Owned, Value, ZString,
    bailout, message, report, sys::zval,
};

/// What [`fact`] answers of a server.
#[derive(Clone, Copy)]
enum Fact {
    Started,
    Shutdown,
    WorkerThread,
    Master,
    ThreadMode,
    Reactor,
    EnableCoroutine,
    SendYield,
    HasOnClose,
    UnsafeEvents,
    HashDispatch,
    CacheSessionAddr,
}

/// The callback groups [`put`] installs on a server.
#[derive(Clone, Copy)]
pub(crate) enum Install {
    Control,
    Task,
    PipeMessage,
    BufferEmpty,
    Close,
    HttpClose,
    Receive,
    HttpReceive,
    RedisReceive,
    UserWorkerStart,
    Allocators,
    AdminCommands,
}

/// Protocol flags (`protocols`, `PortFacts`).
const HTTP: u32 = 1;
const WEBSOCKET: u32 = 2;
const HTTP2: u32 = 4;
const REDIS: u32 = 8;

/// `SW_SOCK_TCP`.
const SOCK_TCP: i64 = 1;
/// `Server::Command::REACTOR_THREAD`.
const COMMAND_REACTOR_THREAD: i64 = 1 << 2;
/// The server's own callbacks (`php_swoole_server_callback_type`), in slot order.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(usize)]
pub(crate) enum ServerCallback {
    Start,
    BeforeShutdown,
    Shutdown,
    WorkerStart,
    WorkerStop,
    BeforeReload,
    AfterReload,
    Task,
    Finish,
    WorkerExit,
    WorkerError,
    ManagerStart,
    ManagerStop,
    PipeMessage,
}
const CALLBACKS: usize = ServerCallback::PipeMessage as usize + 1;
pub(crate) use swoole_rs_ports::PortCallback;
/// Error codes.
const SW_ERROR_INVALID_PARAMS: c_int = 505;
const SW_ERROR_PHP_RUNTIME_NOTICE: c_int = 510;
const SW_ERROR_OPERATION_NOT_SUPPORT: c_int = 507;
const SW_ERROR_SERVER_UNRELATED_THREAD: c_int = 9016;
/// What kind of Server an object is, its most derived class.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Kind {
    Server,
    Http,
    WebSocket,
    Redis,
}

/// What `start()` checks of a `ListenPort`.
struct PortFacts<'a> {
    port: view::Port<'a>,
    protocols: u32,
    dgram: bool,
    stream: bool,
    ssl: bool,
    ssl_verify_without_ca: bool,
}

/// What the Server and Port objects show of a `ListenPort`.
struct PortInfo {
    host: Vec<u8>,
    port: i64,
    kind: i64,
    fd: i64,
    ssl: bool,
}

impl PortInfo {
    fn of(port: view::Port<'_>) -> Self {
        let config = port.config();
        Self {
            host: config.host_bytes().to_vec(),
            port: config.port.get().into(),
            kind: config.r#type.into(),
            fd: port.fd().into(),
            ssl: config.ssl != 0,
        }
    }
}

// crates/server's `new Server(mode)`, `delete serv` and the admin commands;
// `Swoole\Process` (crates/phpprocess) and its object's `Worker`, an `E_ERROR` for an
// unconstructed one; and a copy of `sw_error`.
use swoole_rs_error::last_error_message;
use swoole_rs_phpprocess::checked_worker;
use swoole_rs_server::{Mode, SharedServer, admin_register};

/// What kind of Server `object` is.
fn kind(object: ServerObject<'_>) -> Kind {
    let value = object.value();
    let http = swoole_rs_phpswoole::families().map(|families| &families.http);
    if value.instance_of(&crate::server::REDIS_SERVER) {
        Kind::Redis
    } else if http.is_some_and(|http| value.instance_of((http.websocket_class)())) {
        Kind::WebSocket
    } else if http.is_some_and(|http| value.instance_of((http.server_class)())) {
        Kind::Http
    } else {
        Kind::Server
    }
}

/// `serv->start()` inside `zend_try`: the server runs PHP's callbacks, and one
/// may bail out. The trap's `setjmp` sits in C just below `body`, whose frame
/// owns nothing, so a bailout skips no destructor.
fn start_server(serv: view::Server<'_>) -> Result<c_int, bailout::Bailout> {
    let mut started = 0;
    // SAFETY: as above; the closure holds a view and a borrow, nothing to drop.
    unsafe { bailout::trap_with(|| started = server_methods::start(serv)) }?;
    Ok(started)
}

/// `addProcess`'s worker: -3 bailed out, -2 a thread that is not a user
/// worker, -1 `add_worker` failed, else the id.
fn add_worker(serv: view::Server<'_>, process: &Value, user_data: *mut zval) -> i64 {
    if fact(serv, Fact::WorkerThread) {
        if crate::server::worker_type() != Some(crate::server::WorkerType::User) {
            return -2;
        }
        let id = swoole_rs_process::methods::swoole_get_worker_id();
        if let Some(worker) = serv.worker_record(id.c_cast::<u16>()) {
            worker.set_redirect_stdin(false);
            worker.set_redirect_stdout(false);
            worker.set_redirect_stderr(false);
        }
        let config = serv.config();
        return i64::from(id)
            - i64::from(config.worker_num.get())
            - i64::from(config.task_worker_num.get());
    }
    let mut added = -3;
    let add = || {
        // None once the error an unconstructed process raised returns.
        let Some(worker) = checked_worker(process) else {
            return;
        };
        // SAFETY: the process's live worker, which the server's copy of the
        // Process object keeps for the server's life.
        let worker = unsafe { Worker::new(worker.raw()) };
        let id = swoole_rs_server::worker_methods::add_worker(serv, worker);
        added = if id >= 0 {
            worker.set_user_data(user_data.cast());
            id.into()
        } else {
            -1
        };
    };
    // SAFETY: a bailout (an unconstructed process) jumps past `add`, which
    // holds a view, a pointer and a borrow: nothing to drop.
    let trapped = unsafe { bailout::trap_with(add) };
    trapped.map_or(-3, |()| added)
}

/// `send_yield`'s coroutines waiting on one session, each with its token.
pub(crate) type Waiters = Rc<RefCell<VecDeque<(u64, Coroutine)>>>;

/// `send_coroutine_map`: `send_yield`'s waiting coroutines by session.
type SendWaiters = HashMap<i64, Waiters>;

/// What upstream's `ServerProperty` held.
pub(crate) struct Property {
    /// The Port objects, each held by a reference of its own.
    ports: Vec<Owned>,
    /// Server-level callbacks by type.
    callbacks: [Option<Callable>; CALLBACKS],
    /// `addCommand`'s callables.
    commands: Vec<Callable>,
    send_waiters: SendWaiters,
    /// The next waiter's token.
    send_token: u64,
    /// `addProcess`'s processes, copies holding a reference; boxed, as each
    /// worker's user data points at its copy.
    #[expect(clippy::vec_box, reason = "a worker's user data points at its copy")]
    user_processes: Vec<Box<Owned>>,
}

impl Default for Property {
    /// The fields whose drops reach Zend are constants, built after every call
    /// that could unwind: nothing drops them on the way, which would link Zend's
    /// destructors into tests that have no PHP.
    fn default() -> Self {
        Self {
            send_waiters: SendWaiters::default(),
            send_token: 0,
            ports: const { Vec::new() },
            callbacks: [const { None }; CALLBACKS],
            commands: const { Vec::new() },
            user_processes: const { Vec::new() },
        }
    }
}

/// Fact `id` of `serv`: from its records, or the PHP layer's for what only it knows.
fn fact(serv: view::Server<'_>, id: Fact) -> bool {
    let c = serv.config();
    match id {
        Fact::WorkerThread => serv.is_worker_thread(),
        Fact::Started => serv.is_started(),
        Fact::Shutdown => serv.counters().shutdown.load(Ordering::Relaxed) != 0,
        Fact::Master => crate::server::worker_type() == Some(crate::server::WorkerType::Master),
        Fact::ThreadMode => c.mode_ == Mode::Thread,
        Fact::EnableCoroutine => c.enable_coroutine.get(),
        Fact::SendYield => c.send_yield.get(),
        Fact::HasOnClose => serv.has_callback(swoole_rs_server::callbacks::CallbackKind::Close),
        Fact::UnsafeEvents => c.hash_dispatch() || c.enable_unsafe_event.get(),
        Fact::HashDispatch => c.hash_dispatch(),
        Fact::Reactor => !swoole_rs_corebase::thread_reactor().is_null(),
        Fact::CacheSessionAddr => {
            crate::dispatch::http().is_some_and(|http| (http.can_cache_session_addr)(serv))
        }
    }
}

fn put(serv: view::Server<'_>, which: Install) {
    match which {
        Install::Allocators => crate::dispatch::use_zend_strings(serv),
        Install::AdminCommands => admin_register(serv, php_memory),
        _ => crate::dispatch::install(serv, which),
    }
}

/// `zend_memory_usage(true)`, the admin commands' PHP memory figure.
extern "C" fn php_memory() -> usize {
    swoole_rs_zend::memory_usage(true)
}

/// What a Server object embeds: its share in its server, none until `__construct`
/// binds one, what upstream's `ServerProperty` held, and the `init_arguments`
/// setting, null until `set()` gives one. Methods, and PHP code they run, share
/// it, so each field is a cell borrowed only for one access.
pub(crate) struct ServerData {
    serv: OnceCell<SharedServer>,
    property: RefCell<Property>,
    init_arguments: RefCell<Owned>,
    /// What `instance::set_server_object` marked the object with when it became
    /// its thread's running one; 0 for an object that never was.
    mark: Cell<u64>,
}

impl Custom for ServerData {
    fn class() -> &'static CustomClass<Self> {
        &SERVER
    }

    /// An unbound server.
    fn new() -> Self {
        Self {
            serv: OnceCell::new(),
            property: RefCell::default(),
            init_arguments: RefCell::new(Owned::null()),
            mark: Cell::new(0),
        }
    }
    const DESTRUCTS: bool = true;

    /// Once the object's last reference went, its releases run here, while it
    /// is still live: PHP code they run (a user process's destructor, say) can
    /// reach the object through a `Connection\Iterator`, as upstream's can. An
    /// object destructed at shutdown while something still holds it keeps what
    /// it has until it is freed.
    fn destruct(&self, last: bool) {
        if last {
            self.release();
        }
    }
}

/// A Server object and its data, for as long as the value holding it is borrowed.
#[derive(Clone, Copy)]
pub(crate) struct ServerObject<'a> {
    value: &'a Value,
    data: &'a ServerData,
}

impl<'a> ServerObject<'a> {
    /// A Server method's `$this`: the Server class's methods run on its objects.
    /// Anything else is an `E_ERROR`, which does not return; the frames own nothing.
    pub(crate) fn this(call: &'a Call) -> Self {
        call.this_value().and_then(Self::of).unwrap_or_else(|| {
            crate::server::invalid(crate::server::this_class(call));
            // The `E_ERROR` bailed out of this frame.
            swoole_rs_corebase::fatal("E_ERROR returned")
        })
    }

    /// The running server's object (`php_swoole_server_zval_ptr`) with a
    /// reference of its own, for [`ServerObject::of`]; none before one is bound
    /// and once it is being freed.
    pub(crate) fn running() -> Option<Owned> {
        crate::instance::server_object()
    }

    /// `f` over the running server's object, if there is one.
    pub(crate) fn with_running<R>(f: impl FnOnce(ServerObject<'_>) -> R) -> Option<R> {
        let held = Self::running()?;
        Some(f(ServerObject::of(&held)?))
    }

    /// The Server object `value` holds, if it is one.
    pub(crate) fn of(value: &'a Value) -> Option<Self> {
        let data = value.custom()?;
        Some(Self { value, data })
    }

    /// The value holding the object.
    pub(crate) fn value(self) -> &'a Value {
        self.value
    }

    /// The bound `Server`, or none before `__construct`.
    pub(crate) fn server(self) -> Option<view::Server<'a>> {
        self.data.serv.get().map(SharedServer::view)
    }

    /// [`ServerObject::server`] of a Server method's `$this`, which the call holds.
    pub(crate) fn this_server(call: &Call) -> Option<view::Server<'_>> {
        call.custom::<ServerData>()?
            .serv
            .get()
            .map(SharedServer::view)
    }

    /// [`ServerObject::server`] of the Server object `value` holds, if it is one.
    pub(crate) fn server_of(value: &'a Value) -> Option<view::Server<'a>> {
        Self::of(value)?.server()
    }

    /// A share in the bound `Server`, for an object that names it.
    pub(crate) fn share(self) -> Option<SharedServer> {
        self.data.serv.get().cloned()
    }

    pub(crate) fn set_mark(self, mark: u64) {
        self.data.mark.set(mark);
    }

    /// `__construct` binds the object to its server, once.
    fn set_serv(self, serv: SharedServer) {
        self.data.serv.get_or_init(|| serv);
    }

    /// `init_arguments`, with a reference of its own; null without one.
    pub(crate) fn init_arguments(self) -> Owned {
        self.data.init_arguments.borrow().copy()
    }

    /// Keep `value` as `init_arguments`; the one it replaces is released after
    /// the borrow ends, since releasing it can run PHP code.
    pub(crate) fn set_init_arguments(self, value: &Value) {
        drop(self.data.init_arguments.replace(value.copy()));
    }

    /// `f` over the property; `f` calls nothing that reaches the object.
    fn property<R>(self, f: impl FnOnce(&mut Property) -> R) -> R {
        f(&mut self.data.property.borrow_mut())
    }

    /// `get_callback(kind)`: the callable, a reference the call holds, or none.
    fn callback(self, kind: ServerCallback) -> Option<Callable> {
        self.property(|p| p.callbacks.get(kind as usize).and_then(Option::clone))
    }

    /// `on()`'s server-level callback: replace callback `kind` with `callable`;
    /// false if it is not callable, or once the old one's destructor bails out.
    pub(crate) fn set_callback(self, kind: ServerCallback, callable: &Value) -> bool {
        let kind = kind as usize;
        // Each callable drops out of the property: its destructor may call on() again.
        let Some(old) = self.property(|p| p.callbacks.get_mut(kind).map(Option::take)) else {
            return false;
        };
        drop(old);
        if bailout::pending() {
            return false;
        }
        let Some(callable) = crate::callable::create(callable) else {
            return false;
        };
        // A nested on() (from the old callable's destructor) stored its callable,
        // and the property, after this one's write: keep that pair, drop ours.
        drop(self.property(|p| match p.callbacks.get_mut(kind) {
            Some(slot) if slot.is_none() => slot.replace(callable),
            _ => Some(callable),
        }));
        true
    }

    /// The Port object at `index` (`property->ports.at(index)`), with a
    /// reference of its own; undefined for none.
    pub(crate) fn port(self, index: usize) -> Owned {
        self.property(|p| p.ports.get(index).map(|port| port.copy()))
            .unwrap_or_default()
    }

    /// The Port objects, each with a reference of its own.
    fn ports(self) -> Vec<Owned> {
        self.property(|p| p.ports.iter().map(|port| port.copy()).collect())
    }

    /// The user process at `index`, or null.
    pub(crate) fn user_process(self, index: usize) -> *mut zval {
        self.property(|p| p.user_processes.get(index).map(|process| process.as_ptr()))
            .unwrap_or(ptr::null_mut())
    }
}

/// `server_object->get_callback(kind)`: the callable, a reference the call holds, or none.
pub(crate) fn callback(kind: ServerCallback) -> Option<Callable> {
    ServerObject::with_running(|object| object.callback(kind)).flatten()
}

/// Whether this thread releases the callables stored on the native server and its
/// ports. A thread-mode worker thread's Server and Port objects sit over the one
/// server the master configured and frees, so only the master empties those slots:
/// a worker thread never writes them while others read them.
pub(crate) fn owns_shared() -> bool {
    /// `THREAD_WORKER`, which only thread mode's worker threads are entered as.
    const THREAD_WORKER: u8 = 4;
    swoole_rs_corebase::thread_kind() != THREAD_WORKER
}

impl ServerData {
    /// `php_swoole_server_free_object`'s releases: the `dispatch_func` callable,
    /// the callbacks, the user processes, the ports' links and the commands.
    /// Each goes out of the object before it drops, since its destructor can
    /// run PHP code that reaches the object (a `Connection\Iterator` calls the
    /// server's methods). Nothing the second time.
    fn release(&self) {
        let Some(server) = self.serv.get().map(SharedServer::view) else {
            return;
        };
        if owns_shared() {
            // Freed on the one thread that owns the config's slots.
            let dispatch = server.config().private_data_3.replace(None);
            bailout::defer(crate::dispatch::release_dispatch_func(dispatch));
        }
        let callbacks = std::mem::take(&mut self.property.borrow_mut().callbacks);
        drop(callbacks);
        // `sw_zval_free`: each copy's reference goes.
        let processes = std::mem::take(&mut self.property.borrow_mut().user_processes);
        drop(processes);
        let ports = std::mem::take(&mut self.property.borrow_mut().ports);
        for port in ports {
            crate::port::release(&port);
        }
        let commands = std::mem::take(&mut self.property.borrow_mut().commands);
        drop(commands);
    }
}

impl Drop for ServerData {
    /// The rest of `php_swoole_server_free_object`, before Zend releases the
    /// object's properties: what [`Custom::destruct`] left (it is skipped for an
    /// object PHP never destructs), then the object's share in its server. The
    /// object is no longer the running one, so the callbacks this runs find none.
    fn drop(&mut self) {
        crate::instance::forget(self.mark.get());
        self.release();
        let Some(share) = self.serv.take() else {
            return;
        };
        let server = share.view();
        if fact(server, Fact::Master) {
            crate::thread::free_setting(server);
            // Its shutdown callbacks run now, as they did before the delete. The
            // server is deleted with its last share: when Zend releases this
            // object's ports and iterator, unless PHP still holds a Port, an
            // iterator or a task that names it.
            server_methods::destroy_running(server);
        } else if owns_shared() {
            // Never started, or a base-mode start returned: upstream keeps it to the
            // exit, so MSHUTDOWN deletes it. Any other is kept as upstream keeps it.
            let orphan = !fact(server, Fact::ThreadMode) && server.config().factory_.is_null();
            kept().push((share, orphan));
        }
        // A worker thread's share in the master's server just goes.
    }
}

/// The shares a freed Server object leaves to [`php_swoole_server_mshutdown`], and those a failed
/// `__construct` never bound: upstream keeps their servers, as `SwooleG.server` too.
/// Each is true when MSHUTDOWN deletes its server.
fn kept() -> std::sync::MutexGuard<'static, Vec<(SharedServer, bool)>> {
    static KEPT: std::sync::Mutex<Vec<(SharedServer, bool)>> = std::sync::Mutex::new(Vec::new());
    KEPT.lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

/// MSHUTDOWN: the servers whose objects went in a process that isn't their master, and
/// the server and event worker pool a bailed-out single worker left, are deleted with
/// their last share now that no object names them. A started server some other process
/// runs stays to the exit, as upstream's.
pub extern "C" fn php_swoole_server_mshutdown() {
    for (share, orphan) in std::mem::take(&mut *kept()) {
        let delete = orphan || swoole_rs_server::reactor_process::release_abandoned(share.view());
        if !delete {
            #[expect(
                clippy::mem_forget,
                reason = "a started server another process runs stays to the exit"
            )]
            std::mem::forget(share);
        }
    }
}

/// Property writes on an object, under a class that sees them, while the value
/// holding the object is borrowed; nothing is written for a value that holds
/// none. Zend copies every name and value.
#[derive(Clone, Copy)]
pub(crate) struct Props<'a>(Option<swoole_rs_zend::Props<'a>>);

impl<'a> Props<'a> {
    /// A Server object, under the Server class.
    pub(crate) fn server(object: ServerObject<'a>) -> Self {
        Self(object.value.props(Some(&SERVER)))
    }

    /// The running server's object `held` holds, under the Server class.
    pub(crate) fn running(held: Option<&'a Value>) -> Self {
        Self(held.and_then(|held| held.props(Some(&SERVER))))
    }

    /// A Server object, under its own class.
    pub(crate) fn own(object: ServerObject<'a>) -> Self {
        Self(object.value.props(None).map(swoole_rs_zend::Props::own))
    }

    /// The object `value` holds, under its own class, for as long as `value` is borrowed.
    fn of_value(value: &'a Value) -> Option<Self> {
        Some(Self(Some(value.props(None)?.own())))
    }

    /// `object_init_ex(target, class)`: a new object in `target`, under its class;
    /// the handle borrows `target`, so it can't outlive the object. Zend
    /// instantiates every registered data class.
    pub(crate) fn init(target: &'a mut zval, class: &Class) -> Self {
        *target = class.create_object().into_zval();
        Self(Value::of(target).props(Some(class)))
    }

    /// The same handle as crates/zend's, whose writes defer a bailout.
    fn zend(self) -> Option<swoole_rs_zend::Props<'a>> {
        self.0
    }

    pub(crate) fn long(self, name: &CStr, value: i64) {
        if let Some(props) = self.0 {
            props.long(name, value);
        }
    }

    pub(crate) fn double(self, name: &CStr, value: f64) {
        if let Some(props) = self.0 {
            props.value(name, &Owned::double(value));
        }
    }

    pub(crate) fn string(self, name: &CStr, value: &CStr) {
        if let Some(props) = self.0 {
            props.bytes(name, value.to_bytes());
        }
    }

    pub(crate) fn bytes(self, name: &CStr, value: &[u8]) {
        if let Some(props) = self.0 {
            props.bytes(name, value);
        }
    }

    pub(crate) fn bool(self, name: &CStr, value: bool) {
        if let Some(props) = self.0 {
            props.bool(name, value);
        }
    }

    /// The property takes a copy of a live value.
    pub(crate) fn set(self, name: &CStr, value: &Value) {
        if let Some(props) = self.0 {
            props.value(name, value);
        }
    }

    /// `set()`'s `php_array_merge` of `settings` into the array property `name`.
    pub(crate) fn merge(self, name: &CStr, settings: &Value) {
        if let Some(props) = self.0 {
            props.merge(name, settings);
        }
    }

    pub(crate) fn value(self, name: &CStr, value: &mut zval) {
        self.set(name, Value::of(value));
    }

    /// `host`, `port`, `type`, `sock` (with `fd`) and `ssl` from `info`.
    fn port_info(self, info: &PortInfo, fd: Option<i64>) {
        self.bytes(c"host", &info.host);
        self.long(c"port", info.port);
        self.long(c"type", info.kind);
        if let Some(fd) = fd {
            self.long(c"sock", fd);
        }
        self.bool(c"ssl", info.ssl);
    }

    /// A `Connection\Iterator` over `serv` (and `port`, or all of them) in the
    /// object's `connections` property.
    fn connections(self, serv: SharedServer, port: *mut c_void) {
        self.set(c"connections", &crate::server::new_iterator(serv, port));
    }
}

/// `php_swoole_server_add_port`: a Port object for `port`, kept by the Server
/// object and appended to `$server->ports`; undefined unless `port` is one of
/// the object's server's ports.
pub(crate) fn add_port(object: ServerObject<'_>, port: view::Port<'_>) -> Owned {
    let Some(serv) = object.share() else {
        return Owned::default();
    };
    if !serv
        .view()
        .ports()
        .iter()
        .any(|own| own.raw() == port.raw())
    {
        return Owned::default();
    }
    let zport = crate::port::new_object(serv.clone(), port);
    let kept = zport.copy();
    object.property(|p| p.ports.push(kept));
    let info = PortInfo::of(port);
    if let Some(props) = Props::of_value(&zport) {
        props.port_info(&info, Some(info.fd));
    }
    let copy = zport.copy();
    // The Server object's `ports`, which its class declares.
    if let Some(props) = Props::own(object).zend() {
        props.edit_array(c"ports", |ports| ports.push(zport));
    }
    if let Some(props) = Props::of_value(&copy) {
        props.connections(serv, port.raw());
    }
    copy
}

/// `server_ctor`: bind `object` to `serv` and describe its primary port.
fn bind(call: &Call, share: &SharedServer) {
    let object = ServerObject::this(call);
    crate::instance::set_server_object(object);
    object.set_serv(share.clone());
    let serv = share.view();
    for &port in serv.ports() {
        add_port(object, port);
    }
    let props = Props::server(object);
    props.connections(share.clone(), ptr::null_mut());
    props.long(c"mode", serv.config().mode_ as i64);
    if let Some(primary) = serv.port_at(0) {
        props.port_info(&PortInfo::of(primary), None);
    }
}

/// `zend_throw_exception_ex(swoole_exception_ce, code, message)`.
fn exception(call: &Call, message: &CStr, code: i64) {
    call.throw(swoole_rs_phpswoole::exception(), message, code);
}

use crate::server::set_last_error;
use crate::thread::Admission;

pub(crate) fn class(object: ServerObject<'_>) -> String {
    let name = object.value().class_name().unwrap_or_default();
    name.to_string_lossy().into_owned()
}

fn construct(call: &Call) {
    let object = ServerObject::this(call);
    if object.server().is_some() {
        call.throw_constructed_twice();
        return call.set_bool(false);
    }
    if !swoole_rs_phpswoole::globals().cli.get() {
        let text = message!("{} can only be used in CLI mode", class(object));
        exception(call, text.as_cstr(), -1);
        return call.set_bool(false);
    }
    let parsed = call.arity(1, 4).and_then(|()| {
        Ok((
            call.string(1, b"")?,
            call.long(2, 0)?,
            call.long(3, Mode::Base as i64)?,
            call.long(4, SOCK_TCP)?,
        ))
    });
    let Ok((address, port, mode, kind)) = parsed else {
        return call.set_bool(false);
    };
    let thread = crate::server::THREAD.load(Ordering::Relaxed);
    let Some(server_mode) = Mode::from_code(mode).filter(|&m| thread || m != Mode::Thread) else {
        set_last_error(SW_ERROR_INVALID_PARAMS);
        let text = message!("invalid $mode parameters {}", mode.c_cast::<i32>());
        call.throw_error(text.as_cstr());
        return call.set_bool(false);
    };
    match crate::thread::admit() {
        Admission::New => {}
        Admission::Worker(server) => return bind(call, &server),
        Admission::NotMainThread => {
            set_last_error(SW_ERROR_OPERATION_NOT_SUPPORT);
            exception(
                call,
                c"This operation is only allowed in the main thread",
                -2,
            );
            return call.set_bool(false);
        }
        Admission::Running => {
            set_last_error(SW_ERROR_OPERATION_NOT_SUPPORT);
            let text = message!("server is running. unable to create {}", class(object));
            exception(call, text.as_cstr(), -3);
            return call.set_bool(false);
        }
    }
    // A supported mode.
    let share = SharedServer::new(c_int::try_from(mode).unwrap_or_default());
    let server = share.view();
    if server_mode == Mode::Base {
        server.config().reactor_num.set(1);
        server.config().worker_num.set(1);
    }
    if port == 0 && address.eq_ignore_ascii_case(b"SYSTEMD") {
        if server_methods::add_systemd_socket(server) <= 0 {
            call.throw_error(c"failed to add systemd socket");
            kept().push((share, true));
            return call.set_bool(false);
        }
    } else if {
        // The host as C reads it: up to its first NUL.
        let host = address.split(|&byte| byte == 0).next().unwrap_or_default();
        let host = CString::new(host).unwrap_or_default();
        server_methods::add_port(
            server,
            kind.c_cast::<c_int>(),
            &host,
            port.c_cast::<c_int>(),
        )
    }
    .is_none()
    {
        let code = swoole_rs_corebase::log::last_error();
        let reason = swoole_rs_phpruntime::strerror(code);
        let mut text = Message::new();
        text.push(b"failed to listen server port[");
        text.push(address);
        text.push(format!(":{port}], Error: ").as_bytes());
        text.push(reason.to_bytes());
        text.push(format!("[{code}]").as_bytes());
        exception(call, text.as_cstr(), code.into());
        kept().push((share, true));
        return call.set_bool(false);
    }
    bind(call, &share);
}

fn add_process(call: &Call) {
    let serv = server(call);
    if !fact(serv, Fact::WorkerThread) && fact(serv, Fact::Started) {
        fatal(E_WARNING, c"server is running, cannot add process");
        return call.set_bool(false);
    }
    if call.arity(1, 1).is_err() {
        return call.set_bool(false);
    }
    let Some(process) = call
        .value(1)
        .filter(|process| process.instance_of(swoole_rs_phpprocess::process_class()))
    else {
        fatal(E_ERROR, c"object is not instanceof swoole_process");
        return call.set_bool(false);
    };
    put(serv, Install::UserWorkerStart);
    // A copy holding its own reference, as upstream.
    let copy = Box::new(process.copy());
    let zcopy = copy.as_ptr();
    // The box keeps the copy where the worker's user data points.
    ServerObject::this(call).property(|p| p.user_processes.push(copy));
    match add_worker(serv, process, zcopy) {
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
            // The process object, which the copy holds too.
            if let Some(props) = process.props(Some(swoole_rs_phpprocess::process_class())) {
                props.long(c"id", id);
            }
            call.set_long(id);
        }
    }
}

fn add_command(call: &Call) {
    let serv = server(call);
    if fact(serv, Fact::Started) {
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
    let Some(callable) = call.value(3).and_then(crate::callable::create) else {
        return call.set_bool(false);
    };
    // The property keeps the callable for the server's life; the command table
    // may outlive it.
    let handle = Rc::downgrade(&callable);
    #[expect(
        clippy::arc_with_non_send_sync,
        reason = "the server's command table, like upstream's, holds PHP callables"
    )]
    let handler = std::sync::Arc::new(
        move |_: view::Server<'_>, message: &[u8], reply: &mut Vec<u8>| {
            command_run(&handle, message, reply);
        },
    );
    // Upstream's `(int) types`.
    let types = c_int::from_le_bytes(types.to_le_bytes()[..4].try_into().unwrap_or_default());
    if !swr_v1_server_add_command(serv, name, types, handler) {
        drop(callable);
        return call.set_bool(false);
    }
    ServerObject::this(call).property(|p| p.commands.push(callable));
    call.set_bool(true);
}

/// A command handler (`Server::Command::Handler`): `callable($server, $message)`'s
/// string, or upstream's JSON errors, into `reply`. A bailout is raised last, from
/// this frame. A callable its server already released fails the call.
fn command_run(callable: &Weak<Kept>, message: &[u8], reply: &mut Vec<u8>) {
    let mut send = |text: &[u8]| reply.extend_from_slice(text);
    let Some(callable) = callable.upgrade() else {
        send(br#"{"data": "failed to call function", "code": -1}"#);
        return;
    };
    let message = Owned::string(ZString::new(message)).into_zval();
    let mut args = [crate::dispatch::zserv(), message];
    let retval = Owned::default();
    let called = crate::dispatch::invoke(&callable, &mut args, retval.as_ptr(), false);
    // The message string made above.
    crate::dispatch::dtor(&mut args[1]);
    let bailed = match called {
        Err(_) => true,
        Ok(false) => {
            set_last_error(crate::server::SW_ERROR_PHP_FATAL_ERROR);
            let warned = report(E_WARNING, c"Swoole\\Server: command handler error");
            if warned.is_ok() {
                send(br#"{"data": "failed to call function", "code": -1}"#);
            }
            warned.is_err()
        }
        Ok(true) => {
            match retval.bytes() {
                Some(text) => send(text),
                None => send(br#"{"data": "wrong return type", "code": -2}"#),
            }
            false
        }
    };
    // Released first: a bailout jumps over this frame.
    drop(retval);
    drop(callable);
    if bailed {
        bailout::raise(Err(bailout::Bailout));
    }
}

/// `command()`'s wait, which it shares with the reply's callback: the reply's
/// bytes, which become the call's return value once it resumes. Only Rust
/// memory: the server's callback table may keep the callback, and so this,
/// past the request (a master-handled command's stays until shutdown).
struct Wait {
    reply: RefCell<Option<Vec<u8>>>,
    coroutine: Coroutine,
    replied: Cell<bool>,
}

/// The command's reply (`Server::Command::Callback`): into `reply`, then the
/// waiting coroutine resumes, if it has yielded yet.
fn command_done(wait: Rc<Wait>) -> impl Fn(&[u8]) {
    move |message| {
        wait.reply.replace(Some(message.to_vec()));
        if wait.coroutine.is_suspending() == Some(true) {
            // SAFETY: it parked in `command`; this frame owns nothing that needs a drop.
            unsafe { wait.coroutine.wake(false) };
        } else {
            wait.replied.set(true);
        }
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
    let encoded = match call.value(4).map(Value::json_encode) {
        Some(Ok(Some(encoded))) => encoded,
        Some(Err(bailout)) => return bailout.defer(),
        _ => return call.set_bool(false),
    };
    if let Err(bailout) = swoole_rs_phpruntime::coroutine_check() {
        return bailout.defer();
    }
    let Some(coroutine) = Coroutine::current() else {
        return;
    };
    let wait = Rc::new(Wait {
        reply: RefCell::default(),
        coroutine,
        replied: Cell::new(false),
    });
    let data = encoded.bytes();
    #[expect(
        clippy::arc_with_non_send_sync,
        reason = "the server's callback table, like upstream's, holds this coroutine's wait"
    )]
    let callback = std::sync::Arc::new(command_done(Rc::clone(&wait)));
    // Upstream's `(uint16_t) process_id` and `(ProcessType) process_type`.
    let [low, high, ..] = process_id.to_le_bytes();
    let process_id = u32::from(u16::from_le_bytes([low, high]));
    let process_type = c_int::from_le_bytes(
        process_type.to_le_bytes()[..4]
            .try_into()
            .unwrap_or_default(),
    );
    let sent = swr_v1_server_command(serv, process_id, process_type, name, data, callback);
    drop(encoded);
    if !sent {
        return call.set_bool(false);
    }
    if !wait.replied.get() {
        // `command_done` resumes it.
        swoole_rs_phpcoroutines::suspend();
    }
    let Some(reply) = wait.reply.take() else {
        return;
    };
    if json_decode {
        call.set_owned(Owned::json_decode(&reply));
    } else {
        call.set_owned(Owned::string(ZString::new(&reply)));
    }
}

/// `php_swoole_error(E_WARNING, ...)`: a notice-coded warning, shown when
/// `display_errors` is on.
pub(crate) fn runtime_warning(text: &CStr) {
    set_last_error(SW_ERROR_PHP_RUNTIME_NOTICE);
    if crate::server::display_errors() {
        bailout::defer(report(E_WARNING, text));
    }
}

/// Stop: an `E_ERROR` or a trapped bailout is pending.
struct Stop;

fn stop<T>(level: i32, text: &CStr) -> Result<T, Stop> {
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
fn register_callback(object: ServerObject<'_>, serv: view::Server<'_>) {
    put(serv, Install::Control);
    if object.callback(ServerCallback::Task).is_some() {
        put(serv, Install::Task);
    }
    if object.callback(ServerCallback::PipeMessage).is_some() {
        put(serv, Install::PipeMessage);
    }
    if fact(serv, Fact::SendYield) && fact(serv, Fact::UnsafeEvents) {
        put(serv, Install::BufferEmpty);
    }
}

fn port_facts(object: &Value) -> PortFacts<'_> {
    let port = crate::port::bound_port(object);
    let c = port.config();
    PortFacts {
        port,
        protocols: [
            (c.open_http_protocol.get(), HTTP),
            (c.open_websocket_protocol.get(), WEBSOCKET),
            (c.open_http2_protocol.get(), HTTP2),
            (c.open_redis_protocol.get(), REDIS),
        ]
        .iter()
        .filter(|(on, _)| *on)
        .fold(0, |flags, (_, flag)| flags | flag),
        dgram: crate::port_settings::is_dgram(c.r#type),
        // `Socket::is_stream(type)`: TCP, TCP6 or a Unix stream socket.
        stream: matches!(c.r#type, 1 | 3 | 5),
        ssl: c.ssl != 0,
        ssl_verify_without_ca: c.ssl_verify_without_ca(),
    }
}

fn isset(serv: view::Server<'_>, port: view::Port<'_>, kind: PortCallback) -> bool {
    crate::instance::php_swoole_server_isset_callback(serv, Some(port), kind)
}

/// Optionally `clear_protocol()`, then turn on each protocol in `flags`.
fn protocols(port: view::Port<'_>, flags: u32, clear: bool) {
    let c = port.config();
    if clear {
        swoole_rs_ports::methods::clear_protocol(c);
    }
    c.open_http_protocol
        .set(c.open_http_protocol.get() || (flags & HTTP != 0));
    c.open_websocket_protocol
        .set(c.open_websocket_protocol.get() || (flags & WEBSOCKET != 0));
    c.open_http2_protocol
        .set(c.open_http2_protocol.get() || (flags & HTTP2 != 0));
    c.open_redis_protocol
        .set(c.open_redis_protocol.get() || (flags & REDIS != 0));
}

fn has_key(setting: &ArrayMut, key: &CStr) -> bool {
    setting
        .array()
        .is_some_and(|setting| setting.find(key.to_bytes()).is_some())
}

/// `ServerObject::on_before_start`.
#[expect(clippy::too_many_lines, reason = "upstream's checks, in its order")]
fn before_start(object: ServerObject<'_>, serv: view::Server<'_>) -> Result<(), Stop> {
    if server_methods::create(serv) < 0 {
        // The calling thread's `sw_error`.
        let reason = last_error_message();
        let mut text = Message::new();
        text.push(b"failed to create the server. Error: ");
        text.push(reason.to_bytes());
        return stop(E_ERROR, text.as_cstr());
    }
    if fact(serv, Fact::EnableCoroutine) {
        crate::settings::set_reload_async(serv);
    }
    if fact(serv, Fact::SendYield)
        && !fact(serv, Fact::HasOnClose)
        && fact(serv, Fact::UnsafeEvents)
    {
        put(serv, Install::Close);
    }
    put(serv, Install::Allocators);
    Props::own(object).long(c"master_pid", i64::from(std::process::id()));
    let kind = kind(object);
    let http_server = kind == Kind::Http || kind == Kind::WebSocket;
    let ports = object.ports();
    let mut find_http_port = false;
    // `$this`'s `setting`, which its class declares, until its last write. Only
    // fatal errors are raised meanwhile, and they run no PHP code.
    let edited = Props::own(object).zend().and_then(|props| {
        props.edit_array(c"setting", |settings| {
            let c = serv.config();
            for (key, value) in [
                (c"worker_num", c.worker_num.get()),
                (c"task_worker_num", c.task_worker_num.get()),
                (c"output_buffer_size", c.output_buffer_size.get()),
                (c"max_connection", serv.max_connection()),
            ] {
                if !has_key(settings, key) {
                    settings.insert_long(key.to_bytes(), i64::from(value));
                }
            }
            if has_key(settings, c"admin_server") {
                put(serv, Install::AdminCommands);
            }
            let Some(first) = ports.first() else {
                return Ok(false);
            };
            let primary = port_facts(first);
            if kind == Kind::Redis {
                settings.insert_bool(b"open_redis_protocol", true);
                settings.insert_bool(b"open_http_protocol", false);
                settings.insert_bool(b"open_mqtt_protocol", false);
                settings.insert_bool(b"open_eof_check", false);
                settings.insert_bool(b"open_length_check", false);
                protocols(primary.port, REDIS, true);
                put(serv, Install::RedisReceive);
            } else if http_server {
                if kind == Kind::WebSocket {
                    if !isset(serv, primary.port, PortCallback::Message) {
                        return stop(E_ERROR, c"require onMessage callback");
                    }
                } else if !isset(serv, primary.port, PortCallback::Request) {
                    return stop(E_ERROR, c"require onRequest callback");
                }
                settings.insert_bool(b"open_http_protocol", true);
                settings.insert_bool(b"open_mqtt_protocol", false);
                settings.insert_bool(b"open_eof_check", false);
                settings.insert_bool(b"open_length_check", false);
                let mut flags = HTTP;
                if primary.protocols & HTTP2 != 0 {
                    settings.insert_bool(b"open_http2_protocol", true);
                    flags |= HTTP2;
                }
                if primary.protocols & WEBSOCKET != 0 || kind == Kind::WebSocket {
                    settings.insert_bool(b"open_websocket_protocol", true);
                    flags |= WEBSOCKET;
                }
                protocols(primary.port, flags, true);
                find_http_port = true;
                put(serv, Install::HttpReceive);
            } else {
                if primary.dgram && !primary.ssl && !isset(serv, primary.port, PortCallback::Packet)
                {
                    return stop(E_ERROR, c"require onPacket callback");
                }
                if ((primary.dgram && primary.ssl) || primary.stream)
                    && !isset(serv, primary.port, PortCallback::Receive)
                {
                    return stop(E_ERROR, c"require onReceive callback");
                }
                put(serv, Install::Receive);
            }
            Ok(true)
        })
    });
    let Some(edited) = edited else {
        // A pending bailout reports itself; otherwise PHP code left a non-array there.
        if bailout::pending() {
            return Err(Stop);
        }
        return stop(E_ERROR, c"Swoole\\Server::$setting must be an array");
    };
    if !edited? {
        return Ok(());
    }
    // A reference of its own: `$port->set()` may replace the property.
    let Some(setting) = object.value().property(c"setting").map(Value::copy) else {
        return Ok(());
    };
    let setting: &Value = &setting;
    for zport in ports.iter().skip(1) {
        // The Port class declares `setting`.
        if zport.property(c"setting").is_some_and(Value::is_null) {
            // `$port->set($setting)`, Port's own method.
            let set = zport.call_method(Some(&crate::PORT), b"set", &[setting]);
            trapped(set.is_err())?;
        }
    }
    for zport in &ports {
        let port = port_facts(zport);
        if port.dgram && !port.ssl && !isset(serv, port.port, PortCallback::Packet) {
            return stop(E_ERROR, c"require onPacket callback");
        }
        if port.ssl_verify_without_ca {
            return stop(
                E_ERROR,
                c"server open verify peer require `ssl_client_cert_file` or `ssl_capath` or `ssl_cafile` config",
            );
        }
        if port.protocols & HTTP2 != 0 && !fact(serv, Fact::HashDispatch) {
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
            let receive = isset(serv, port.port, PortCallback::Receive);
            if port.protocols & WEBSOCKET != 0 {
                if !isset(serv, port.port, PortCallback::Message) && !receive {
                    return stop(E_ERROR, c"require onMessage callback");
                }
            } else if !isset(serv, port.port, PortCallback::Request) && !receive {
                return stop(E_ERROR, c"require onRequest callback");
            }
            if !http_server && isset(serv, port.port, PortCallback::Request) {
                let text = message!(
                    "use {} class and open http related protocols may lead to some errors (inconsistent class type)",
                    class(object)
                );
                runtime_warning(text.as_cstr());
                if bailout::pending() {
                    return Err(Stop);
                }
            }
        } else if port.protocols & REDIS == 0
            && port.stream
            && !isset(serv, port.port, PortCallback::Receive)
        {
            return stop(E_ERROR, c"require onReceive callback");
        }
    }
    if find_http_port {
        put(serv, Install::HttpReceive);
        if fact(serv, Fact::CacheSessionAddr) {
            put(serv, Install::HttpClose);
        }
    }
    if crate::server::THREAD.load(Ordering::Relaxed)
        && fact(serv, Fact::ThreadMode)
        && let Err(bailout) = crate::thread::copy_setting(setting)
    {
        bailout.defer();
        return Err(Stop);
    }
    if !swoole_rs_phpswoole::enable_library() {
        return Ok(());
    }
    let mut zserv = crate::dispatch::zserv();
    let called = swoole_rs_zend::host::named_call(
        br"\Swoole\Server\Helper::onBeforeStart",
        &[Value::of(&mut zserv)],
    );
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
    let refusal = if fact(serv, Fact::Started) {
        Some("The server is running, unable to execute {}->start()")
    } else if fact(serv, Fact::Shutdown) {
        Some("The server have been shutdown, unable to execute {}->start()")
    } else if fact(serv, Fact::Reactor) {
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
    match start_server(serv) {
        Err(bailout) => return bailout.defer(),
        Ok(..0) => {
            let reason = server_methods::get_startup_error_message();
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
    let Some(co) = Coroutine::current() else {
        return Ok(false);
    };
    if data.is_empty() {
        return Ok(false);
    }
    loop {
        let queued = ServerObject::with_running(|object| {
            object.property(|p| {
                p.send_token += 1;
                (
                    Rc::clone(p.send_waiters.entry(session_id).or_default()),
                    p.send_token,
                )
            })
        });
        let Some((waiters, token)) = queued else {
            return Ok(false);
        };
        waiters.borrow_mut().push_back((token, co));
        let timeout = server.config().send_timeout.get();
        // `onBufferEmpty` or `onClose` wakes it.
        if !swoole_rs_phpcoroutines::suspend_for(timeout) {
            waiters.borrow_mut().retain(|&(waiter, _)| waiter != token);
            return Ok(false);
        }
        let sent = server_methods::send(server, session_id, data);
        let yielded = !sent && swoole_rs_corebase::log::last_error() == SW_ERROR_OUTPUT_SEND_YIELD;
        if yielded && fact(server, Fact::SendYield) {
            continue;
        }
        return Ok(sent);
    }
}

/// Take the session's waiting coroutines out of the running server to wake
/// (`onBufferEmpty` resumes each, `onClose` cancels each), none when none
/// wait; never dropped implicitly, so a frame that holds them owns nothing a
/// bailout would skip.
pub(crate) fn send_waiters(session_id: i64) -> Option<std::mem::ManuallyDrop<Waiters>> {
    ServerObject::with_running(|object| {
        // Without waiters, skip hashing the key: SipHash is most of an onClose (#2964).
        object.property(|p| {
            let waiters = &mut p.send_waiters;
            (!waiters.is_empty()).then(|| waiters.remove(&session_id))?
        })
    })
    .flatten()
    .map(std::mem::ManuallyDrop::new)
}

/// The next coroutine to wake: `pop` takes it off the queue (a resume), else
/// it stays first until its `send_yield` leaves (a cancel).
pub(crate) fn send_next(waiters: &Waiters, pop: bool) -> Option<Coroutine> {
    let mut queue = waiters.borrow_mut();
    let next = if pop {
        queue.pop_front()
    } else {
        queue.front().copied()
    };
    next.map(|(_, co)| co)
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
        // Not dropped: its drop would link `Owned`'s Zend destructor, which the test
        // binary has no PHP to resolve; an empty property owns nothing to free.
        let property = std::mem::ManuallyDrop::new(Property::default());
        assert!(property.callbacks.iter().all(Option::is_none));
        assert!(property.ports.is_empty() && property.user_processes.is_empty());
        assert_eq!(
            property.callbacks.len(),
            ServerCallback::PipeMessage as usize + 1
        );
    }
}
