//! `Swoole\Process\Pool`: registration, objects, methods and the pool's event
//! callbacks, over crates/process's `ProcessPool` (`PoolRecord`).
use crate::{
    host::{self, Callable},
    process::{set_worker_of, worker_pid, worker_view},
    report::{INVALID_PARAMS, OPERATION_NOT_SUPPORT, cli, fatal, notice, set_last_error},
    signal::clean,
};
use std::{
    cell::{Cell, RefCell},
    ffi::{CString, c_int, c_void},
    ptr,
    rc::Rc,
    sync::atomic::{AtomicPtr, Ordering},
};
use swoole_rs_corebase::CCast;
use swoole_rs_phpstubs::process_pool;
use swoole_rs_process::process_pool::{Hooks, MessageFn, PoolRef, RecvData, WorkerFn};
use swoole_rs_process::worker_record::{Worker, WorkerRecord};
use swoole_rs_process::{
    methods,
    process_pool::{PROTOCOL_MESSAGE, PROTOCOL_STREAM, PoolRecord, max_package_size, packet_limit},
    signal::swoole_signal_isset,
    worker_record,
};
use swoole_rs_reactor::signal_upstream::Handler as SignalHandler;
use swoole_rs_reactor::socket_upstream::socket_set_fd_option;
use swoole_rs_zend::{
    Call, Class, Custom, CustomClass, E_ERROR, E_WARNING, Minit, Owned, Value, ZString, handlers,
    message,
    string::{self, ZBuf},
    sys::{ZEND_ACC_PUBLIC, zval},
};
use swoole_rs_zend::{method_table, sys as z};

const IPC_NONE: i64 = 0;
const IPC_UNIXSOCK: i64 = 1;
const IPC_MSGQUEUE: i64 = 2;
const IPC_SOCKET: i64 = 3;
/// `SW_SOCK_TCP` and `SW_SOCK_UNIX_STREAM`.
const SOCK_TCP: c_int = 1;
const SOCK_UNIX_STREAM: c_int = 5;
/// `SW_MASTER` and `SW_WORKER`.
const MASTER: c_int = 1;
const WORKER: c_int = 2;
/// `SW_WORKER_BUSY` and `SW_WORKER_IDLE`.
const WORKER_BUSY: u8 = 1;
const WORKER_IDLE: u8 = 2;
/// `SW_EVENT_DATA_OBJ_PTR`.
const EVENT_DATA_OBJ_PTR: u8 = 1 << 5;

type Pool = *mut PoolRecord;

/// `swoole::MessageBus`'s first members: its Rust bus, then its allocator.
#[repr(C)]
struct BusHead {
    bus: *mut c_void,
    allocator: *const c_void,
}

static POOL: CustomClass<PoolObject> = CustomClass::new();
/// `current_pool` and `current_worker`.
static CURRENT: AtomicPtr<PoolRecord> = AtomicPtr::new(ptr::null_mut());
static CURRENT_WORKER: AtomicPtr<c_void> = AtomicPtr::new(ptr::null_mut());

/// Swoole\\Exception.
fn exception() -> &'static Class {
    swoole_rs_phpswoole::exception()
}

/// `ProcessPool::create` in a zeroed pool that keeps a copy of `$this`'s zval;
/// on failure, none, with `errno` set.
fn pool_new(worker_num: u32, key: c_int, ipc: c_int, object: &Value, async_: bool) -> Option<Pool> {
    let pool = Box::into_raw(Box::new(PoolRecord::empty()));
    if methods::create(handle(pool), worker_num, key, ipc) < 0 {
        // SAFETY: the allocation above, which nothing else holds.
        drop(unsafe { Box::from_raw(pool) });
        return None;
    }
    // Copied without a reference, as upstream's `sw_zval_dup`.
    let hooks = Hooks {
        ptr: Some(Box::into_raw(Box::new(object.copy_value())).cast()),
        ..Hooks::default()
    };
    // SAFETY: the pool has no hooks yet, and those `start` installs read this copy
    // (`owner`), which `pool_free` drops only as it destroys the pool.
    unsafe { handle(pool).install(&hooks) };
    handle(pool).set_is_async(async_);
    Some(pool)
}

fn pool_free(pool: Pool) {
    // SAFETY: the zval copy `pool_new` made.
    drop(unsafe { Box::from_raw(handle(pool).ptr().cast::<zval>()) });
    methods::destroy(handle(pool));
    // SAFETY: the allocation `pool_new` made.
    drop(unsafe { Box::from_raw(pool) });
}

