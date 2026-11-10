//! `Swoole\Process\Pool`: registration, objects, methods and the pool's event
//! callbacks, over crates/process's `ProcessPool` (`PoolRecord`).
use crate::{
    host,
    process::{entry as process_entry, set_worker_of, worker_pid, worker_view},
    report::{INVALID_PARAMS, OPERATION_NOT_SUPPORT, cli, fatal, notice, set_last_error},
    signal::clean,
};
use std::{
    ffi::{c_int, c_void},
    ptr,
    sync::atomic::{AtomicPtr, Ordering},
};
use swoole_rs_corebase::DataHead;
use swoole_rs_phpstubs::process_pool;
use swoole_rs_process::worker_record::Worker;
use swoole_rs_process::{
    methods,
    pool::swr_v1_process_pool_ipc_listener,
    process_pool::{PROTOCOL_MESSAGE, PROTOCOL_STREAM, PoolRecord, max_package_size, packet_limit},
    signal::swoole_signal_isset,
    worker_globals::thread_globals,
    worker_record,
};
use swoole_rs_reactor::socket_upstream::socket_set_fd_option;
use swoole_rs_zend::{
    Call, Class, E_ERROR, E_WARNING, Object, class_name, handlers, long_zval, message, object_of,
    string,
    sys::{
        IS_ARRAY, IS_NULL, ZEND_ACC_NOT_SERIALIZABLE, ZEND_ACC_PUBLIC, object_init_ex,
        swr_zend_array_init, zend_class_entry, zend_declare_property_bool, zend_hash_index_find,
        zend_hash_index_update, zend_object, zend_object_handlers, zend_read_property, zval,
        zval_ptr_dtor,
    },
    throw_constructed_twice, throw_error, throw_exception, type_of, unset_property_deny,
    update_property_long, value,
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

/// `swoole::RecvData`.
#[repr(C)]
struct RecvData {
    info: DataHead,
    data: *mut u8,
}

type Pool = *mut PoolRecord;

/// `swoole::MessageBus`'s first members: its Rust bus, then its allocator.
#[repr(C)]
struct BusHead {
    bus: *mut c_void,
    allocator: *const c_void,
}

// `swoole_signal_set(signo, handler, safety)` (crates/reactor).
use swoole_rs_reactor::signal_upstream::swoole_signal_set;

static POOL: Class = Class::new();
/// `current_pool` and `current_worker`.
static CURRENT: AtomicPtr<PoolRecord> = AtomicPtr::new(ptr::null_mut());
static CURRENT_WORKER: AtomicPtr<c_void> = AtomicPtr::new(ptr::null_mut());

fn exception() -> *mut zend_class_entry {
    host::exception()
}

/// The fields of a pool this module created.
fn record<'a>(pool: Pool) -> &'a mut PoolRecord {
    // SAFETY: callers pass a live pool.
    unsafe { &mut *pool }
}

/// `ProcessPool::create` in a zeroed pool that keeps a copy of `$this`'s zval;
/// on failure, none, with `errno` set.
fn pool_new(
    worker_num: u32,
    key: c_int,
    ipc: c_int,
    object: *mut zval,
    async_: bool,
) -> Option<Pool> {
    // SAFETY: every `ProcessPool` field is an integer, a bool or a pointer.
    let pool = Box::into_raw(unsafe { Box::<PoolRecord>::new_zeroed().assume_init() });
    // SAFETY: a zeroed pool.
    if unsafe { methods::create(pool, worker_num, key, ipc) } < 0 {
        // SAFETY: the allocation above, which nothing else holds.
        drop(unsafe { Box::from_raw(pool) });
        return None;
    }
    // SAFETY: a live zval, copied without a reference as upstream's `sw_zval_dup`.
    record(pool).ptr = Box::into_raw(Box::new(unsafe { object.read() })).cast();
    record(pool).is_async = async_;
    Some(pool)
}

fn pool_free(pool: Pool) {
    // SAFETY: the zval copy `pool_new` made.
    drop(unsafe { Box::from_raw(record(pool).ptr.cast::<zval>()) });
    // SAFETY: a created pool.
    unsafe { methods::destroy(pool) };
    // SAFETY: the allocation `pool_new` made.
    drop(unsafe { Box::from_raw(pool) });
}

/// `SwooleWG.worker`'s status, and its request count once idle again.
fn worker_busy(busy: bool) {
    // SAFETY: this thread's worker globals.
    let worker = unsafe { (*thread_globals()).worker };
    // SAFETY: this process's worker.
    let worker = unsafe { Worker::new(worker) };
    if busy {
        worker.set_status(WORKER_BUSY);
    } else {
        worker.add_request_count();
        worker.set_status(WORKER_IDLE);
    }
}

/// The message's payload as a string: the bus's packet itself when it holds one.
fn message_data(pool: Pool, message: *mut c_void, target: *mut zval) {
    // SAFETY: the pool's received message.
    let message = unsafe { &*message.cast::<RecvData>() };
    let (length, data) = (message.info.len as usize, message.data);
    let string = if length == 0 {
        string::copy(b"")
    } else if message.info.flags & EVENT_DATA_OBJ_PTR != 0 {
        let string = string::from_val(data);
        // SAFETY: the bus allocates its packets as zend strings of at least `length` bytes.
        unsafe { string::truncate(string, length) };
        // SAFETY: an OBJ_PTR message comes from the pool's live message bus; the
        // string is now the caller's.
        unsafe { swoole_rs_server::bus_move_packet(record(pool).message_bus) };
        string
    } else {
        // SAFETY: `length` bytes at `data`.
        string::copy(unsafe { std::slice::from_raw_parts(data, length) })
    };
    // SAFETY: a writable zval takes the string.
    unsafe { value::set_str(target, string) };
}

/// `struct ProcessPoolObject`: the pool, its six callables and two flags.
#[repr(C)]
struct PoolObject {
    pool: Pool,
    on_start: *mut c_void,
    on_shutdown: *mut c_void,
    on_worker_start: *mut c_void,
    on_worker_stop: *mut c_void,
    on_worker_exit: *mut c_void,
    on_message: *mut c_void,
    enable_coroutine: bool,
    enable_message_bus: bool,
}

type Object_ = Object<PoolObject>;

/// # Safety
/// `object` is a live Pool object.
unsafe fn data<'a>(object: *mut zend_object) -> &'a mut PoolObject {
    // SAFETY: per the caller.
    unsafe { Object_::data(object) }
}

fn this<'a>(call: &Call) -> &'a mut PoolObject {
    // SAFETY: Pool methods are non-static, so `$this` is a Pool object.
    unsafe { data(call.this()) }
}

/// `process_pool_get_and_check_pool`.
fn checked(object: &PoolObject) -> Pool {
    if object.pool.is_null() {
        host::unconstructed();
    }
    object.pool
}

fn view<'a>(pool: Pool) -> &'a PoolRecord {
    record(pool)
}

unsafe extern "C" fn create(ce: *mut zend_class_entry) -> *mut zend_object {
    // SAFETY: Zend calls `create_object` with this class or a subclass; zeroed
    // data is a pool object without a pool or callables.
    unsafe { Object_::create(&POOL, ce) }
}

unsafe extern "C" fn free(object: *mut zend_object) {
    // SAFETY: Zend frees an object this class created.
    let pool = unsafe { data(object) };
    if !pool.pool.is_null() {
        pool_free(pool.pool);
    }
    for callable in [
        pool.on_worker_start,
        pool.on_message,
        pool.on_worker_stop,
        pool.on_start,
        pool.on_worker_exit,
        pool.on_shutdown,
    ] {
        if !callable.is_null() {
            // SAFETY: the object owns its callables.
            unsafe { host::callable_free(callable) };
        }
    }
    // SAFETY: as above.
    unsafe { Object_::dtor(object) };
    swoole_rs_zend::bailout::finish();
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
pub unsafe extern "C" fn php_swoole_process_pool_minit(_module: c_int) {
    // SAFETY: a `zend_try` trap and Zend's bailout.
    unsafe { swoole_rs_process::pool::set_bailout(trap, raise) };
    // SAFETY: MINIT, with a static table.
    unsafe { POOL.register(c"Swoole\\Process\\Pool", METHODS.as_ptr(), configure) };
    for name in [c"master_pid", c"workerPid", c"workerId"] {
        // SAFETY: MINIT, after `register`.
        unsafe { POOL.property_long(name, -1) };
    }
    // SAFETY: as above.
    unsafe { POOL.property_null(c"workers") };
    for name in [c"workerRunning", c"running"] {
        #[expect(clippy::cast_possible_wrap, reason = "a small flag")]
        // SAFETY: as above; upstream declares the bools with -1 (true).
        unsafe {
            zend_declare_property_bool(
                POOL.entry(),
                name.as_ptr(),
                name.count_bytes(),
                -1,
                ZEND_ACC_PUBLIC as c_int,
            );
        }
    }
}

/// `timer_if_use_reactor()` inside a running pool: not in its master, nor in a
/// synchronous worker loop, which dispatches timers itself. `None` outside a pool.
pub(crate) fn timer_uses_reactor(master: bool) -> Option<bool> {
    let pool = CURRENT.load(Ordering::Acquire);
    (!pool.is_null()).then(|| {
        let pool = record(pool);
        !master && (pool.is_async || pool.main_loop.is_null())
    })
}

fn configure(entry: &mut zend_class_entry, handlers: &mut zend_object_handlers) {
    entry.ce_flags |= ZEND_ACC_NOT_SERIALIZABLE;
    handlers.clone_obj = None;
    handlers.unset_property = Some(unset_property_deny);
    Object_::install(entry, handlers, create, free);
}

fn update_bool(object: *mut zend_object, name: &std::ffi::CStr, value: bool) {
    // SAFETY: a live Pool object.
    unsafe { swoole_rs_zend::Props::new(POOL.entry(), object) }.bool(name, value);
}

fn update_long(object: *mut zend_object, name: &std::ffi::CStr, value: i64) {
    // SAFETY: a live Pool object.
    unsafe { update_property_long(POOL.entry(), object, name, value) };
}

fn getpid() -> i64 {
    i64::from(swoole_rs_process::os::getpid())
}

/// Call a pool callback with `$pool` (and the worker id), as upstream's
/// `zend::function::call`; warn "%s->onX handler error" on failure.
fn dispatch(
    callable: *mut c_void,
    object: *mut zval,
    worker: Option<i64>,
    coroutine: bool,
    event: &str,
) {
    // SAFETY: a live object zval, copied without a reference as upstream does.
    let mut args = [unsafe { *object }, long_zval(worker.unwrap_or(0))];
    let argc = if worker.is_some() { 2 } else { 1 };
    // SAFETY: a live callable and `argc` arguments.
    let called = unsafe { host::callable_call(callable, argc, args.as_mut_ptr(), coroutine) };
    if !called && !swoole_rs_zend::bailout::pending() {
        // SAFETY: a live object zval.
        let std = unsafe { object_of(object) };
        // SAFETY: a live object.
        let name = unsafe { class_name(std) };
        notice(
            E_WARNING,
            &message!("{}->{event} handler error", name.to_string_lossy()),
        );
    }
}

/// `process_pool_signal_handler`.
unsafe extern "C" fn on_signal(signo: c_int) {
    let pool = CURRENT.load(Ordering::Acquire);
    if pool.is_null() {
        return;
    }
    signal(pool, signo);
}

/// `ProcessPool::signal` in this process's worker (or null).
fn signal(pool: Pool, signo: c_int) {
    // SAFETY: the running pool and this process's worker (or null).
    unsafe { methods::signal(pool, signo, CURRENT_WORKER.load(Ordering::Acquire).cast()) };
}

/// The object a pool callback belongs to.
fn owner(pool: Pool) -> (*mut zval, *mut zend_object, &'static mut PoolObject) {
    // The pool's object zval, alive while the pool runs.
    let object = record(pool).ptr.cast::<zval>();
    // SAFETY: as above.
    let std = unsafe { object_of(object) };
    // SAFETY: a Pool object.
    (object, std, unsafe { data(std) })
}

unsafe extern "C" fn on_worker_start(pool: Pool, worker: *mut c_void) {
    let (object, std, this) = owner(pool);
    clean();
    if swoole_rs_zend::bailout::pending() {
        swoole_rs_zend::bailout::finish();
        return;
    }
    CURRENT.store(pool, Ordering::Release);
    CURRENT_WORKER.store(worker, Ordering::Release);
    let id = i64::from(worker_view(worker).id);
    update_bool(std, c"running", true);
    update_bool(std, c"workerRunning", true);
    update_long(std, c"workerPid", getpid());
    update_long(std, c"workerId", id);
    methods::swoole_set_worker_type(WORKER);
    swoole_rs_corebase::swr_v1_core_set_enable_coroutine(this.enable_coroutine);
    if !this.on_worker_start.is_null() {
        dispatch(
            this.on_worker_start,
            object,
            Some(id),
            this.enable_coroutine,
            "onWorkerStart",
        );
    }
    if swoole_rs_zend::bailout::pending() {
        swoole_rs_zend::bailout::finish();
        return;
    }
    let term = !this.on_message.is_null() || this.enable_coroutine;
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
            // SAFETY: a valid signal and handler.
            _ = unsafe { swoole_signal_set(signo, on_signal as *const () as usize, false) };
        }
    }
}

