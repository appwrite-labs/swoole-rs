//! `Swoole\Process\Pool`: registration, objects, methods and the pool's event
//! callbacks. C++ keeps the `ProcessPool` (`src/os/process_pool.cc`) and reaches
//! it for Rust through the pool host table.
use crate::{
    host::host as process_host,
    process::{set_worker_of, worker_pid, worker_view},
    report::{INVALID_PARAMS, OPERATION_NOT_SUPPORT, fatal, notice, set_last_error},
    signal::swr_v1_phpprocess_clean,
};
use std::{
    ffi::{c_int, c_void},
    ptr,
    sync::atomic::{AtomicPtr, Ordering},
};
use swoole_rs_phpstubs::process_pool;
use swoole_rs_zend::{
    Call, Class, E_ERROR, E_WARNING, Object, class_name, handlers, long_zval, message, object_of,
    sys::{
        IS_ARRAY, IS_NULL, ZEND_ACC_NOT_SERIALIZABLE, ZEND_ACC_PUBLIC, object_init_ex,
        swr_zend_array_init, swr_zend_is_true, swr_zval_try_addref, zend_array, zend_class_entry,
        zend_declare_property_bool, zend_hash_index_find, zend_hash_index_update,
        zend_hash_str_find, zend_object, zend_object_handlers, zend_read_property,
        zend_update_property_bool, zval, zval_ptr_dtor,
    },
    throw_constructed_twice, throw_error, throw_exception, type_of, unset_property_deny,
    update_property_long,
};
use swoole_rs_zend::{method_table, sys as z};

const IPC_NONE: i64 = 0;
const IPC_UNIXSOCK: i64 = 1;
const IPC_MSGQUEUE: i64 = 2;
const IPC_SOCKET: i64 = 3;

/// `swr_phpprocess_pool_view`.
#[repr(C)]
#[derive(Clone, Copy)]
pub struct PoolView {
    pub started: bool,
    pub async_: bool,
    pub ipc_mode: u8,
    pub message_bus: bool,
    pub on_message: bool,
    pub worker_num: u32,
}

type Pool = *mut c_void;

/// `swr_v1_phpprocess_pool_events`.
#[repr(C)]
pub struct Events {
    pub start: Option<unsafe extern "C" fn(Pool)>,
    pub shutdown: Option<unsafe extern "C" fn(Pool)>,
    pub worker_start: Option<unsafe extern "C" fn(Pool, *mut c_void)>,
    pub worker_stop: Option<unsafe extern "C" fn(Pool, *mut c_void)>,
    pub worker_exit: Option<unsafe extern "C" fn(Pool, *mut c_void)>,
    pub message: Option<unsafe extern "C" fn(Pool, *mut c_void)>,
}