/// `SwooleWG.worker`'s status, and its request count once idle again.
fn worker_busy(busy: bool) {
    swoole_rs_process::worker_globals::with_current(|worker| {
        if busy {
            worker.set_status(WORKER_BUSY);
        } else {
            worker.add_request_count();
            worker.set_status(WORKER_IDLE);
        }
    });
}

/// The message's payload as a string: the bus's packet itself when it holds one.
fn message_data(pool: Pool, message: &RecvData) -> Owned {
    let (length, data) = (
        message.info.len as usize,
        message.data.cast_mut().cast::<u8>(),
    );
    let string = if length == 0 {
        ZString::new(b"")
    } else if message.info.flags & EVENT_DATA_OBJ_PTR != 0 {
        // SAFETY: the bus allocates its packets as unshared zend strings of at
        // least `length` written bytes.
        let packet = unsafe { ZBuf::from_raw(string::from_val(data), length) };
        // SAFETY: an OBJ_PTR message comes from the pool's live message bus; the
        // string is now the caller's.
        unsafe { swoole_rs_server::bus_move_packet(handle(pool).message_bus()) };
        packet.into_zstring()
    } else {
        // SAFETY: `length` bytes at `data`.
        ZString::new(unsafe { std::slice::from_raw_parts(data, length) })
    };
    Owned::string(string)
}

/// One event's callable. Dispatch clones it out before calling PHP, so the
/// callback can replace it (`on()`) without freeing the callable that runs.
#[derive(Default)]
struct Handler(RefCell<Option<Rc<Callable>>>);

impl Handler {
    fn get(&self) -> Option<Rc<Callable>> {
        self.0.borrow().clone()
    }

    fn is_set(&self) -> bool {
        self.0.borrow().is_some()
    }

    /// Store `callable`, handing back the one it replaces for the caller to drop.
    fn replace(&self, callable: Option<Callable>) -> Option<Rc<Callable>> {
        self.0.replace(callable.map(Rc::new))
    }
}

/// `struct ProcessPoolObject`: the pool, its six callables and two flags. PHP
/// code a callback runs can call the object's methods, so every field is a cell
/// and the object is only ever borrowed shared.
#[derive(Default)]
struct PoolObject {
    pool: Cell<Pool>,
    on_start: Handler,
    on_shutdown: Handler,
    on_worker_start: Handler,
    on_worker_stop: Handler,
    on_worker_exit: Handler,
    on_message: Handler,
    enable_coroutine: Cell<bool>,
    enable_message_bus: Cell<bool>,
}

impl Custom for PoolObject {
    fn class() -> &'static CustomClass<Self> {
        &POOL
    }
    fn new() -> Self {
        Self::default()
    }
}

impl Drop for PoolObject {
    fn drop(&mut self) {
        let pool = self.pool.get();
        if !pool.is_null() {
            pool_free(pool);
        }
        for handler in [
            &self.on_worker_start,
            &self.on_message,
            &self.on_worker_stop,
            &self.on_start,
            &self.on_worker_exit,
            &self.on_shutdown,
        ] {
            drop(handler.replace(None));
        }
    }
}

/// `$this`, which every instance method runs on.
fn this(call: &Call) -> Option<&PoolObject> {
    call.custom()
}

/// `process_pool_get_and_check_pool`.
fn checked(object: Option<&PoolObject>) -> Pool {
    let pool = object.map_or(ptr::null_mut(), |object| object.pool.get());
    if pool.is_null() {
        host::unconstructed();
    }
    pool
}

/// A pool this module created, to run pool operations on.
fn handle<'a>(pool: Pool) -> PoolRef<'a> {
    // SAFETY: callers pass a live pool.
    unsafe { PoolRef::new(pool) }
}

method_table! {
    static METHODS = [
        c"__construct" => zim_swoole_process_pool___construct, &process_pool::arginfo_class_Swoole_Process_Pool___construct, z::ZEND_ACC_PUBLIC;
        c"__destruct" => zim_swoole_process_pool___destruct, &process_pool::arginfo_class_Swoole_Process_Pool___destruct, z::ZEND_ACC_PUBLIC;
        c"set" => zim_swoole_process_pool_set, &process_pool::arginfo_class_Swoole_Process_Pool_set, z::ZEND_ACC_PUBLIC;
        c"on" => zim_swoole_process_pool_on, &process_pool::arginfo_class_Swoole_Process_Pool_on, z::ZEND_ACC_PUBLIC;
        c"getProcess" => zim_swoole_process_pool_getProcess, &process_pool::arginfo_class_Swoole_Process_Pool_getProcess, z::ZEND_ACC_PUBLIC;
        c"listen" => zim_swoole_process_pool_listen, &process_pool::arginfo_class_Swoole_Process_Pool_listen, z::ZEND_ACC_PUBLIC;
        c"write" => zim_swoole_process_pool_write, &process_pool::arginfo_class_Swoole_Process_Pool_write, z::ZEND_ACC_PUBLIC;
        c"sendMessage" => zim_swoole_process_pool_sendMessage, &process_pool::arginfo_class_Swoole_Process_Pool_sendMessage, z::ZEND_ACC_PUBLIC;
        c"detach" => zim_swoole_process_pool_detach, &process_pool::arginfo_class_Swoole_Process_Pool_detach, z::ZEND_ACC_PUBLIC;
        c"start" => zim_swoole_process_pool_start, &process_pool::arginfo_class_Swoole_Process_Pool_start, z::ZEND_ACC_PUBLIC;
        c"stop" => zim_swoole_process_pool_stop, &process_pool::arginfo_class_Swoole_Process_Pool_stop, z::ZEND_ACC_PUBLIC;
        c"shutdown" => zim_swoole_process_pool_shutdown, &process_pool::arginfo_class_Swoole_Process_Pool_shutdown, z::ZEND_ACC_PUBLIC;
    ];
}

