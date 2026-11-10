//! `Swoole\Process`: registration, objects and methods. Each method mirrors
//! upstream's ZPP block and checks, then works on the object's `Worker` through
//! the host table.
use crate::{
    host,
    report::{INVALID_PARAMS, cli, errno, fatal, notice, set_errno, set_last_error, sys},
    signal::signal,
    worker::{self, WorkerView},
};
use std::{
    ffi::{CStr, c_char, c_int, c_long, c_void},
    ptr,
    sync::atomic::{AtomicU32, Ordering},
};
use swoole_rs_ossupport::{QueueRecord, swr_v1_os_msg_queue_pop, swr_v1_os_msg_queue_push};
use swoole_rs_phpstubs::process;
use swoole_rs_zend::{
    Call, Class, E_ERROR, E_WARNING, Object, add_assoc_long, class_name, declare_property_null,
    each_value, handlers, message, object_of, register_long_constant, set_object,
    sys::{
        _zend_bailout, IS_NULL, ZEND_ACC_NOT_SERIALIZABLE, ZEND_ACC_PRIVATE, swr_zend_emalloc,
        swr_zend_is_true, swr_zend_object_addref, swr_zend_object_release, swr_zend_string_alloc,
        swr_zend_string_free, zend_class_entry, zend_get_executed_filename, zend_hash_str_find,
        zend_object, zend_object_handlers, zend_read_property, zend_string, zval,
    },
    throw_constructed_twice, throw_error, throw_exception, type_of, unset_property_deny,
    update_property, update_property_long,
};
use swoole_rs_zend::{method_table, sys as z};

/// `Swoole\Process::IPC_NOWAIT` (`MSGQUEUE_NOWAIT`).
const IPC_NOWAIT: i64 = 1 << 8;
const MSGQUEUE_ORIENT: i64 = 1;
const MSGQUEUE_BALANCE: i64 = 2;
const PIPE_TYPE_STREAM: i64 = 1;
const PIPE_TYPE_DGRAM: i64 = 2;
const PIPE_CLOSE_READ: i64 = 3;
const PIPE_CLOSE_WRITE: i64 = 4;
/// `SW_MSGMAX`.
const MSGMAX: usize = 65536;
const MSGMAX_LONG: i64 = 65536;

static PROCESS: Class = Class::new();

/// `round_process_id`: the id of the next process made outside a server worker.
static ROUND_ID: AtomicU32 = AtomicU32::new(0);

/// `struct ProcessObject { Worker *worker; zend_object *zsocket; PipeType
/// pipe_type; bool enable_coroutine; bool blocking; zend_object std; }`.
#[repr(C)]
struct Process {
    worker: *mut c_void,
    socket: *mut zend_object,
    pipe_type: c_int,
    enable_coroutine: bool,
    blocking: bool,
}

type ProcessObject = Object<Process>;

/// A `SysV` message, `struct { long type; char data[SW_MSGMAX]; }`, on the heap.
struct QueueMessage(Vec<c_long>);

impl QueueMessage {
    fn new(kind: c_long) -> Self {
        let mut words = vec![0; MSGMAX / size_of::<c_long>()];
        words.insert(0, kind);
        Self(words)
    }

    fn as_ptr(&mut self) -> *mut c_void {
        self.0.as_mut_ptr().cast()
    }

    fn data(&mut self) -> &mut [u8] {
        let data = self.0.as_mut_ptr().wrapping_add(1).cast::<u8>();
        // SAFETY: the words after the type hold MSGMAX bytes.
        unsafe { std::slice::from_raw_parts_mut(data, MSGMAX) }
    }
}

pub(crate) fn entry() -> *mut zend_class_entry {
    PROCESS.entry()
}

/// The data of a Process object.
/// # Safety
/// `object` is a live Process (or subclass) object.
unsafe fn data<'a>(object: *mut zend_object) -> &'a mut Process {
    // SAFETY: per the caller.
    unsafe { ProcessObject::data(object) }
}

fn this<'a>(call: &Call) -> &'a mut Process {
    // SAFETY: Process methods are non-static, so `$this` is a Process object.
    unsafe { data(call.this()) }
}

/// `php_swoole_process_get_and_check_worker`: fatal without a constructed worker.
fn checked(process: &Process) -> *mut c_void {
    if process.worker.is_null() {
        host::unconstructed();
    }
    process.worker
}

/// The worker of a Process object (null before its constructor ran).
pub(crate) fn worker_of(object: *mut zend_object) -> *mut c_void {
    // SAFETY: callers pass a live Process object.
    unsafe { data(object) }.worker
}

/// `php_swoole_process_set_worker` on a Process object.
pub(crate) fn set_worker_of(
    object: *mut zend_object,
    worker: *mut c_void,
    enable_coroutine: bool,
    pipe_type: c_int,
) {
    // SAFETY: callers pass a live Process object.
    set_worker(unsafe { data(object) }, worker, enable_coroutine, pipe_type);
}

/// A live worker's pid.
pub(crate) fn worker_pid(worker: *mut c_void) -> i32 {
    view(worker).pid
}

pub(crate) fn worker_view(worker: *mut c_void) -> WorkerView {
    view(worker)
}

/// A live worker's queue, or null.
fn queue_of(worker: *mut c_void) -> *mut QueueRecord {
    worker::record(worker).queue().cast()
}

fn view(worker: *mut c_void) -> WorkerView {
    worker::view(worker)
}

unsafe extern "C" fn create(ce: *mut zend_class_entry) -> *mut zend_object {
    // SAFETY: Zend calls `create_object` with this class or a subclass; zeroed
    // data is a Process without a worker.
    unsafe { ProcessObject::create(&PROCESS, ce) }
}