unsafe extern "C" fn on_message(pool: Pool, message: *mut c_void) {
    let (object, _, this) = owner(pool);
    // SAFETY: a live object zval, copied without a reference as upstream does.
    let mut args = [unsafe { *object }, long_zval(0)];
    message_data(pool, message, &raw mut args[1]);
    worker_busy(true);
    // SAFETY: the onMessage callable `start()` required, with two arguments.
    let called = unsafe {
        host::callable_call(this.on_message, 2, args.as_mut_ptr(), this.enable_coroutine)
    };
    let bailed = swoole_rs_zend::bailout::pending();
    if !called && !bailed {
        // SAFETY: a live object zval.
        let std = unsafe { object_of(object) };
        // SAFETY: a live object.
        let name = unsafe { class_name(std) };
        notice(
            E_WARNING,
            &message!("{}->onMessage handler error", name.to_string_lossy()),
        );
    }
    if !bailed {
        worker_busy(false);
    }
    // SAFETY: the string argument this callback owns.
    unsafe { zval_ptr_dtor(&raw mut args[1]) };
    swoole_rs_zend::bailout::finish();
}

fn worker_stopped(pool: Pool, worker: *mut c_void, exit: bool) {
    let (object, std, this) = owner(pool);
    update_bool(std, c"running", false);
    update_bool(std, c"workerRunning", false);
    let (callable, event) = if exit {
        (this.on_worker_exit, "onWorkerExit")
    } else {
        (this.on_worker_stop, "onWorkerStop")
    };
    if !callable.is_null() {
        let id = i64::from(worker_view(worker).id);
        dispatch(callable, object, Some(id), false, event);
    }
}