/// The pool's Rust worker loops call PHP inside this trap and leave before re-raising.
unsafe extern "C" fn trap(body: unsafe extern "C" fn(*mut c_void), arg: *mut c_void) -> c_int {
    // SAFETY: a C body for `zend_try`.
    unsafe { swoole_rs_zend::sys::swr_zend_try(Some(body), arg) }
}

/// `zend_bailout()`, from a frame that owns nothing.
unsafe extern "C" fn raise() {
    // SAFETY: Zend's bailout.
    unsafe {
        swoole_rs_zend::sys::_zend_bailout(c"crates/phpprocess/src/pool.rs".as_ptr(), line!())
    };
}

/// `php_swoole_process_pool_minit`: register `Swoole\Process\Pool`.
/// # Safety
/// Called once, at MINIT, after the Process MINIT.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn php_swoole_process_pool_minit(module: c_int) {
    // SAFETY: Zend's MINIT of this module, on this thread.
    let minit = unsafe { Minit::new(module) };
    // SAFETY: a `zend_try` trap and Zend's bailout.
    unsafe { swoole_rs_process::pool::set_bailout(trap, raise) };
    minit.register_object::<PoolObject>(c"Swoole\\Process\\Pool", &METHODS.0, 0);
    for name in [c"master_pid", c"workerPid", c"workerId"] {
        minit.property_long(&POOL, name, -1, ZEND_ACC_PUBLIC);
    }
    minit.property_null(&POOL, c"workers", ZEND_ACC_PUBLIC);
    // Upstream declares the bools with -1 (true).
    for name in [c"workerRunning", c"running"] {
        minit.property_bool(&POOL, name, true, ZEND_ACC_PUBLIC);
    }
}

/// `timer_if_use_reactor()` inside a running pool: not in its master, nor in a
/// synchronous worker loop, which dispatches timers itself. `None` outside a pool.
pub(crate) fn timer_uses_reactor(master: bool) -> Option<bool> {
    let pool = CURRENT.load(Ordering::Acquire);
    (!pool.is_null()).then(|| {
        let pool = handle(pool);
        !master && (pool.is_async() || !pool.has_main_loop())
    })
}

fn update_bool(object: &Value, name: &std::ffi::CStr, value: bool) {
    if let Some(props) = object.props(Some(&POOL)) {
        props.bool(name, value);
    }
}

fn update_long(object: &Value, name: &std::ffi::CStr, value: i64) {
    if let Some(props) = object.props(Some(&POOL)) {
        props.long(name, value);
    }
}

fn getpid() -> i64 {
    i64::from(swoole_rs_process::os::getpid())
}

/// Call a pool callback with `$pool` (and the worker id), as upstream's
/// `zend::function::call`; warn "%s->onX handler error" on failure.
fn dispatch(
    callable: &Callable,
    object: &Value,
    worker: Option<i64>,
    coroutine: bool,
    event: &str,
) {
    let id = Owned::long(worker.unwrap_or(0));
    let args: &[&Value] = if worker.is_some() {
        &[object, &id]
    } else {
        &[object]
    };
    let called = callable.call(args, coroutine);
    if !called && !swoole_rs_zend::bailout::pending() {
        handler_error(object, event);
    }
}

/// `php_swoole_error(E_WARNING, "%s->onX handler error")`.
fn handler_error(object: &Value, event: &str) {
    notice(
        E_WARNING,
        &message!(
            "{}->{event} handler error",
            object.value_name().to_string_lossy()
        ),
    );
}