unsafe extern "C" fn free(object: *mut zend_object) {
    // SAFETY: Zend frees an object this class created.
    let process = unsafe { data(object) };
    if !process.worker.is_null() {
        let queue = queue_of(process.worker);
        if !queue.is_null() {
            // SAFETY: the object owns its worker and the worker its boxed queue.
            drop(unsafe { Box::from_raw(queue) });
        }
        // SAFETY: as above.
        unsafe { worker::free(process.worker) };
    }
    if !process.socket.is_null() {
        // SAFETY: the object holds one reference to its exported socket.
        unsafe { swr_zend_object_release(process.socket) };
    }
    // SAFETY: as above.
    unsafe { ProcessObject::dtor(object) };
}

// `swoole_set_process_name()` (php_swoole.cc), which `Process::name()` aliases.
unsafe extern "C" {
    fn zif_swoole_set_process_name(frame: *mut z::zend_execute_data, value: *mut z::zval);
}

method_table! {
    static METHODS = [
        c"__construct" => zim_swoole_process___construct, &process::arginfo_class_Swoole_Process___construct, z::ZEND_ACC_PUBLIC;
        c"__destruct" => zim_swoole_process___destruct, &process::arginfo_class_Swoole_Process___destruct, z::ZEND_ACC_PUBLIC;
        c"wait" => zim_swoole_process_wait, &process::arginfo_class_Swoole_Process_wait, z::ZEND_ACC_PUBLIC | z::ZEND_ACC_STATIC;
        c"signal" => zim_swoole_process_signal, &process::arginfo_class_Swoole_Process_signal, z::ZEND_ACC_PUBLIC | z::ZEND_ACC_STATIC;
        c"alarm" => zim_swoole_process_alarm, &process::arginfo_class_Swoole_Process_alarm, z::ZEND_ACC_PUBLIC | z::ZEND_ACC_STATIC;
        c"kill" => zim_swoole_process_kill, &process::arginfo_class_Swoole_Process_kill, z::ZEND_ACC_PUBLIC | z::ZEND_ACC_STATIC;
        c"daemon" => zim_swoole_process_daemon, &process::arginfo_class_Swoole_Process_daemon, z::ZEND_ACC_PUBLIC | z::ZEND_ACC_STATIC;
        #[cfg(any(target_os = "linux", target_os = "android"))]
        c"setAffinity" => crate::affinity::zim_swoole_process_setAffinity, &process::arginfo_class_Swoole_Process_setAffinity, z::ZEND_ACC_PUBLIC | z::ZEND_ACC_STATIC;
        #[cfg(any(target_os = "linux", target_os = "android"))]
        c"getAffinity" => crate::affinity::zim_swoole_process_getAffinity, &process::arginfo_class_Swoole_Process_getAffinity, z::ZEND_ACC_PUBLIC | z::ZEND_ACC_STATIC;
        c"setPriority" => zim_swoole_process_setPriority, &process::arginfo_class_Swoole_Process_setPriority, z::ZEND_ACC_PUBLIC;
        c"getPriority" => zim_swoole_process_getPriority, &process::arginfo_class_Swoole_Process_getPriority, z::ZEND_ACC_PUBLIC;
        c"set" => zim_swoole_process_set, &process::arginfo_class_Swoole_Process_set, z::ZEND_ACC_PUBLIC;
        c"setTimeout" => zim_swoole_process_setTimeout, &process::arginfo_class_Swoole_Process_setTimeout, z::ZEND_ACC_PUBLIC;
        c"setBlocking" => zim_swoole_process_setBlocking, &process::arginfo_class_Swoole_Process_setBlocking, z::ZEND_ACC_PUBLIC;
        c"useQueue" => zim_swoole_process_useQueue, &process::arginfo_class_Swoole_Process_useQueue, z::ZEND_ACC_PUBLIC;
        c"statQueue" => zim_swoole_process_statQueue, &process::arginfo_class_Swoole_Process_statQueue, z::ZEND_ACC_PUBLIC;
        c"freeQueue" => zim_swoole_process_freeQueue, &process::arginfo_class_Swoole_Process_freeQueue, z::ZEND_ACC_PUBLIC;
        c"start" => zim_swoole_process_start, &process::arginfo_class_Swoole_Process_start, z::ZEND_ACC_PUBLIC;
        c"write" => zim_swoole_process_write, &process::arginfo_class_Swoole_Process_write, z::ZEND_ACC_PUBLIC;
        c"close" => zim_swoole_process_close, &process::arginfo_class_Swoole_Process_close, z::ZEND_ACC_PUBLIC;
        c"read" => zim_swoole_process_read, &process::arginfo_class_Swoole_Process_read, z::ZEND_ACC_PUBLIC;
        c"push" => zim_swoole_process_push, &process::arginfo_class_Swoole_Process_push, z::ZEND_ACC_PUBLIC;
        c"pop" => zim_swoole_process_pop, &process::arginfo_class_Swoole_Process_pop, z::ZEND_ACC_PUBLIC;
        c"exit" => zim_swoole_process_exit, &process::arginfo_class_Swoole_Process_exit, z::ZEND_ACC_PUBLIC;
        c"exec" => zim_swoole_process_exec, &process::arginfo_class_Swoole_Process_exec, z::ZEND_ACC_PUBLIC;
        c"exportSocket" => zim_swoole_process_exportSocket, &process::arginfo_class_Swoole_Process_exportSocket, z::ZEND_ACC_PUBLIC;
        c"name" => zif_swoole_set_process_name, &process::arginfo_class_Swoole_Process_name, 0;
    ];
}

/// `swoole_process_ce`, which crates/phpevents and the Server read.
#[unsafe(no_mangle)]
pub static mut swoole_process_ce: *mut zend_class_entry = ptr::null_mut();