unsafe extern "C" fn on_worker_stop(pool: Pool, worker: *mut c_void) {
    worker_stopped(pool, worker, false);
    swoole_rs_zend::bailout::finish();
}

unsafe extern "C" fn on_worker_exit(pool: Pool, worker: *mut c_void) {
    worker_stopped(pool, worker, true);
    swoole_rs_zend::bailout::finish();
}

unsafe extern "C" fn on_start(pool: Pool) {
    let (object, std, this) = owner(pool);
    update_long(std, c"master_pid", getpid());
    update_bool(std, c"running", true);
    methods::swoole_set_worker_type(MASTER);
    swoole_rs_corebase::swr_v1_core_set_enable_coroutine(false);
    if !this.on_start.is_null() {
        dispatch(this.on_start, object, None, false, "onStart");
    }
    swoole_rs_zend::bailout::finish();
}

unsafe extern "C" fn on_shutdown(pool: Pool) {
    let (object, std, this) = owner(pool);
    update_bool(std, c"running", false);
    update_bool(std, c"workerRunning", false);
    if !this.on_shutdown.is_null() {
        dispatch(this.on_shutdown, object, None, false, "onShutdown");
    }
    swoole_rs_zend::bailout::finish();
}

fn construct(call: &Call) {
    let object = call.this();
    let refuse = |text: String| {
        set_last_error(OPERATION_NOT_SUPPORT);
        // SAFETY: in a handler.
        unsafe { throw_error(message!("{text}").as_cstr()) };
        call.set_bool(false);
    };
    // SAFETY: a live object.
    let name = unsafe { class_name(object) }.to_string_lossy().into_owned();
    // SAFETY: C++'s reader of an ini flag.
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
        // SAFETY: in a handler, with Swoole's exception class.
        unsafe {
            throw_exception(
                exception(),
                c"This operation is only allowed in the main thread",
                -1,
            );
        }
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
    let pool = this(call);
    let fail = |text: &std::ffi::CStr| {
        // SAFETY: in a handler, with Swoole's exception class.
        unsafe { throw_exception(exception(), text, i64::from(crate::report::errno())) };
        call.set_bool(false);
    };
    if !pool.pool.is_null() {
        // SAFETY: in a handler, with `$this`.
        unsafe { throw_constructed_twice(object) };
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
        // SAFETY: in a handler.
        unsafe {
            throw_error(
                c"the parameter $ipc_type must be SWOOLE_IPC_UNIXSOCK when enable coroutine",
            );
        };
        call.set_bool(false);
        return;
    }
    #[expect(
        clippy::cast_possible_truncation,
        clippy::cast_sign_loss,
        reason = "upstream converts to uint32_t, key_t and swIPCMode"
    )]
    let made = pool_new(
        worker_num as u32,
        key as c_int,
        ipc_type as c_int,
        call.this_zval(),
        coroutine,
    );
    let Some(made) = made else {
        return fail(c"failed to create process pool");
    };
    pool.enable_coroutine = coroutine;
    pool.pool = made;
}