/// `process_pool_signal_handler`.
extern "C" fn on_signal(signo: c_int) {
    let pool = CURRENT.load(Ordering::Acquire);
    if pool.is_null() {
        return;
    }
    signal(pool, signo);
}

/// `ProcessPool::signal` in this process's worker (or null).
fn signal(pool: Pool, signo: c_int) {
    methods::signal(
        handle(pool),
        signo,
        CURRENT_WORKER.load(Ordering::Acquire).cast(),
    );
}

/// The object a pool callback belongs to, and its data.
fn owner<'a>(pool: Pool) -> Option<(&'a Value, &'a PoolObject)> {
    // SAFETY: the pool's copy of its object zval, alive while the pool runs.
    let object = unsafe { Value::from_ptr(handle(pool).ptr().cast()) }?;
    Some((object, object.custom()?))
}

extern "C-unwind" fn on_worker_start(pool: PoolRef<'_>, worker: Worker<'_>) {
    let (pool, worker) = (pool.raw(), worker.raw().cast::<c_void>());
    let Some((object, this)) = owner(pool) else {
        return;
    };
    clean();
    if swoole_rs_zend::bailout::pending() {
        swoole_rs_zend::bailout::finish();
        return;
    }
    CURRENT.store(pool, Ordering::Release);
    CURRENT_WORKER.store(worker, Ordering::Release);
    let id = i64::from(worker_view(worker).id);
    update_bool(object, c"running", true);
    update_bool(object, c"workerRunning", true);
    update_long(object, c"workerPid", getpid());
    update_long(object, c"workerId", id);
    methods::swoole_set_worker_type(WORKER);
    swoole_rs_corebase::swr_v1_core_set_enable_coroutine(this.enable_coroutine.get());
    if let Some(callable) = this.on_worker_start.get() {
        let coroutine = this.enable_coroutine.get();
        dispatch(&callable, object, Some(id), coroutine, "onWorkerStart");
    }
    if swoole_rs_zend::bailout::pending() {
        swoole_rs_zend::bailout::finish();
        return;
    }
    let term = this.on_message.is_set() || this.enable_coroutine.get();
    worker_signals(term);
    swoole_rs_zend::bailout::finish();
}

/// The worker's handler for SIGTERM (when `term`), SIGWINCH and SIGRTMIN, where
/// none is set.
fn worker_signals(term: bool) {
    #[cfg(any(target_os = "linux", target_os = "android"))]
    let rtmin = Some(libc::SIGRTMIN());
    #[cfg(not(any(target_os = "linux", target_os = "android")))]
    let rtmin = None;
    let signals = [term.then_some(libc::SIGTERM), Some(libc::SIGWINCH), rtmin];
    for signo in signals.into_iter().flatten() {
        if !swoole_signal_isset(signo) {
            _ = host::signal_swap(signo, SignalHandler::new(on_signal), false);
        }
    }
}

extern "C-unwind" fn on_message(pool: PoolRef<'_>, message: &RecvData) {
    let pool = pool.raw();
    let Some((object, this)) = owner(pool) else {
        return;
    };
    let data = message_data(pool, message);
    worker_busy(true);
    // The onMessage callable `start()` required.
    let callable = this.on_message.get();
    let coroutine = this.enable_coroutine.get();
    let called = callable
        .as_ref()
        .is_some_and(|callable| callable.call(&[object, &data], coroutine));
    drop(callable);
    let bailed = swoole_rs_zend::bailout::pending();
    if !called && !bailed {
        handler_error(object, "onMessage");
    }
    if !bailed {
        worker_busy(false);
    }
    drop(data);
    swoole_rs_zend::bailout::finish();
}

fn worker_stopped(pool: Pool, worker: *mut c_void, exit: bool) {
    let Some((object, this)) = owner(pool) else {
        return;
    };
    update_bool(object, c"running", false);
    update_bool(object, c"workerRunning", false);
    let (handler, event) = if exit {
        (&this.on_worker_exit, "onWorkerExit")
    } else {
        (&this.on_worker_stop, "onWorkerStop")
    };
    if let Some(callable) = handler.get() {
        let id = i64::from(worker_view(worker).id);
        dispatch(&callable, object, Some(id), false, event);
    }
}

extern "C-unwind" fn on_worker_stop(pool: PoolRef<'_>, worker: Worker<'_>) {
    worker_stopped(pool.raw(), worker.raw().cast(), false);
    swoole_rs_zend::bailout::finish();
}

extern "C-unwind" fn on_worker_exit(pool: PoolRef<'_>, worker: Worker<'_>) {
    worker_stopped(pool.raw(), worker.raw().cast(), true);
    swoole_rs_zend::bailout::finish();
}