/// `php_swoole_process_minit`: register `Swoole\Process`, its constants and
/// properties, and (when pcntl is absent) the signal constants.
/// # Safety
/// Called once, at MINIT.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn php_swoole_process_minit(module: c_int) {
    // SAFETY: MINIT, with a static table.
    unsafe { PROCESS.register(c"Swoole\\Process", METHODS.as_ptr(), configure) };
    for (name, value) in [
        (c"IPC_NOWAIT", IPC_NOWAIT),
        (c"PIPE_MASTER", 1),
        (c"PIPE_WORKER", 2),
        (c"PIPE_READ", PIPE_CLOSE_READ),
        (c"PIPE_WRITE", PIPE_CLOSE_WRITE),
        (c"PIPE_TYPE_NONE", 0),
        (c"PIPE_TYPE_STREAM", PIPE_TYPE_STREAM),
        (c"PIPE_TYPE_DGRAM", PIPE_TYPE_DGRAM),
    ] {
        // SAFETY: MINIT, after `register`.
        unsafe { PROCESS.constant_long(name, value) };
    }
    for name in [c"pipe", c"msgQueueId", c"msgQueueKey", c"pid", c"id"] {
        // SAFETY: as above.
        unsafe { PROCESS.property_null(name) };
    }
    // SAFETY: as above.
    unsafe { declare_property_null(entry(), c"callback", ZEND_ACC_PRIVATE) };
    if !swoole_rs_zend::module_loaded(c"pcntl") {
        for (name, value) in signal_constants() {
            // SAFETY: MINIT of `module`.
            unsafe { register_long_constant(name, value, module) };
        }
    }
    // SAFETY: as above.
    unsafe { register_long_constant(c"SWOOLE_MSGQUEUE_ORIENT", MSGQUEUE_ORIENT, module) };
    // SAFETY: as above.
    unsafe { register_long_constant(c"SWOOLE_MSGQUEUE_BALANCE", MSGQUEUE_BALANCE, module) };
    // SAFETY: MINIT, before any reader.
    unsafe { swoole_process_ce = entry() };
}

/// `SW_SET_CLASS_NOT_SERIALIZABLE`, clone and unset denied, custom object.
fn configure(entry: &mut zend_class_entry, handlers: &mut zend_object_handlers) {
    entry.ce_flags |= ZEND_ACC_NOT_SERIALIZABLE;
    handlers.clone_obj = None;
    handlers.unset_property = Some(unset_property_deny);
    ProcessObject::install(entry, handlers, create, free);
}

/// The 31 signal constants and friends upstream registers without pcntl, in its order.
fn signal_constants() -> Vec<(&'static CStr, i64)> {
    let mut constants = vec![
        (c"SIGHUP", libc::SIGHUP),
        (c"SIGINT", libc::SIGINT),
        (c"SIGQUIT", libc::SIGQUIT),
        (c"SIGILL", libc::SIGILL),
        (c"SIGTRAP", libc::SIGTRAP),
        (c"SIGABRT", libc::SIGABRT),
        (c"SIGBUS", libc::SIGBUS),
        (c"SIGFPE", libc::SIGFPE),
        (c"SIGKILL", libc::SIGKILL),
        (c"SIGUSR1", libc::SIGUSR1),
        (c"SIGSEGV", libc::SIGSEGV),
        (c"SIGUSR2", libc::SIGUSR2),
        (c"SIGPIPE", libc::SIGPIPE),
        (c"SIGALRM", libc::SIGALRM),
        (c"SIGTERM", libc::SIGTERM),
    ];
    #[cfg(any(target_os = "linux", target_os = "android"))]
    constants.push((c"SIGSTKFLT", libc::SIGSTKFLT));
    constants.extend([
        (c"SIGCHLD", libc::SIGCHLD),
        (c"SIGCONT", libc::SIGCONT),
        (c"SIGSTOP", libc::SIGSTOP),
        (c"SIGTSTP", libc::SIGTSTP),
        (c"SIGTTIN", libc::SIGTTIN),
        (c"SIGTTOU", libc::SIGTTOU),
        (c"SIGURG", libc::SIGURG),
        (c"SIGXCPU", libc::SIGXCPU),
        (c"SIGXFSZ", libc::SIGXFSZ),
        (c"SIGVTALRM", libc::SIGVTALRM),
        (c"SIGPROF", libc::SIGPROF),
        (c"SIGWINCH", libc::SIGWINCH),
        (c"SIGIO", libc::SIGIO),
    ]);
    #[cfg(any(target_os = "linux", target_os = "android"))]
    constants.push((c"SIGPWR", libc::SIGPWR));
    constants.push((c"SIGSYS", libc::SIGSYS));
    let mut all: Vec<(&'static CStr, i64)> = constants
        .into_iter()
        .map(|(name, value)| (name, i64::from(value)))
        .collect();
    all.extend([
        (c"SIG_IGN", crate::signal::SIG_IGN),
        (c"PRIO_PROCESS", i64::from(libc::PRIO_PROCESS)),
        (c"PRIO_PGRP", i64::from(libc::PRIO_PGRP)),
        (c"PRIO_USER", i64::from(libc::PRIO_USER)),
    ]);
    all
}

/// `php_swoole_process_get_worker` (`check`: `..._get_and_check_worker`).
/// # Safety
/// `object` is a live Process object zval.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn swr_v1_phpprocess_worker(object: *const zval, check: bool) -> *mut c_void {
    // SAFETY: per the caller.
    let object = unsafe { object_of(object) };
    // SAFETY: as above.
    let process = unsafe { data(object) };
    if check {
        checked(process)
    } else {
        process.worker
    }
}

fn set_worker(
    process: &mut Process,
    worker: *mut c_void,
    enable_coroutine: bool,
    pipe_type: c_int,
) {
    process.worker = worker;
    process.pipe_type = pipe_type;
    process.enable_coroutine = enable_coroutine;
    process.blocking = true;
}