fn destruct(_call: &Call) {}

fn set(call: &Call) {
    let Ok(options) = call.arity(1, 1).and_then(|()| call.array_ref(1)) else {
        call.set_bool(false);
        return;
    };
    let this = this(call);
    let pool = record(checked(this));
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
        pool.max_packet_size = packet_limit(pool.use_msgqueue != 0, max_package_size(size));
    }
    if let Some(value) = options.get(b"max_wait_time") {
        let Some(seconds) = value.applied_long() else {
            return;
        };
        pool.max_wait_time = u32::try_from(seconds.clamp(0, u32::MAX.into())).unwrap_or(u32::MAX);
    }
    if let Some(value) = options.get(b"enable_coroutine") {
        this.enable_coroutine = value.truthy();
        pool.is_async = this.enable_coroutine;
    }
    if let Some(value) = options.get(b"enable_message_bus") {
        this.enable_message_bus = value.truthy();
    }
}

fn on(call: &Call) {
    let this = this(call);
    let pool = checked(this);
    let view = view(pool);
    if view.started {
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
    let callback = call.zval(2);
    let slot = match name.to_ascii_lowercase().as_slice() {
        b"workerstart" => &mut this.on_worker_start,
        b"message" => {
            if i64::from(view.ipc_mode) == IPC_NONE {
                // SAFETY: in a handler, with Swoole's exception class.
                unsafe {
                    throw_exception(
                        exception(),
                        c"cannot set `onMessage` event with ipc_type=0",
                        i64::from(INVALID_PARAMS),
                    );
                }
                call.set_bool(false);
                return;
            }
            &mut this.on_message
        }
        b"workerstop" => &mut this.on_worker_stop,
        b"workerexit" => &mut this.on_worker_exit,
        b"start" => &mut this.on_start,
        b"shutdown" => &mut this.on_shutdown,
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
    let old = std::mem::replace(slot, ptr::null_mut());
    if !old.is_null() {
        // SAFETY: the callable this slot owned.
        unsafe { host::callable_free(old) };
        if swoole_rs_zend::bailout::pending() {
            return;
        }
    }
    // SAFETY: a passed argument.
    let created = unsafe { host::callable_create(callback) };
    // The old callable's destructor may have called on() for this event: this
    // call's callable replaces the one that stored.
    let nested = std::mem::replace(slot, created);
    if !nested.is_null() {
        // SAFETY: the nested call's callable, which no slot holds now.
        unsafe { host::callable_free(nested) };
    }
    call.set_bool(true);
}

fn unsupported(call: &Call, view: &PoolRecord) {
    fatal(
        E_WARNING,
        &message!("unsupported ipc type[{}]", view.ipc_mode),
    );
    call.set_bool(false);
}

fn listen(call: &Call) {
    let pool = checked(this(call));
    let view = view(pool);
    if view.started {
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
    if i64::from(view.ipc_mode) != IPC_SOCKET {
        return unsupported(call, view);
    }
    let unix = address
        .get(..6)
        .is_some_and(|head| head.eq_ignore_ascii_case(b"unix:/"));
    // The argument string is NUL-terminated in place; a unix path skips "unix:".
    let host_ptr = address.as_ptr().wrapping_add(if unix { 5 } else { 0 });
    let kind = if unix { SOCK_UNIX_STREAM } else { SOCK_TCP };
    #[expect(
        clippy::cast_possible_truncation,
        reason = "upstream's int port and backlog"
    )]
    // SAFETY: a created pool and a NUL-terminated address.
    let listened = unsafe {
        methods::listen(
            pool,
            kind,
            host_ptr.cast(),
            port as c_int,
            backlog as c_int,
            swoole_rs_tls::upstream::make_server_socket,
        )
    };
    // SAFETY: the pool's IPC state.
    let socket = unsafe { swr_v1_process_pool_ipc_listener(record(pool).ipc) };
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
    let view = view(pool);
    if i64::from(view.ipc_mode) != IPC_SOCKET {
        return unsupported(call, view);
    }
    if data.is_empty() {
        call.set_bool(false);
        return;
    }
    #[expect(
        clippy::cast_possible_truncation,
        reason = "upstream's uint32_t length"
    )]
    // SAFETY: a created pool and the argument's bytes.
    let written = unsafe { methods::response(pool, data.as_ptr(), data.len() as u32) };
    call.set_bool(written >= 0);
}