/// `swr_v1_phpprocess_pool_host`, field for field.
#[repr(C)]
pub struct PoolHost {
    pub exception: *mut zend_class_entry,
    pub process: *mut *mut zend_class_entry,
    pub server_exists: unsafe extern "C" fn() -> bool,
    pub worker_id: unsafe extern "C" fn() -> i64,
    pub pool_new: unsafe extern "C" fn(
        worker_num: u32,
        key: i32,
        ipc_type: c_int,
        object: *mut zval,
        async_: bool,
    ) -> Pool,
    pub pool_free: unsafe extern "C" fn(Pool),
    pub pool_view: unsafe extern "C" fn(Pool) -> PoolView,
    pub pool_object: unsafe extern "C" fn(Pool) -> *mut zval,
    pub pool_set_async: unsafe extern "C" fn(Pool, bool),
    pub pool_configure: unsafe extern "C" fn(Pool, *mut zend_array),
    pub pool_listen:
        unsafe extern "C" fn(Pool, host: *const u8, port: i64, backlog: i64, unix: bool) -> c_int,
    pub pool_response: unsafe extern "C" fn(Pool, data: *const u8, length: usize) -> c_int,
    pub pool_send: unsafe extern "C" fn(Pool, worker: i64, data: *const u8, length: usize) -> bool,
    pub signal_swap:
        unsafe extern "C" fn(signo: c_int, handler: Option<unsafe extern "C" fn(c_int)>) -> usize,
    pub pool_protocol: unsafe extern "C" fn(Pool, message_bus: bool) -> c_int,
    pub pool_run: unsafe extern "C" fn(Pool, events: *const Events) -> c_int,
    pub pool_detach: unsafe extern "C" fn(Pool) -> bool,
    pub pool_shutdown: unsafe extern "C" fn(Pool) -> bool,
    pub pool_signal: unsafe extern "C" fn(Pool, worker: *mut c_void, signo: c_int),
    pub worker_signals: unsafe extern "C" fn(term: bool, handler: unsafe extern "C" fn(c_int)),
    pub enter_worker: unsafe extern "C" fn(coroutine: bool),
    pub enter_master: unsafe extern "C" fn(),
    pub message_data: unsafe extern "C" fn(Pool, message: *mut c_void, target: *mut zval),
    pub worker_busy: unsafe extern "C" fn(busy: bool),
    pub pool_worker_pid: unsafe extern "C" fn(Pool, worker: i64) -> i32,
    pub worker_set_pid: unsafe extern "C" fn(worker: *mut c_void, pid: i32),
    pub worker_copy: unsafe extern "C" fn(Pool, worker: i64, pipe: *mut c_int) -> *mut c_void,
}

static HOST: AtomicPtr<PoolHost> = AtomicPtr::new(ptr::null_mut());
static POOL: Class = Class::new();
/// `current_pool` and `current_worker`.
static CURRENT: AtomicPtr<c_void> = AtomicPtr::new(ptr::null_mut());
static CURRENT_WORKER: AtomicPtr<c_void> = AtomicPtr::new(ptr::null_mut());

fn host() -> &'static PoolHost {
    // SAFETY: MINIT stores C++'s static table before any method or callback runs.
    unsafe { &*HOST.load(Ordering::Acquire) }
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
        // SAFETY: C++ logs the fatal error and exits.
        unsafe { (process_host().unconstructed)() };
    }
    object.pool
}

fn view(pool: Pool) -> PoolView {
    // SAFETY: a live pool.
    unsafe { (host().pool_view)(pool) }
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
        // SAFETY: the object owns its pool.
        unsafe { (host().pool_free)(pool.pool) };
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
            unsafe { (process_host().callable_free)(callable) };
        }
    }
    // SAFETY: as above.
    unsafe { Object_::dtor(object) };
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

/// MINIT: register `Swoole\Process\Pool` as `php_swoole_process_pool_minit` did.
/// # Safety
/// Called once, at MINIT, after the Process MINIT, with the static method
/// table and a host table that lives for the process.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn swr_v1_phpprocess_pool_minit(table: *const PoolHost) {
    HOST.store(table.cast_mut(), Ordering::Release);
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

/// `sw_process_pool()`: the pool `start()` is running, or null.
#[unsafe(no_mangle)]
pub extern "C" fn swr_v1_phpprocess_pool_current() -> *mut c_void {
    CURRENT.load(Ordering::Acquire)
}

fn configure(entry: &mut zend_class_entry, handlers: &mut zend_object_handlers) {
    entry.ce_flags |= ZEND_ACC_NOT_SERIALIZABLE;
    handlers.clone_obj = None;
    handlers.unset_property = Some(unset_property_deny);
    Object_::install(entry, handlers, create, free);
}

fn update_bool(object: *mut zend_object, name: &std::ffi::CStr, value: bool) {
    // SAFETY: a live Pool object; Zend copies the name.
    unsafe {
        zend_update_property_bool(
            POOL.entry(),
            object,
            name.as_ptr(),
            name.count_bytes(),
            i64::from(value),
        );
    }
}

fn update_long(object: *mut zend_object, name: &std::ffi::CStr, value: i64) {
    // SAFETY: a live Pool object.
    unsafe { update_property_long(POOL.entry(), object, name, value) };
}