/// `php_swoole_process_start`: in the child, run the callback, then bail out
/// as `exit` would. -1 when the callback or the event loop cannot start.
/// # Safety
/// `worker` is the live worker of the Process object `object`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn swr_v1_phpprocess_start(worker: *mut c_void, object: *mut zval) -> c_int {
    // SAFETY: per the caller.
    let std = unsafe { object_of(object) };
    let mut slot = swoole_rs_zend::long_zval(0);
    // SAFETY: a Process object; the declared private property's slot comes back.
    let callback =
        unsafe { zend_read_property(entry(), std, c"callback".as_ptr(), 8, true, &raw mut slot) };
    // SAFETY: a live zval.
    let callable = unsafe { host::callable_create(callback) };
    if callable.is_null() {
        return -1;
    }
    worker::child_enter(worker);
    let worker = view(worker);
    for (redirect, target) in [
        (worker.redirect_stdin, libc::STDIN_FILENO),
        (worker.redirect_stdout, libc::STDOUT_FILENO),
        (worker.redirect_stderr, libc::STDERR_FILENO),
    ] {
        // SAFETY: dup2 has no memory preconditions.
        if redirect && unsafe { libc::dup2(worker.pipe, target) } < 0 {
            sys(E_WARNING, &message!("dup2() failed"));
        }
    }
    crate::signal::clean();
    // SAFETY: a Process object.
    unsafe { update_property_long(entry(), std, c"pid", i64::from(worker.pid)) };
    if worker.pipe >= 0 {
        // SAFETY: as above.
        unsafe { update_property_long(entry(), std, c"pipe", i64::from(worker.pipe)) };
    }
    // SAFETY: as above.
    let coroutine = unsafe { data(std) }.enable_coroutine;
    if coroutine && host::reactor_init() < 0 {
        // SAFETY: the callable made above.
        unsafe { host::callable_free(callable) };
        return -1;
    }
    // SAFETY: the callable, with `$this` as its one argument.
    if !unsafe { host::callable_call(callable, 1, object, coroutine) } {
        // SAFETY: a live object.
        let name = unsafe { class_name(std) };
        notice(
            E_WARNING,
            &message!("{}->onStart handler error", name.to_string_lossy()),
        );
    }
    if coroutine {
        host::event_wait();
    }
    // SAFETY: the callable made above.
    unsafe { host::callable_free(callable) };
    swoole_rs_zend::bailout::finish();
    // SAFETY: equivalent to exit: Zend unwinds to its bailout point; this frame
    // owns nothing that needs a drop.
    unsafe { _zend_bailout(c"crates/phpprocess/src/process.rs".as_ptr(), line!()) }
}

fn construct(call: &Call) {
    let object = call.this();
    let process = this(call);
    // SAFETY: a live object.
    let name = unsafe { class_name(object) }.to_string_lossy();
    let refuse = |text| {
        // SAFETY: in a handler.
        unsafe { throw_error(message!("{text}").as_cstr()) };
        call.set_bool(false);
    };
    if !process.worker.is_null() {
        // SAFETY: in a handler, with `$this`.
        unsafe { throw_constructed_twice(object) };
        call.set_bool(false);
        return;
    }
    // SAFETY: C++'s reader of an ini flag.
    if !cli() {
        return refuse(format!("{name} can only be used in PHP CLI mode"));
    }
    match host::construct_guard() {
        1 => return refuse(format!("{name} can't be used in master process")),
        2 => return refuse(format!("unable to create {name} with async-io threads")),
        _ => {}
    }
    drop(name);
    // Upstream makes the worker before parsing, so a failed parse leaves it set.
    // A new worker the object owns from here.
    let worker = worker::new();
    process.worker = worker;
    let parsed = call.arity(1, 4).and_then(|()| {
        call.func(1)?;
        Ok((
            call.bool(2, false)?,
            call.long(3, PIPE_TYPE_DGRAM)?,
            call.bool(4, false)?,
        ))
    });
    let Ok((redirect, mut pipe_type, enable_coroutine)) = parsed else {
        call.set_bool(false);
        return;
    };
    // SAFETY: the worker made above.
    if !unsafe { host::worker_share(worker) } {
        if redirect {
            worker::redirect(worker);
            pipe_type = PIPE_TYPE_STREAM;
        }
        let base = host::worker_base();
        if ROUND_ID.load(Ordering::Relaxed) == 0 {
            ROUND_ID.store(base, Ordering::Relaxed);
        }
        let id = ROUND_ID.fetch_add(1, Ordering::Relaxed);
        worker::set_id(worker, id);
        if pipe_type > 0 {
            let socket_type = if pipe_type == PIPE_TYPE_STREAM {
                libc::SOCK_STREAM
            } else {
                libc::SOCK_DGRAM
            };
            let fd = worker::open_pipe(worker, socket_type);
            if fd < 0 {
                // SAFETY: in a handler, with Swoole's exception class.
                unsafe {
                    throw_exception(
                        host::exception(),
                        c"failed to create unix soccket",
                        i64::from(errno()),
                    );
                }
                call.set_bool(false);
                return;
            }
            call.update_property_long(Some(&PROCESS), c"pipe", i64::from(fd));
        }
    }
    call.update_property_long(Some(&PROCESS), c"id", i64::from(view(worker).id));
    // SAFETY: `$this`, and the callback argument.
    unsafe { update_property(entry(), object, c"callback", call.zval(1)) };
    #[expect(
        clippy::cast_possible_truncation,
        reason = "upstream casts to PipeType"
    )]
    set_worker(process, worker, enable_coroutine, pipe_type as c_int);
}

fn destruct(_call: &Call) {}

fn wait(call: &Call) {
    let Ok(blocking) = call.arity(0, 1).and_then(|()| call.bool(1, true)) else {
        call.set_bool(false);
        return;
    };
    let mut status = 0;
    let options = if blocking { 0 } else { libc::WNOHANG };
    // SAFETY: `status` is a writable local.
    let pid = unsafe { libc::waitpid(-1, &raw mut status, options) };
    if pid <= 0 {
        call.set_bool(false);
        return;
    }
    call.set_array();
    call.add_assoc_long(c"pid", i64::from(pid));
    call.add_assoc_long(c"code", i64::from(libc::WEXITSTATUS(status)));
    call.add_assoc_long(c"signal", i64::from(libc::WTERMSIG(status)));
}