fn send_message(call: &Call) {
    let pool = checked(this(call));
    let view = view(pool);
    if !view.started {
        fatal(E_WARNING, &message!("process pool is not started."));
        call.set_bool(false);
        return;
    }
    if i64::from(view.ipc_mode) != IPC_UNIXSOCK {
        return unsupported(call, view);
    }
    let parsed = call
        .arity(2, 2)
        .and_then(|()| Ok((call.string(1, b"")?, call.long(2, 0)?)));
    let Ok((data, worker)) = parsed else {
        call.set_bool(false);
        return;
    };
    #[expect(
        clippy::cast_possible_truncation,
        clippy::cast_sign_loss,
        reason = "upstream's WorkerId"
    )]
    // SAFETY: a started pool and the argument's bytes.
    let sent =
        unsafe { methods::send_message(pool, worker as u32, data.as_ptr().cast(), data.len()) };
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
    if view(pool).started {
        fatal(E_WARNING, &message!("process pool is started"));
        call.set_bool(false);
        return;
    }
    // The reactor must be cleaned up before registering signals.
    swoole_rs_reactor::swoole_event_free();
    let signals = pool_signals();
    let mut previous = [0usize; 6];
    for (signo, old) in signals.iter().zip(previous.iter_mut()) {
        if *signo != 0 {
            // SAFETY: a valid signal and handler.
            *old = unsafe { swoole_signal_set(*signo, on_signal as *const () as usize, false) };
        }
    }
    if !protocol(pool, this.enable_message_bus) {
        call.set_bool(false);
        return;
    }
    let view = view(pool);
    if this.on_worker_start.is_null() && this.on_message.is_null() {
        if view.is_async {
            fatal(E_ERROR, &message!("require 'onWorkerStart' callback"));
            call.set_bool(false);
            return;
        } else if i64::from(view.ipc_mode) != IPC_NONE {
            fatal(E_ERROR, &message!("require 'onMessage' callback"));
            call.set_bool(false);
            return;
        }
    }
    if !this.on_worker_exit.is_null() && !this.enable_coroutine {
        // SAFETY: in a handler, with Swoole's exception class.
        unsafe {
            throw_exception(
                exception(),
                c"cannot set `onWorkerExit` without enable_coroutine",
                i64::from(INVALID_PARAMS),
            );
        }
        call.set_bool(false);
        return;
    }
    CURRENT.store(pool, Ordering::Release);
    let record = record(pool);
    if this.on_message.is_null() {
        record.main_loop = ptr::null();
    } else {
        record.on_message = on_message as *const c_void;
    }
    record.on_start = on_start as *const c_void;
    record.on_shutdown = on_shutdown as *const c_void;
    record.on_worker_start = on_worker_start as *const c_void;
    record.on_worker_stop = on_worker_stop as *const c_void;
    if this.enable_coroutine && !this.on_worker_exit.is_null() {
        record.on_worker_exit = on_worker_exit as *const c_void;
    }
    // SAFETY: a created pool; runs the master loop until the pool stops.
    if unsafe { methods::start(pool) } < 0 {
        call.set_bool(false);
        return;
    }
    // SAFETY: as above.
    unsafe { methods::wait(pool) };
    CURRENT.store(ptr::null_mut(), Ordering::Release);
    for (signo, old) in signals.iter().zip(previous) {
        if *signo != 0 {
            // SAFETY: restores the handler `start()` replaced.
            _ = unsafe { swoole_signal_set(*signo, old, false) };
        }
    }
}