extern "C-unwind" fn on_start(pool: PoolRef<'_>) {
    let pool = pool.raw();
    if let Some((object, this)) = owner(pool) {
        update_long(object, c"master_pid", getpid());
        update_bool(object, c"running", true);
        methods::swoole_set_worker_type(MASTER);
        swoole_rs_corebase::swr_v1_core_set_enable_coroutine(false);
        if let Some(callable) = this.on_start.get() {
            dispatch(&callable, object, None, false, "onStart");
        }
    }
    swoole_rs_zend::bailout::finish();
}

extern "C-unwind" fn on_shutdown(pool: PoolRef<'_>) {
    let pool = pool.raw();
    if let Some((object, this)) = owner(pool) {
        update_bool(object, c"running", false);
        update_bool(object, c"workerRunning", false);
        if let Some(callable) = this.on_shutdown.get() {
            dispatch(&callable, object, None, false, "onShutdown");
        }
    }
    swoole_rs_zend::bailout::finish();
}

fn construct(call: &Call) {
    let refuse = |text: String| {
        set_last_error(OPERATION_NOT_SUPPORT);
        call.throw_error(message!("{text}").as_cstr());
        call.set_bool(false);
    };
    let Some(this_value) = call.this_value() else {
        return;
    };
    let name = this_value.value_name().to_string_lossy().into_owned();
    if !cli() {
        return refuse(format!("{name} can only be used in PHP CLI mode"));
    }
    if !swoole_rs_corebase::swr_v1_core_server().is_null() {
        return refuse("cannot create server and process pool instances simultaneously".into());
    }
    if !CURRENT.load(Ordering::Acquire).is_null() {
        return refuse(
            "A process pool instance has already been created and cannot be created again".into(),
        );
    }
    if !host::main_thread() {
        set_last_error(OPERATION_NOT_SUPPORT);
        call.throw(
            exception(),
            c"This operation is only allowed in the main thread",
            -1,
        );
        call.set_bool(false);
        return;
    }
    let parsed = call.arity(1, 4).and_then(|()| {
        Ok((
            call.long(1, 0)?,
            call.long(2, IPC_NONE)?,
            call.long(3, 0)?,
            call.bool(4, false)?,
        ))
    });
    let Ok((worker_num, ipc_type, key, coroutine)) = parsed else {
        call.set_bool(false);
        return;
    };
    let Some(pool) = this(call) else {
        return;
    };
    let fail = |text: &std::ffi::CStr| {
        call.throw(exception(), text, i64::from(crate::report::errno()));
        call.set_bool(false);
    };
    if !pool.pool.get().is_null() {
        call.throw_constructed_twice();
        call.set_bool(false);
        return;
    }
    if worker_num <= 0 {
        return fail(c"the parameter $worker_num must be greater than 0");
    }
    if ipc_type == IPC_MSGQUEUE && !crate::process::key_is_valid(key) {
        return fail(c"the parameter $msgqueue_key is out of range");
    }
    if coroutine && ipc_type > 0 && ipc_type != IPC_UNIXSOCK {
        call.throw_error(
            c"the parameter $ipc_type must be SWOOLE_IPC_UNIXSOCK when enable coroutine",
        );
        call.set_bool(false);
        return;
    }
    let made = pool_new(
        worker_num.c_cast::<u32>(),
        key.c_cast::<c_int>(),
        ipc_type.c_cast::<c_int>(),
        this_value,
        coroutine,
    );
    let Some(made) = made else {
        return fail(c"failed to create process pool");
    };
    pool.enable_coroutine.set(coroutine);
    pool.pool.set(made);
}

fn destruct(_call: &Call) {}

fn set(call: &Call) {
    let Ok(options) = call.arity(1, 1).and_then(|()| call.array_ref(1)) else {
        call.set_bool(false);
        return;
    };
    let this = this(call);
    // Each conversion below can run PHP code, which can call this object's
    // methods: the record is borrowed per write, after its value converted.
    let pool = checked(this);
    swoole_rs_phpswoole::php_swoole_set_global_option(options);
    swoole_rs_phpcoroutines::set_coroutine_option(options);
    // Each setter stops at a conversion's bailout, which upstream's ended the
    // call at; the handler raises it.
    if swoole_rs_zend::bailout::pending() {
        return;
    }
    swoole_rs_phpswoole::php_swoole_set_aio_option(options);
    if swoole_rs_zend::bailout::pending() {
        return;
    }
    if let Some(value) = options.get(b"max_package_size") {
        let size = swoole_rs_phpswoole::php_swoole_parse_to_size(value);
        let pool = handle(pool);
        pool.set_max_packet_size(packet_limit(pool.use_msgqueue(), max_package_size(size)));
    }
    if let Some(value) = options.get(b"max_wait_time") {
        let Some(seconds) = value.applied_long() else {
            return;
        };
        handle(pool).set_max_wait_time(
            u32::try_from(seconds.clamp(0, u32::MAX.into())).unwrap_or(u32::MAX),
        );
    }
    let Some(this) = this else {
        return;
    };
    if let Some(value) = options.get(b"enable_coroutine") {
        let enable = value.truthy();
        this.enable_coroutine.set(enable);
        handle(pool).set_is_async(enable);
    }
    if let Some(value) = options.get(b"enable_message_bus") {
        this.enable_message_bus.set(value.truthy());
    }
}