/// `php_swoole_msgqueue_key_is_valid` for a signed 32-bit `key_t`.
pub(crate) fn key_is_valid(key: i64) -> bool {
    if key < 0 {
        key >= i64::from(i32::MIN)
    } else {
        key <= i64::from(u32::MAX)
    }
}

fn use_queue(call: &Call) {
    let parsed = call.arity(0, 3).and_then(|()| {
        Ok((
            call.long(1, 0)?,
            call.long(2, MSGQUEUE_BALANCE)?,
            call.long(3, -1)?,
        ))
    });
    let Ok((mut key, mut mode, capacity)) = parsed else {
        call.set_bool(false);
        return;
    };
    let worker = checked(this(call));
    if key > 0 && !key_is_valid(key) {
        fatal(
            E_WARNING,
            &message!("message queue key is out of range [{key}]"),
        );
        call.set_bool(false);
        return;
    }
    if key <= 0 {
        // SAFETY: Zend's current file name, NUL-terminated, or null.
        let file = unsafe { zend_get_executed_filename() };
        // SAFETY: ftok reads the path.
        key = i64::from(unsafe { libc::ftok(file, 1) });
    }
    let blocking = mode & IPC_NOWAIT == 0;
    if !blocking {
        mode &= !IPC_NOWAIT;
    }
    #[expect(
        clippy::cast_possible_truncation,
        reason = "upstream converts to key_t"
    )]
    let mut queue = QueueRecord::new(key as i32, true, 0);
    if !queue.ready() {
        call.set_bool(false);
        return;
    }
    if !blocking {
        queue.set_blocking(false);
    }
    if let Ok(capacity) = usize::try_from(capacity)
        && capacity > 0
    {
        queue.set_capacity(capacity);
    }
    let id = queue.id();
    // The worker owns the queue from here.
    let record = worker::record(worker);
    record.set_queue(Box::into_raw(Box::new(queue)).cast());
    #[expect(
        clippy::cast_possible_truncation,
        clippy::cast_sign_loss,
        reason = "upstream stores the mode in a uint8_t"
    )]
    record.set_msgqueue_mode(mode as u8);
    call.update_property_long(Some(&PROCESS), c"msgQueueId", i64::from(id));
    call.update_property_long(Some(&PROCESS), c"msgQueueKey", key);
    call.set_bool(true);
}

fn stat_queue(call: &Call) {
    let worker = checked(this(call));
    if queue_of(worker).is_null() {
        fatal(
            E_WARNING,
            &message!("no queue, can't get stats of the queue"),
        );
        call.set_bool(false);
        return;
    }
    // SAFETY: a worker with a queue.
    let Some((num, bytes)) = unsafe { &*queue_of(worker) }.stat() else {
        call.set_bool(false);
        return;
    };
    call.set_array();
    #[expect(
        clippy::cast_possible_wrap,
        reason = "upstream stores size_t in zend_long"
    )]
    // SAFETY: the array just initialised.
    unsafe {
        add_assoc_long(call.return_value(), c"queue_num", num as i64);
    }
    #[expect(clippy::cast_possible_wrap, reason = "as above")]
    // SAFETY: as above.
    unsafe {
        add_assoc_long(call.return_value(), c"queue_bytes", bytes as i64);
    }
}

fn free_queue(call: &Call) {
    let worker = checked(this(call));
    let queue = queue_of(worker);
    // SAFETY: the worker's live queue, if any.
    let freed = !queue.is_null() && unsafe { &mut *queue }.destroy();
    if freed {
        // SAFETY: the worker's boxed queue, freed once and forgotten.
        drop(unsafe { Box::from_raw(queue) });
        worker::record(worker).set_queue(ptr::null_mut());
    }
    call.set_bool(freed);
}

fn kill(call: &Call) {
    let parsed = call
        .arity(1, 2)
        .and_then(|()| Ok((call.long(1, 0)?, call.long(2, i64::from(libc::SIGTERM))?)));
    let Ok((pid, sig)) = parsed else {
        call.set_bool(false);
        return;
    };
    #[expect(clippy::cast_possible_truncation, reason = "upstream casts to int")]
    let (pid, sig) = (pid as c_int, sig as c_int);
    if swoole_rs_process::os::kill(pid, sig) < 0 {
        if !(sig == 0 && errno() == libc::ESRCH) {
            sys(E_WARNING, &message!("kill({pid}, {sig}) failed"));
        }
        call.set_bool(false);
        return;
    }
    call.set_bool(true);
}

fn alarm(call: &Call) {
    let parsed = call
        .arity(1, 2)
        .and_then(|()| Ok((call.long(1, 0)?, call.long(2, 0)?)));
    let Ok((usec, kind)) = parsed else {
        call.set_bool(false);
        return;
    };
    // SAFETY: C++'s reader of an ini flag.
    if !cli() {
        fatal(E_ERROR, &message!("cannot use Swoole\\Process::alarm here"));
        call.set_bool(false);
        return;
    }
    // SAFETY: `SwooleTG.timer != nullptr`.
    if unsafe { worker::swoole_timer_is_available() } {
        fatal(
            E_WARNING,
            &message!("cannot use both 'timer' and 'alarm' at the same time"),
        );
        call.set_bool(false);
        return;
    }
    // SAFETY: all-zero is a disarmed timer.
    let mut timer: libc::itimerval = unsafe { std::mem::zeroed() };
    if usec > 0 {
        let interval = libc::timeval {
            tv_sec: (usec / 1_000_000) as _,
            tv_usec: (usec % 1_000_000) as _,
        };
        timer.it_interval = interval;
        timer.it_value = interval;
    }
    #[expect(clippy::cast_possible_truncation, reason = "upstream passes an int")]
    // SAFETY: `timer` is a valid itimerval; the old value is not wanted.
    let set = unsafe { libc::setitimer(kind as _, &raw const timer, ptr::null_mut()) };
    if set < 0 {
        sys(E_WARNING, &message!("setitimer() failed"));
        call.set_bool(false);
        return;
    }
    call.set_bool(true);
}