fn getpid() -> i64 {
    // SAFETY: getpid has no preconditions.
    i64::from(unsafe { libc::getpid() })
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
    let called =
        unsafe { (process_host().callable_call)(callable, argc, args.as_mut_ptr(), coroutine) };
    if !called {
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
    // SAFETY: the running pool and this process's worker (or null).
    unsafe { (host().pool_signal)(pool, CURRENT_WORKER.load(Ordering::Acquire), signo) };
}

/// The object a pool callback belongs to.
fn owner(pool: Pool) -> (*mut zval, *mut zend_object, &'static mut PoolObject) {
    // SAFETY: the pool's object zval, alive while the pool runs.
    let object = unsafe { (host().pool_object)(pool) };
    // SAFETY: as above.
    let std = unsafe { object_of(object) };
    // SAFETY: a Pool object.
    (object, std, unsafe { data(std) })
}

unsafe extern "C" fn on_worker_start(pool: Pool, worker: *mut c_void) {
    let (object, std, this) = owner(pool);
    swr_v1_phpprocess_clean();
    CURRENT.store(pool, Ordering::Release);
    CURRENT_WORKER.store(worker, Ordering::Release);
    let id = i64::from(worker_view(worker).id);
    update_bool(std, c"running", true);
    update_bool(std, c"workerRunning", true);
    update_long(std, c"workerPid", getpid());
    update_long(std, c"workerId", id);
    // SAFETY: C++ sets the worker type and coroutine flag.
    unsafe { (host().enter_worker)(this.enable_coroutine) };
    if !this.on_worker_start.is_null() {
        dispatch(
            this.on_worker_start,
            object,
            Some(id),
            this.enable_coroutine,
            "onWorkerStart",
        );
    }
    let term = !this.on_message.is_null() || this.enable_coroutine;
    // SAFETY: C++ installs the handler for unset signals.
    unsafe { (host().worker_signals)(term, on_signal) };
    swoole_rs_zend::bailout::finish();
}

unsafe extern "C" fn on_message(pool: Pool, message: *mut c_void) {
    let (object, _, this) = owner(pool);
    // SAFETY: a live object zval, copied without a reference as upstream does.
    let mut args = [unsafe { *object }, long_zval(0)];
    // SAFETY: the received message; C++ fills the string argument.
    unsafe { (host().message_data)(pool, message, &raw mut args[1]) };
    let host = host();
    // SAFETY: this worker's status.
    unsafe { (host.worker_busy)(true) };
    // SAFETY: the onMessage callable `start()` required, with two arguments.
    let called = unsafe {
        (process_host().callable_call)(this.on_message, 2, args.as_mut_ptr(), this.enable_coroutine)
    };
    if !called {
        // SAFETY: a live object zval.
        let std = unsafe { object_of(object) };
        // SAFETY: a live object.
        let name = unsafe { class_name(std) };
        notice(
            E_WARNING,
            &message!("{}->onMessage handler error", name.to_string_lossy()),
        );
    }
    // SAFETY: as above.
    unsafe { (host.worker_busy)(false) };
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
    // SAFETY: C++ sets the worker type and coroutine flag.
    unsafe { (host().enter_master)() };
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
    if !unsafe { (process_host().cli)() } {
        return refuse(format!("{name} can only be used in PHP CLI mode"));
    }
    let host = host();
    // SAFETY: C++'s reader of the server global.
    if unsafe { (host.server_exists)() } {
        return refuse("cannot create server and process pool instances simultaneously".into());
    }
    if !CURRENT.load(Ordering::Acquire).is_null() {
        return refuse(
            "A process pool instance has already been created and cannot be created again".into(),
        );
    }
    // SAFETY: C++'s reader of the thread's identity.
    if !unsafe { (process_host().main_thread)() } {
        set_last_error(OPERATION_NOT_SUPPORT);
        // SAFETY: in a handler, with Swoole's exception class.
        unsafe {
            throw_exception(
                host.exception,
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
        unsafe { throw_exception(host.exception, text, i64::from(crate::report::errno())) };
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
    // SAFETY: C++ makes the pool and keeps a copy of `$this`'s zval.
    let made = unsafe {
        (host.pool_new)(
            worker_num as u32,
            key as i32,
            ipc_type as c_int,
            call.this_zval(),
            coroutine,
        )
    };
    if made.is_null() {
        return fail(c"failed to create process pool");
    }
    pool.enable_coroutine = coroutine;
    pool.pool = made;
}

fn destruct(_call: &Call) {}

/// `php_swoole_array_get_value`: the element, unless absent or null.
fn option(options: *mut zend_array, key: &std::ffi::CStr) -> Option<*mut zval> {
    // SAFETY: a live array and a NUL-terminated key.
    let value = unsafe { zend_hash_str_find(options, key.as_ptr(), key.count_bytes()) };
    // SAFETY: a live element.
    (!value.is_null() && unsafe { type_of(value) } != IS_NULL).then_some(value)
}

fn set(call: &Call) {
    let Ok(Some(options)) = call.arity(1, 1).and_then(|()| call.array_table(1)) else {
        call.set_bool(false);
        return;
    };
    let this = this(call);
    let pool = checked(this);
    let host = host();
    // SAFETY: a live pool and the options array.
    unsafe { (host.pool_configure)(pool, options) };
    if let Some(value) = option(options, c"enable_coroutine") {
        // SAFETY: a live element.
        this.enable_coroutine = unsafe { swr_zend_is_true(value) };
        // SAFETY: a live pool.
        unsafe { (host.pool_set_async)(pool, this.enable_coroutine) };
    }
    if let Some(value) = option(options, c"enable_message_bus") {
        // SAFETY: a live element.
        this.enable_message_bus = unsafe { swr_zend_is_true(value) };
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
                        host().exception,
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
    let process = process_host();
    if !slot.is_null() {
        // SAFETY: the callable this slot owned.
        unsafe { (process.callable_free)(*slot) };
    }
    // SAFETY: a passed argument.
    *slot = unsafe { (process.callable_create)(callback) };
    call.set_bool(true);
}

fn unsupported(call: &Call, view: PoolView) {
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
    let unix = address.len() >= 6 && address[..6].eq_ignore_ascii_case(b"unix:/");
    // The argument string is NUL-terminated in place; a unix path skips "unix:".
    let host_ptr = address.as_ptr().wrapping_add(if unix { 5 } else { 0 });
    // SAFETY: a live pool and a NUL-terminated address.
    let listened = unsafe { (host().pool_listen)(pool, host_ptr, port, backlog, unix) };
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
    // SAFETY: a live pool and the argument's bytes.
    call.set_bool(unsafe { (host().pool_response)(pool, data.as_ptr(), data.len()) } >= 0);
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
    // SAFETY: a live pool and the argument's bytes.
    call.set_bool(unsafe { (host().pool_send)(pool, worker, data.as_ptr(), data.len()) });
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
    let host = host();
    // The reactor must be cleaned up before registering signals.
    // SAFETY: C++'s event loop teardown.
    unsafe { (process_host().event_free)() };
    let signals = pool_signals();
    let mut previous = [0usize; 6];
    for (signo, old) in signals.iter().zip(previous.iter_mut()) {
        if *signo != 0 {
            // SAFETY: a valid signal and handler.
            *old = unsafe { (host.signal_swap)(*signo, Some(on_signal)) };
        }
    }
    // SAFETY: a live pool.
    if unsafe { (host.pool_protocol)(pool, this.enable_message_bus) } != 0 {
        call.set_bool(false);
        return;
    }
    let view = view(pool);
    if this.on_worker_start.is_null() && this.on_message.is_null() {
        if view.async_ {
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
                host.exception,
                c"cannot set `onWorkerExit` without enable_coroutine",
                i64::from(INVALID_PARAMS),
            );
        }
        call.set_bool(false);
        return;
    }
    CURRENT.store(pool, Ordering::Release);
    let events = Events {
        start: Some(on_start),
        shutdown: Some(on_shutdown),
        worker_start: Some(on_worker_start),
        worker_stop: Some(on_worker_stop),
        worker_exit: (this.enable_coroutine && !this.on_worker_exit.is_null())
            .then_some(on_worker_exit as unsafe extern "C" fn(Pool, *mut c_void)),
        message: (!this.on_message.is_null())
            .then_some(on_message as unsafe extern "C" fn(Pool, *mut c_void)),
    };
    // SAFETY: a live pool; runs the master loop until the pool stops.
    if unsafe { (host.pool_run)(pool, &raw const events) } < 0 {
        call.set_bool(false);
        return;
    }
    CURRENT.store(ptr::null_mut(), Ordering::Release);
    for (signo, old) in signals.iter().zip(previous) {
        if *signo != 0 {
            // SAFETY: restores the handler `start()` replaced.
            let handler: Option<unsafe extern "C" fn(c_int)> =
                // SAFETY: a handler address swoole_signal_set returned, or 0.
                unsafe { std::mem::transmute::<usize, Option<unsafe extern "C" fn(c_int)>>(old) };
            // SAFETY: as above.
            unsafe { (host.signal_swap)(*signo, handler) };
        }
    }
}

fn detach(call: &Call) {
    let pool = CURRENT.load(Ordering::Acquire);
    if pool.is_null() {
        call.set_bool(false);
        return;
    }
    // SAFETY: the running pool.
    call.set_bool(unsafe { (host().pool_detach)(pool) });
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
    let host = host();
    let view = view(pool);
    if id >= i64::from(view.worker_num) {
        notice(E_WARNING, &message!("invalid worker_id[{id}]"));
        call.set_bool(false);
        return;
    }
    // SAFETY: C++'s reader of this process's worker id.
    let current_id = unsafe { (host.worker_id)() };
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
    // SAFETY: C++'s static class entry, filled at MINIT.
    let process_ce = unsafe { *host.process };
    // SAFETY: a live element, when found.
    if found.is_null() || unsafe { type_of(found) } == IS_NULL {
        let mut object = long_zval(0);
        // SAFETY: a writable zval and a registered class.
        unsafe { object_init_ex(&raw mut object, process_ce) };
        let mut pipe = -1;
        // SAFETY: the running pool; the Process object takes the copy.
        let worker = unsafe { (host.worker_copy)(pool, id, &raw mut pipe) };
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
        set_worker_of(std, worker, true, c_int::from(view.async_));
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
        // SAFETY: the running pool.
        let pid = unsafe { (host.pool_worker_pid)(pool, id) };
        if worker_pid(worker) != pid {
            // SAFETY: the object's worker copy.
            unsafe { (host.worker_set_pid)(worker, pid) };
            // SAFETY: a Process object.
            unsafe { update_property_long(process_ce, std, c"pid", i64::from(pid)) };
        }
    }
    // SAFETY: the element, which the return value shares.
    let value = unsafe { *found };
    call.set_value(value);
    // SAFETY: the return slot now holds the shared object.
    unsafe { swr_zval_try_addref(call.return_value()) };
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
        // SAFETY: SIGTERM's action: stop running and stop this worker.
        unsafe {
            (host().pool_signal)(pool, CURRENT_WORKER.load(Ordering::Acquire), libc::SIGTERM);
        };
    }
}

fn shutdown(call: &Call) {
    let pool = CURRENT.load(Ordering::Acquire);
    if pool.is_null() {
        // SAFETY: in a handler, with Swoole's exception class.
        unsafe {
            throw_exception(
                host().exception,
                c"The process pool is not started",
                i64::from(INVALID_PARAMS),
            );
        }
        call.set_bool(false);
        return;
    }
    // SAFETY: the running pool.
    call.set_bool(unsafe { (host().pool_shutdown)(pool) });
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