fn on(call: &Call) {
    let this = this(call);
    let pool = checked(this);
    let Some(this) = this else {
        return;
    };
    let view = handle(pool);
    if view.started() {
        fatal(
            E_WARNING,
            &message!("process pool is started. unable to register event callback function"),
        );
        call.set_bool(false);
        return;
    }
    let Ok(name) = call.arity(2, 2).and_then(|()| call.string(1, b"")) else {
        call.set_bool(false);
        return;
    };
    let Some(callback) = call.value(2) else {
        call.set_bool(false);
        return;
    };
    let slot = match name.to_ascii_lowercase().as_slice() {
        b"workerstart" => &this.on_worker_start,
        b"message" => {
            if i64::from(view.ipc_mode()) == IPC_NONE {
                call.throw(
                    exception(),
                    c"cannot set `onMessage` event with ipc_type=0",
                    i64::from(INVALID_PARAMS),
                );
                call.set_bool(false);
                return;
            }
            &this.on_message
        }
        b"workerstop" => &this.on_worker_stop,
        b"workerexit" => &this.on_worker_exit,
        b"start" => &this.on_start,
        b"shutdown" => &this.on_shutdown,
        _ => {
            notice(
                E_WARNING,
                &message!("unknown event type[{}]", String::from_utf8_lossy(name)),
            );
            call.set_bool(false);
            return;
        }
    };
    // The slot lets go first: a destructor that bails out leaves it empty.
    if let Some(old) = slot.replace(None) {
        drop(old);
        if swoole_rs_zend::bailout::pending() {
            return;
        }
    }
    let created = Callable::create(callback);
    // The old callable's destructor may have called on() for this event: this
    // call's callable replaces the one that stored, which drops.
    drop(slot.replace(created));
    call.set_bool(true);
}

fn unsupported(call: &Call, view: PoolRef<'_>) {
    fatal(
        E_WARNING,
        &message!("unsupported ipc type[{}]", view.ipc_mode()),
    );
    call.set_bool(false);
}

fn listen(call: &Call) {
    let pool = checked(this(call));
    let view = handle(pool);
    if view.started() {
        fatal(
            E_WARNING,
            &message!("process pool is started. unable to listen"),
        );
        call.set_bool(false);
        return;
    }
    let parsed = call
        .arity(1, 3)
        .and_then(|()| Ok((call.string(1, b"")?, call.long(2, 0)?, call.long(3, 2048)?)));
    let Ok((address, port, backlog)) = parsed else {
        call.set_bool(false);
        return;
    };
    if i64::from(view.ipc_mode()) != IPC_SOCKET {
        return unsupported(call, view);
    }
    let unix = address
        .get(..6)
        .is_some_and(|head| head.eq_ignore_ascii_case(b"unix:/"));
    // The address up to its first NUL, as C reads it; a unix path skips "unix:".
    let host = address.get(if unix { 5 } else { 0 }..).unwrap_or_default();
    let host =
        CString::new(host.split(|&byte| byte == 0).next().unwrap_or_default()).unwrap_or_default();
    let kind = if unix { SOCK_UNIX_STREAM } else { SOCK_TCP };
    let listened = methods::listen_socket(
        handle(pool),
        kind,
        &host,
        port.c_cast::<c_int>(),
        backlog.c_cast::<c_int>(),
    );
    let socket = handle(pool).ipc_listener();
    if !socket.is_null() {
        // SAFETY: the pool's live listener record, which nothing else borrows here.
        socket_set_fd_option(unsafe { &mut *socket.cast() }, 0, 1);
    }
    call.set_bool(listened >= 0);
}

fn write(call: &Call) {
    let Ok(data) = call.arity(1, 1).and_then(|()| call.string(1, b"")) else {
        call.set_bool(false);
        return;
    };
    let pool = checked(this(call));
    let view = handle(pool);
    if i64::from(view.ipc_mode()) != IPC_SOCKET {
        return unsupported(call, view);
    }
    if data.is_empty() {
        call.set_bool(false);
        return;
    }
    let length = data.len().c_cast::<u32>() as usize;
    let written = methods::response(view, data.get(..length).unwrap_or(data));
    call.set_bool(written >= 0);
}