/// The pool's protocol: messages over a bus whose packets are zend strings, or
/// a stream.
fn protocol(pool: Pool, message_bus: bool) -> bool {
    if !message_bus {
        // SAFETY: a created pool.
        unsafe { methods::set_protocol(pool, PROTOCOL_STREAM) };
        return true;
    }
    // SAFETY: as above.
    if record(pool).message_bus.is_null() && unsafe { methods::create_message_bus(pool) } != 0 {
        return false;
    }
    let bus = record(pool).message_bus.cast::<BusHead>();
    // SAFETY: the pool's message bus, whose allocator its next buffer uses.
    unsafe { (*bus).allocator = swoole_rs_phpswoole::sw_zend_string_allocator() };
    // SAFETY: a created pool.
    unsafe { methods::set_protocol(pool, PROTOCOL_MESSAGE) };
    true
}

fn detach(call: &Call) {
    let pool = CURRENT.load(Ordering::Acquire);
    if pool.is_null() {
        call.set_bool(false);
        return;
    }
    // SAFETY: the running pool.
    call.set_bool(unsafe { methods::detach(pool) });
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
    let view = view(pool);
    if id >= i64::from(view.worker_num) {
        notice(E_WARNING, &message!("invalid worker_id[{id}]"));
        call.set_bool(false);
        return;
    }
    let current_id = i64::from(methods::swoole_get_worker_id());
    if id < 0 {
        id = current_id;
    }
    let workers = workers_array(call);
    // SAFETY: a live array zval.
    let value = unsafe { (*workers).value };
    // SAFETY: an array zval holds its table.
    let table = unsafe { value.arr };
    let key = id.cast_unsigned();
    // SAFETY: a live table.
    let mut found = unsafe { zend_hash_index_find(table, key) };
    let process_ce = process_entry();
    // SAFETY: a live element, when found.
    if found.is_null() || unsafe { type_of(found) } == IS_NULL {
        let mut object = long_zval(0);
        // SAFETY: a writable zval and a registered class.
        unsafe { object_init_ex(&raw mut object, process_ce) };
        // A worker C++ allocates, as the Process object frees it.
        let worker = crate::worker::new();
        #[expect(
            clippy::cast_sign_loss,
            clippy::cast_possible_truncation,
            reason = "a checked worker id"
        )]
        // SAFETY: the running pool and its worker `id`; the Process object takes the copy.
        let pipe = unsafe { worker_record::copy_for_process(pool, id as usize, worker.cast()) };
        // SAFETY: the new object.
        let std = unsafe { object_of(&raw const object) };
        // SAFETY: as above.
        unsafe { update_property_long(process_ce, std, c"id", current_id) };
        // SAFETY: as above.
        unsafe { update_property_long(process_ce, std, c"pid", i64::from(worker_pid(worker))) };
        if i64::from(view.ipc_mode) == IPC_UNIXSOCK {
            // SAFETY: as above.
            unsafe { update_property_long(process_ce, std, c"pipe", i64::from(pipe)) };
        }
        // Upstream passes PIPE_TYPE_STREAM as enable_coroutine and async as the pipe type.
        set_worker_of(std, worker, true, c_int::from(view.is_async));
        // SAFETY: the array takes the new object's reference.
        unsafe {
            zend_hash_index_update(table, key, &raw mut object);
        }
        // SAFETY: as above.
        found = unsafe { zend_hash_index_find(table, key) };
    } else {
        // SAFETY: a Process object.
        let std = unsafe { object_of(found) };
        let worker = crate::process::worker_of(std);
        #[expect(
            clippy::cast_sign_loss,
            clippy::cast_possible_truncation,
            reason = "a checked worker id"
        )]
        // SAFETY: the running pool's worker `id`.
        let pid = unsafe { Worker::new(view.workers.wrapping_add(id as usize)).pid() };
        if worker_pid(worker) != pid {
            crate::worker::record(worker).set_pid(pid);
            // SAFETY: a Process object.
            unsafe { update_property_long(process_ce, std, c"pid", i64::from(pid)) };
        }
    }
    // SAFETY: the live element, which the return value shares (`ZVAL_COPY`).
    unsafe { call.set_copy(found) };
}