fn start(call: &Call) {
    let worker = checked(this(call));
    let pid = view(worker).pid;
    if pid != 0 && swoole_rs_process::os::kill(pid, 0) == 0 {
        fatal(E_WARNING, &message!("process has already been started"));
        call.set_bool(false);
        return;
    }
    let pid = worker::fork(worker);
    match pid.cmp(&0) {
        std::cmp::Ordering::Less => {
            sys(E_WARNING, &message!("fork() failed"));
            call.set_bool(false);
        }
        std::cmp::Ordering::Greater => {
            let server = host::server_class();
            // SAFETY: `$this`; upstream names Server as the scope of the public property.
            unsafe { update_property_long(server, call.this(), c"pid", i64::from(pid)) };
            call.set_long(i64::from(pid));
        }
        std::cmp::Ordering::Equal => {
            // SAFETY: the child: `$this` and its worker.
            let started = unsafe { swr_v1_phpprocess_start(worker, call.this_zval()) };
            call.set_bool(started >= 0);
        }
    }
}

fn read(call: &Call) {
    let Ok(size) = call.arity(0, 1).and_then(|()| call.long(1, 8192)) else {
        call.set_bool(false);
        return;
    };
    let process = this(call);
    let worker = checked(process);
    if view(worker).pipe < 0 {
        fatal(E_WARNING, &message!("no pipe, cannot read from pipe"));
        call.set_bool(false);
        return;
    }
    #[expect(
        clippy::cast_sign_loss,
        clippy::cast_possible_truncation,
        reason = "upstream converts to size_t"
    )]
    let size = size as usize;
    // SAFETY: Zend's allocator; it bails out when it cannot allocate.
    let buffer = unsafe { swr_zend_string_alloc(size) };
    let start = string_data(buffer);
    // SAFETY: the string has room for `size` bytes and a NUL.
    let read = unsafe { worker::pipe_read(worker, start, size, process.blocking) };
    let Ok(length) = usize::try_from(read) else {
        // SAFETY: the string made above, never shared.
        unsafe { swr_zend_string_free(buffer) };
        call.set_bool(false);
        return;
    };
    let end = start.wrapping_add(length);
    // SAFETY: `length <= size`, inside the string.
    unsafe { *end = 0 };
    // SAFETY: the string made above.
    unsafe { (*buffer).len = length };
    // SAFETY: the return value takes the string's one reference.
    unsafe { call.set_string(buffer) };
}

fn string_data(string: *mut zend_string) -> *mut c_char {
    string
        .wrapping_byte_add(std::mem::offset_of!(zend_string, val))
        .cast()
}

fn write(call: &Call) {
    let Ok(data) = call.arity(1, 1).and_then(|()| call.string(1, b"")) else {
        call.set_bool(false);
        return;
    };
    if data.is_empty() {
        fatal(E_WARNING, &message!("the data to send is empty"));
        call.set_bool(false);
        return;
    }
    let process = this(call);
    let worker = checked(process);
    if view(worker).pipe < 0 {
        fatal(E_WARNING, &message!("no pipe, cannot write into pipe"));
        call.set_bool(false);
        return;
    }
    // SAFETY: the argument's bytes.
    let written =
        unsafe { worker::pipe_write(worker, data.as_ptr().cast(), data.len(), process.blocking) };
    if written < 0 {
        sys(E_WARNING, &message!("write() failed"));
        call.set_bool(false);
        return;
    }
    call.set_long(written as i64);
}

fn export_socket(call: &Call) {
    let process = this(call);
    let pipe = if process.worker.is_null() {
        -1
    } else {
        view(process.worker).pipe
    };
    if pipe < 0 {
        fatal(E_WARNING, &message!("no pipe, cannot export stream"));
        call.set_bool(false);
        return;
    }
    if process.socket.is_null() {
        let stream = i64::from(process.pipe_type) == PIPE_TYPE_STREAM;
        process.socket = host::dup_socket(pipe, stream);
        if process.socket.is_null() {
            call.set_bool(false);
            return;
        }
    }
    // SAFETY: the object keeps its reference; the return value takes a new one.
    unsafe { swr_zend_object_addref(process.socket) };
    // SAFETY: as above.
    unsafe { set_object(call.return_value(), process.socket) };
}

fn push(call: &Call) {
    let Ok(data) = call.arity(1, 1).and_then(|()| call.string(1, b"")) else {
        call.set_bool(false);
        return;
    };
    if data.is_empty() {
        fatal(E_WARNING, &message!("the data to push is empty"));
        call.set_bool(false);
        return;
    }
    if data.len() >= MSGMAX {
        fatal(E_WARNING, &message!("the data to push is too big"));
        call.set_bool(false);
        return;
    }
    let worker = checked(this(call));
    let view = view(worker);
    if queue_of(worker).is_null() {
        fatal(E_WARNING, &message!("no msgqueue, cannot use push()"));
        call.set_bool(false);
        return;
    }
    let mut message = QueueMessage::new(c_long::from(view.id) + 1);
    if let Some(payload) = message.data().get_mut(..data.len()) {
        payload.copy_from_slice(data);
    }
    // SAFETY: a worker with a queue, and a message with `data.len()` payload bytes.
    let pushed =
        unsafe { swr_v1_os_msg_queue_push(queue_of(worker), message.as_ptr(), data.len()) };
    call.set_bool(pushed);
}