fn send_message(call: &Call) {
    let pool = checked(this(call));
    let view = handle(pool);
    if !view.started() {
        fatal(E_WARNING, &message!("process pool is not started."));
        call.set_bool(false);
        return;
    }
    if i64::from(view.ipc_mode()) != IPC_UNIXSOCK {
        return unsupported(call, view);
    }
    let parsed = call
        .arity(2, 2)
        .and_then(|()| Ok((call.string(1, b"")?, call.long(2, 0)?)));
    let Ok((data, worker)) = parsed else {
        call.set_bool(false);
        return;
    };
    let sent = methods::send_message(view, worker.c_cast::<u32>(), data);
    call.set_bool(sent);
}

/// The signals `start()` takes over while the pool runs.
fn pool_signals() -> [c_int; 6] {
    #[cfg(any(target_os = "linux", target_os = "android"))]
    let rtmin = libc::SIGRTMIN();
    #[cfg(not(any(target_os = "linux", target_os = "android")))]
    let rtmin = 0;
    [
        libc::SIGTERM,
        libc::SIGUSR1,
        libc::SIGUSR2,
        libc::SIGIO,
        libc::SIGWINCH,
        rtmin,
    ]
}

fn start(call: &Call) {
    let this = this(call);
    let pool = checked(this);
    let Some(this) = this else {
        return;
    };
    if handle(pool).started() {
        fatal(E_WARNING, &message!("process pool is started"));
        call.set_bool(false);
        return;
    }
    // The reactor must be cleaned up before registering signals.
    swoole_rs_reactor::swoole_event_free();
    let signals = pool_signals();
    let mut previous = [SignalHandler::NONE; 6];
    for (signo, old) in signals.iter().zip(previous.iter_mut()) {
        if *signo != 0 {
            *old = host::signal_swap(*signo, SignalHandler::new(on_signal), false);
        }
    }
    if !protocol(pool, this.enable_message_bus.get()) {
        call.set_bool(false);
        return;
    }
    let view = handle(pool);
    if !this.on_worker_start.is_set() && !this.on_message.is_set() {
        if view.is_async() {
            fatal(E_ERROR, &message!("require 'onWorkerStart' callback"));
            call.set_bool(false);
            return;
        } else if i64::from(view.ipc_mode()) != IPC_NONE {
            fatal(E_ERROR, &message!("require 'onMessage' callback"));
            call.set_bool(false);
            return;
        }
    }
    if this.on_worker_exit.is_set() && !this.enable_coroutine.get() {
        call.throw(
            exception(),
            c"cannot set `onWorkerExit` without enable_coroutine",
            i64::from(INVALID_PARAMS),
        );
        call.set_bool(false);
        return;
    }
    CURRENT.store(pool, Ordering::Release);
    let record = handle(pool);
    let with_message = this.on_message.is_set();
    if !with_message {
        record.clear_main_loop();
    }
    let with_exit = this.enable_coroutine.get() && this.on_worker_exit.is_set();
    let message: MessageFn = on_message;
    let exit: WorkerFn = on_worker_exit;
    let hooks = Hooks {
        on_message: with_message.then_some(message),
        on_start: Some(on_start),
        on_shutdown: Some(on_shutdown),
        on_worker_start: Some(on_worker_start),
        on_worker_stop: Some(on_worker_stop),
        on_worker_exit: with_exit.then_some(exit),
        ..Hooks::default()
    };
    // SAFETY: each hook reads the pool's `ptr` as the copy of its object `pool_new`
    // stored (`owner`), which lives until `pool_free` destroys the pool.
    unsafe { record.install(&hooks) };
    if methods::start(handle(pool)) < 0 {
        call.set_bool(false);
        return;
    }
    methods::wait(handle(pool));
    CURRENT.store(ptr::null_mut(), Ordering::Release);
    for (signo, old) in signals.iter().zip(previous) {
        if *signo != 0 {
            // Restores the handler `start()` replaced.
            _ = host::signal_swap(*signo, old, false);
        }
    }
}

/// The pool's protocol: messages over a bus whose packets are zend strings, or
/// a stream.
fn protocol(pool: Pool, message_bus: bool) -> bool {
    if !message_bus {
        methods::set_protocol(handle(pool), PROTOCOL_STREAM);
        return true;
    }
    if handle(pool).message_bus().is_null() && methods::create_message_bus(handle(pool)) != 0 {
        return false;
    }
    let bus = handle(pool).message_bus().cast::<BusHead>();
    // SAFETY: the pool's message bus, whose allocator its next buffer uses.
    unsafe { (*bus).allocator = swoole_rs_phpswoole::sw_zend_string_allocator() };
    methods::set_protocol(handle(pool), PROTOCOL_MESSAGE);
    true
}