/// `sw_zend_read_and_convert_property_array(..., "workers")`.
fn workers_array(call: &Call) -> *mut zval {
    let mut slot = long_zval(0);
    // SAFETY: `$this`; the declared property's slot comes back.
    let property = unsafe {
        zend_read_property(
            POOL.entry(),
            call.this(),
            c"workers".as_ptr(),
            7,
            false,
            &raw mut slot,
        )
    };
    // SAFETY: a live property.
    if unsafe { type_of(property) } != IS_ARRAY {
        // SAFETY: the property's old value, replaced below.
        unsafe { zval_ptr_dtor(property) };
        // SAFETY: as above.
        unsafe { swr_zend_array_init(property) };
    }
    property
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
        // SAFETY: in a handler, with Swoole's exception class.
        unsafe {
            throw_exception(
                exception(),
                c"The process pool is not started",
                i64::from(INVALID_PARAMS),
            );
        }
        call.set_bool(false);
        return;
    }
    // SAFETY: the running pool.
    call.set_bool(unsafe { methods::shutdown(pool) });
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pool_object_holds_the_pool_callables_and_flags() {
        assert_eq!(
            Object_::OFFSET,
            7 * size_of::<*mut c_void>() + size_of::<*mut c_void>(),
            "seven pointers and two bools, padded to the zend_object"
        );
    }
}