fn pop(call: &Call) {
    let Ok(size) = call.arity(0, 1).and_then(|()| call.long(1, MSGMAX_LONG)) else {
        call.set_bool(false);
        return;
    };
    let size = usize::try_from(size)
        .ok()
        .filter(|size| (1..=MSGMAX).contains(size))
        .unwrap_or(MSGMAX);
    let worker = checked(this(call));
    let view = view(worker);
    if queue_of(worker).is_null() {
        fatal(E_WARNING, &message!("no msgqueue, cannot use pop()"));
        call.set_bool(false);
        return;
    }
    let balance = i64::from(worker::record(worker).msgqueue_mode()) == MSGQUEUE_BALANCE;
    let mut message = QueueMessage::new(if balance {
        0
    } else {
        c_long::from(view.id) + 1
    });
    // SAFETY: a worker with a queue, and room for `size` payload bytes.
    let received = unsafe { swr_v1_os_msg_queue_pop(queue_of(worker), message.as_ptr(), size) };
    let Ok(length) = usize::try_from(received) else {
        call.set_bool(false);
        return;
    };
    let data = message.data();
    call.set_bytes(data.get(..length).unwrap_or(data));
}

fn exec(call: &Call) {
    let parsed = call
        .arity(2, 2)
        .and_then(|()| Ok((call.string(1, b"")?, call.array_table(2)?)));
    let Ok((file, Some(args))) = parsed else {
        call.set_bool(false);
        return;
    };
    if file.is_empty() {
        fatal(E_WARNING, &message!("exec file name is empty"));
        call.set_bool(false);
        return;
    }
    // The argument's string is NUL-terminated in place. The vector is request
    // memory, as upstream's, so a bailout from a conversion warning leaks nothing.
    let path = file.as_ptr().cast::<c_char>();
    // SAFETY: a live array.
    let count = unsafe { (*args).nNumOfElements } as usize;
    // SAFETY: Zend's allocator; it bails out when it cannot allocate.
    let argv = unsafe { swr_zend_emalloc(size_of::<*const c_char>() * (count + 2)) }
        .cast::<*const c_char>();
    // SAFETY: slot 0 of `count + 2`.
    unsafe { *argv = path };
    let mut next = 1;
    let each = |value: *mut zval| {
        // SAFETY: a live element; the string is leaked, as upstream leaks it.
        let string = unsafe { swoole_rs_zend::value::get_string(value) };
        let slot = argv.wrapping_add(next);
        // SAFETY: at most `count` elements, so `next <= count`.
        unsafe { *slot = string_data(string).cast_const() };
        next += 1;
        true
    };
    // SAFETY: the array argument, which the loop does not modify.
    unsafe { each_value(args, each) };
    let last = argv.wrapping_add(next);
    // SAFETY: `next <= count + 1`.
    unsafe { *last = ptr::null() };
    // SAFETY: a NUL-terminated path and a null-terminated argument vector.
    let failed = unsafe { libc::execv(path, argv) } < 0;
    if failed {
        sys(
            E_WARNING,
            &message!("execv({}) failed", String::from_utf8_lossy(file)),
        );
        call.set_bool(false);
    } else {
        call.set_bool(true);
    }
}

fn daemon(call: &Call) {
    let parsed = call.arity(0, 3).and_then(|()| {
        Ok((
            call.bool(1, true)?,
            call.bool(2, true)?,
            call.array_table(3)?,
        ))
    });
    let Ok((nochdir, noclose, pipes)) = parsed else {
        call.set_bool(false);
        return;
    };
    if let Some(pipes) = pipes {
        let mut fd = 0;
        let each = |value: *mut zval| {
            // SAFETY: a live element.
            if unsafe { type_of(value) } != IS_NULL {
                // SAFETY: C++ resolves a stream, socket or int to its fd.
                let from = unsafe { host::convert_to_fd(value) };
                if from >= 0 {
                    worker::dup(from, fd);
                }
            }
            let more = fd != 2;
            fd += 1;
            more
        };
        // SAFETY: the array argument, which the loop does not modify.
        unsafe { each_value(pipes, each) };
    }
    // SAFETY: crates/ossupport's daemon().
    call.set_bool(
        unsafe { worker::swoole_daemon(c_int::from(nochdir), c_int::from(noclose)) } == 0,
    );
}

fn exit(call: &Call) {
    let Ok(code) = call.arity(0, 1).and_then(|()| call.long(1, 0)) else {
        call.set_bool(false);
        return;
    };
    let worker = checked(this(call));
    if swoole_rs_process::os::getpid() != view(worker).pid {
        fatal(E_WARNING, &message!("not current process"));
        call.set_bool(false);
        return;
    }
    let code = if (0..=255).contains(&code) {
        code
    } else {
        fatal(
            E_WARNING,
            &message!("exit ret_code range is [>0 and <255] "),
        );
        1
    };
    // SAFETY: the event loop's teardown.
    if unsafe { worker::swoole_event_is_available() } {
        // SAFETY: as above.
        unsafe { worker::swoole_event_free() };
    }
    #[expect(clippy::cast_possible_truncation, reason = "0..=255")]
    // SAFETY: exits the process, as upstream does.
    unsafe {
        libc::exit(code as c_int)
    }
}

fn close(call: &Call) {
    let Ok(which) = call.arity(0, 1).and_then(|()| call.long(1, 0)) else {
        call.set_bool(false);
        return;
    };
    let worker = checked(this(call));
    let view = view(worker);
    if view.pipe < 0 {
        fatal(E_WARNING, &message!("no pipe, cannot close the pipe"));
        call.set_bool(false);
        return;
    }
    if !view.pipe_object {
        fatal(E_WARNING, &message!("cannot close the pipe"));
        call.set_bool(false);
        return;
    }
    // SAFETY: a worker owning its pipe.
    if unsafe { worker::pipe_close(worker, which) } < 0 {
        sys(E_WARNING, &message!("close() failed"));
        call.set_bool(false);
        return;
    }
    call.set_bool(true);
}