fn detach(call: &Call) {
    let pool = CURRENT.load(Ordering::Acquire);
    if pool.is_null() {
        call.set_bool(false);
        return;
    }
    call.set_bool(methods::detach(handle(pool)));
}

fn get_process(call: &Call) {
    let pool = CURRENT.load(Ordering::Acquire);
    if pool.is_null() {
        call.set_bool(false);
        return;
    }
    let Ok(mut id) = call.arity(0, 1).and_then(|()| call.long(1, -1)) else {
        call.set_bool(false);
        return;
    };
    let view = handle(pool);
    if id >= i64::from(view.worker_num()) {
        notice(E_WARNING, &message!("invalid worker_id[{id}]"));
        call.set_bool(false);
        return;
    }
    let current_id = i64::from(methods::swoole_get_worker_id());
    if id < 0 {
        id = current_id;
    }
    // `$this`, of the registered class.
    let Some(props) = call.props(Some(&POOL)) else {
        return;
    };
    // The property reads null meanwhile; creating a Process object runs no PHP code.
    props.edit_array(c"workers", |workers| {
        // Separated: a new worker is added in place.
        if workers.as_mut_ptr().is_none() {
            return;
        }
        let key = id.cast_unsigned();
        let class = crate::process::process_class();
        let existing = workers.array().and_then(|table| table.index(key));
        match existing.filter(|found| !found.is_null()) {
            None => {
                let object = class.create_object();
                // A worker C++ allocates, as the Process object frees it.
                let mut copy = WorkerRecord::boxed();
                #[expect(
                    clippy::cast_sign_loss,
                    clippy::cast_possible_truncation,
                    reason = "a checked worker id"
                )]
                let pipe = worker_record::copy_for_process(view, id as usize, &mut copy);
                // The Process object takes the copy.
                let worker = Box::into_raw(copy).cast::<c_void>();
                if let Some(props) = object.props(Some(class)) {
                    props.long(c"id", current_id);
                    props.long(c"pid", i64::from(worker_pid(worker)));
                    if i64::from(view.ipc_mode()) == IPC_UNIXSOCK {
                        props.long(c"pipe", i64::from(pipe));
                    }
                }
                // Upstream passes PIPE_TYPE_STREAM as enable_coroutine and async as the pipe type.
                set_worker_of(&object, worker, true, c_int::from(view.is_async()));
                // The table takes the new object's reference.
                workers.update_index(key, object);
            }
            Some(found) => {
                if found.object().is_some() {
                    let worker = crate::process::worker_of(found);
                    #[expect(
                        clippy::cast_sign_loss,
                        clippy::cast_possible_truncation,
                        reason = "a checked worker id"
                    )]
                    let pid = view.worker(id as usize).map_or(0, Worker::pid);
                    if worker_pid(worker) != pid {
                        crate::worker::with(worker, |record| record.set_pid(pid));
                        if let Some(props) = found.props(Some(class)) {
                            props.long(c"pid", i64::from(pid));
                        }
                    }
                }
            }
        }
        // The return value shares the element (`ZVAL_COPY`).
        if let Some(found) = workers.array().and_then(|table| table.index(key)) {
            call.set_owned(found.copy());
        }
    });
}

fn stop(_call: &Call) {
    let pool = CURRENT.load(Ordering::Acquire);
    if !pool.is_null() {
        // SIGTERM's action: stop running and stop this worker.
        signal(pool, libc::SIGTERM);
    }
}

fn shutdown(call: &Call) {
    let pool = CURRENT.load(Ordering::Acquire);
    if pool.is_null() {
        call.throw(
            exception(),
            c"The process pool is not started",
            i64::from(INVALID_PARAMS),
        );
        call.set_bool(false);
        return;
    }
    call.set_bool(methods::shutdown(handle(pool)));
}

handlers! {
    zim_swoole_process_pool___construct => construct;
    zim_swoole_process_pool___destruct => destruct;
    zim_swoole_process_pool_set => set;
    zim_swoole_process_pool_on => on;
    zim_swoole_process_pool_getProcess => get_process;
    zim_swoole_process_pool_listen => listen;
    zim_swoole_process_pool_write => write;
    zim_swoole_process_pool_sendMessage => send_message;
    zim_swoole_process_pool_detach => detach;
    zim_swoole_process_pool_start => start;
    zim_swoole_process_pool_stop => stop;
    zim_swoole_process_pool_shutdown => shutdown;
}