fn set(call: &Call) {
    let Ok(Some(options)) = call.arity(1, 1).and_then(|()| call.array_table(1)) else {
        call.set_bool(false);
        return;
    };
    let process = this(call);
    checked(process);
    let key = c"enable_coroutine";
    // SAFETY: the array argument and a NUL-terminated key.
    let value = unsafe { zend_hash_str_find(options, key.as_ptr(), key.count_bytes()) };
    // SAFETY: a live element.
    if !value.is_null() && unsafe { type_of(value) } != IS_NULL {
        // SAFETY: as above.
        process.enable_coroutine = unsafe { swr_zend_is_true(value) };
    }
}

fn set_timeout(call: &Call) {
    let Ok(seconds) = call.arity(1, 1).and_then(|()| call.double(1, 0.0)) else {
        call.set_bool(false);
        return;
    };
    let worker = checked(this(call));
    if view(worker).pipe < 0 {
        fatal(E_WARNING, &message!("no pipe, cannot setTimeout the pipe"));
        call.set_bool(false);
        return;
    }
    // SAFETY: a worker with a pipe.
    call.set_bool(unsafe { worker::pipe_timeout(worker, seconds) });
}

fn set_blocking(call: &Call) {
    let Ok(blocking) = call.arity(1, 1).and_then(|()| call.bool(1, false)) else {
        call.set_bool(false);
        return;
    };
    let process = this(call);
    if process.worker.is_null() || view(process.worker).pipe < 0 {
        fatal(E_WARNING, &message!("no pipe, cannot setBlocking the pipe"));
        call.set_bool(false);
        return;
    }
    process.blocking = blocking;
    // SAFETY: a worker with a pipe.
    call.set_bool(unsafe { worker::pipe_blocking(process.worker, blocking) });
}

/// `SW_CHECK_PRIORITY_WHO`: a null `$who` means this process for
/// `PRIO_PROCESS`; otherwise it is an error.
fn priority_who(call: &Call, which: i64, who: Option<i64>) -> Option<i64> {
    if let Some(who) = who {
        return Some(who);
    }
    if which == i64::from(libc::PRIO_PROCESS) {
        let worker = checked(this(call));
        return Some(i64::from(view(worker).pid));
    }
    fatal(E_WARNING, &message!("$who parameter must not be null"));
    set_last_error(INVALID_PARAMS);
    call.set_bool(false);
    None
}

#[expect(
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    reason = "upstream passes zend_long to setpriority's int and id_t parameters"
)]
fn set_priority(call: &Call) {
    let parsed = call
        .arity(2, 3)
        .and_then(|()| Ok((call.long(1, 0)?, call.long(2, 0)?, call.long_or_null(3)?)));
    let Ok((which, priority, who)) = parsed else {
        call.set_bool(false);
        return;
    };
    let Some(who) = priority_who(call, which, who) else {
        return;
    };
    // SAFETY: setpriority has no memory preconditions.
    if unsafe { libc::setpriority(which as _, who as _, priority as c_int) } < 0 {
        set_last_error(errno());
        call.set_bool(false);
        return;
    }
    call.set_bool(true);
}

#[expect(
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    reason = "upstream passes zend_long to getpriority's int and id_t parameters"
)]
fn get_priority(call: &Call) {
    let parsed = call
        .arity(1, 2)
        .and_then(|()| Ok((call.long(1, 0)?, call.long_or_null(2)?)));
    let Ok((which, who)) = parsed else {
        call.set_bool(false);
        return;
    };
    let Some(who) = priority_who(call, which, who) else {
        return;
    };
    set_errno(0);
    // SAFETY: getpriority has no memory preconditions.
    let priority = unsafe { libc::getpriority(which as _, who as _) };
    if priority == -1 && errno() != 0 {
        set_last_error(errno());
        call.set_bool(false);
        return;
    }
    call.set_long(i64::from(priority));
}

handlers! {
    zim_swoole_process___construct => construct;
    zim_swoole_process___destruct => destruct;
    zim_swoole_process_wait => wait;
    zim_swoole_process_signal => signal;
    zim_swoole_process_alarm => alarm;
    zim_swoole_process_kill => kill;
    zim_swoole_process_daemon => daemon;
    zim_swoole_process_setPriority => set_priority;
    zim_swoole_process_getPriority => get_priority;
    zim_swoole_process_set => set;
    zim_swoole_process_setTimeout => set_timeout;
    zim_swoole_process_setBlocking => set_blocking;
    zim_swoole_process_useQueue => use_queue;
    zim_swoole_process_statQueue => stat_queue;
    zim_swoole_process_freeQueue => free_queue;
    zim_swoole_process_start => start;
    zim_swoole_process_write => write;
    zim_swoole_process_close => close;
    zim_swoole_process_read => read;
    zim_swoole_process_push => push;
    zim_swoole_process_pop => pop;
    zim_swoole_process_exit => exit;
    zim_swoole_process_exec => exec;
    zim_swoole_process_exportSocket => export_socket;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn queue_keys_cover_signed_and_unsigned_key_t() {
        assert!(key_is_valid(i64::from(i32::MIN)), "the signed minimum");
        assert!(!key_is_valid(i64::from(i32::MIN) - 1), "below it");
        assert!(key_is_valid(i64::from(u32::MAX)), "the unsigned maximum");
        assert!(!key_is_valid(i64::from(u32::MAX) + 1), "above it");
    }

    #[test]
    fn signal_constants_follow_upstream_order() {
        let names: Vec<_> = signal_constants().iter().map(|(name, _)| *name).collect();
        assert_eq!(names.first(), Some(&c"SIGHUP"), "starts with SIGHUP");
        assert_eq!(names.last(), Some(&c"PRIO_USER"), "ends with PRIO_USER");
        assert!(names.contains(&c"SIG_IGN"), "SIG_IGN is registered");
    }

    #[test]
    fn process_object_matches_upstream_layout() {
        assert_eq!(
            ProcessObject::OFFSET,
            2 * size_of::<*mut c_void>() + size_of::<c_int>() + 2 + 2,
            "worker, zsocket, pipe_type, two bools, padded to the zend_object"
        );
    }
}
