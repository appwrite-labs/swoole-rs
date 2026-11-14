//! `Swoole\Process`: registration, objects and methods. Each method mirrors
//! upstream's ZPP block and checks, then works on the object's `Worker` through
//! the host table.
use crate::{
    host::{self, Callable},
    report::{INVALID_PARAMS, cli, errno, fatal, notice, set_errno, set_last_error, sys},
    signal::signal,
    worker::{self, WorkerView},
};
use std::{
    cell::{Cell, RefCell},
    ffi::{CStr, c_char, c_int, c_long},
    ptr,
    sync::atomic::{AtomicU32, Ordering},
};
use swoole_rs_corebase::CCast;
use swoole_rs_ossupport::QueueRecord;
use swoole_rs_phpstubs::process;
use swoole_rs_process::worker_record::{Worker, WorkerRecord};
use swoole_rs_zend::{
    Call, Class, Custom, CustomClass, E_ERROR, E_WARNING, Minit, Owned, Value, bailout, handlers,
    message,
    string::ZBuf,
    sys::{_zend_bailout, IS_NULL, ZEND_ACC_PRIVATE, ZEND_ACC_PUBLIC, zend_string},
};
use swoole_rs_zend::{method_table, sys as z};

/// What a method asks of the executor with only its call in hand: the function
/// takes a [`Php`](swoole_rs_zend::Php), so only a thread that runs PHP reaches
/// the shim, whose declaration stays private to this module.
mod leased {
    unsafe extern "C" {
        /// `ftok(zend_get_executed_filename(), 1)`: it reads this thread's
        /// executor for the file whose code runs and stats that file. Runs no
        /// PHP code.
        safe fn swr_zend_process_ftok() -> libc::key_t;
    }

    /// The System V key of the file whose code is running; -1 for none.
    pub(super) fn executed_file_key(_php: swoole_rs_zend::Php<'_>) -> libc::key_t {
        swr_zend_process_ftok()
    }
}

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

static PROCESS: CustomClass<Process> = CustomClass::new();

/// `round_process_id`: the id of the next process made outside a server worker.
static ROUND_ID: AtomicU32 = AtomicU32::new(0);

/// `struct ProcessObject`: the worker it owns (null before its constructor
/// ran), the queue it owns, its exported socket, pipe type and flags.
///
/// The worker record's queue pointer never owns: it points into `queue` after
/// `useQueue`, or at the pool's queue on a worker from `Pool::getProcess`, which
/// the pool frees. Only `queue` is ever freed here, by dropping it.
#[derive(Default)]
struct Process {
    /// Dropped first, as the C++ object's free releases it before the worker.
    socket: RefCell<Option<Owned>>,
    worker: WorkerSlot,
    pipe_type: Cell<c_int>,
    enable_coroutine: Cell<bool>,
    blocking: Cell<bool>,
}

impl Custom for Process {
    fn class() -> &'static CustomClass<Self> {
        &PROCESS
    }
    fn new() -> Self {
        Self::default()
    }
}

/// `Swoole\Process`.
#[must_use]
pub fn process_class() -> &'static Class {
    &PROCESS
}

/// `$this`, which every instance method runs on.
fn this(call: &Call) -> Option<&Process> {
    call.custom()
}

/// `php_swoole_process_get_and_check_worker`: fatal without a constructed worker.
/// None once the error returns (no fatal-error hook is installed).
fn checked(process: Option<&Process>) -> Option<Worker<'_>> {
    let worker = process.and_then(|process| process.worker.get());
    if worker.is_none() {
        host::unconstructed();
    }
    worker
}

/// The worker of a Process object (none before its constructor ran).
pub(crate) fn worker_of(object: &Value) -> Option<Worker<'_>> {
    object.custom::<Process>()?.worker.get()
}

/// `php_swoole_process_set_worker` on a new Process object, which takes
/// `worker` (dropped when the object has one already, or is no Process).
pub(crate) fn set_worker_of(
    object: &Value,
    worker: WorkerRecord,
    enable_coroutine: bool,
    pipe_type: c_int,
) {
    if let Some(process) = object.custom::<Process>()
        && process.worker.set(worker)
    {
        configure(process, enable_coroutine, pipe_type);
    }
}

/// A live worker's queue, or null.
fn queue_of(worker: Worker<'_>) -> *mut QueueRecord {
    worker.queue().cast()
}

/// `f` over a live worker's queue, or `None` when it has none.
fn with_queue<R>(worker: Worker<'_>, f: impl FnOnce(&QueueRecord) -> R) -> Option<R> {
    // SAFETY: null, or a queue its Process object or the pool owns, which lives until
    // `freeQueue` or its owner's drop, neither of which runs during `f`.
    unsafe { queue_of(worker).as_ref() }.map(f)
}

fn view(worker: Worker<'_>) -> WorkerView {
    worker::view(worker)
}

// `swoole_set_process_name()` (php_swoole.cc), which `Process::name()` aliases.
use swoole_rs_phpswoole::zif_swoole_set_process_name;

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

/// `php_swoole_process_minit`: register `Swoole\Process`, its constants and
/// properties, and (when pcntl is absent) the signal constants.
pub fn php_swoole_process_minit(minit: &Minit) {
    minit.register_object::<Process>(c"Swoole\\Process", METHODS.rows(), 0);
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
        minit.class_constant_long(&PROCESS, name, value);
    }
    for name in [c"pipe", c"msgQueueId", c"msgQueueKey", c"pid", c"id"] {
        minit.property_null(&PROCESS, name, ZEND_ACC_PUBLIC);
    }
    minit.property_null(&PROCESS, c"callback", ZEND_ACC_PRIVATE);
    if !swoole_rs_zend::module_loaded(c"pcntl") {
        for (name, value) in signal_constants() {
            minit.long_constant(name, value);
        }
    }
    minit.long_constant(c"SWOOLE_MSGQUEUE_ORIENT", MSGQUEUE_ORIENT);
    minit.long_constant(c"SWOOLE_MSGQUEUE_BALANCE", MSGQUEUE_BALANCE);
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

/// `php_swoole_process_get_and_check_worker`: the worker of a Process object, which
/// the object keeps for as long as it lives; none, with the error raised, before
/// its constructor ran.
pub fn checked_worker(object: &Value) -> Option<Worker<'_>> {
    checked(object.custom())
}

/// `php_swoole_process_set_worker`'s flags, once the object has its worker.
fn configure(process: &Process, enable_coroutine: bool, pipe_type: c_int) {
    process.pipe_type.set(pipe_type);
    process.enable_coroutine.set(enable_coroutine);
    process.blocking.set(true);
}

/// `php_swoole_process_start`: in the child, run the callback, then bail out
/// as `exit` would. It returns only when the callback or the event loop cannot
/// start; a bailout pending on the way is raised here, as the closing one is.
/// `worker` is the Process object `this`'s.
pub fn start_worker(worker: Worker<'_>, this: &Value) {
    start_child(worker, this);
}

/// [`start_worker`] on `$this`.
fn start_child(worker: Worker<'_>, this: &Value) {
    if this.object().is_none() {
        return;
    }
    // The declared private property.
    let callable = match this.read_property(Some(&PROCESS), c"callback") {
        Ok(callback) => callback.as_deref().and_then(Callable::create),
        Err(bailout) => {
            bailout.defer();
            None
        }
    };
    let Some(callable) = callable else {
        swoole_rs_zend::bailout::finish();
        return;
    };
    worker::child_enter(worker);
    let worker = view(worker);
    for (redirect, target) in [
        (worker.redirect_stdin, libc::STDIN_FILENO),
        (worker.redirect_stdout, libc::STDOUT_FILENO),
        (worker.redirect_stderr, libc::STDERR_FILENO),
    ] {
        // Without a pipe, `dup2(-1, …)` fails as upstream's does.
        if redirect && swoole_rs_corebase::sys::dup2(worker.pipe.unwrap_or(-1), target) < 0 {
            sys(E_WARNING, &message!("dup2() failed"));
        }
    }
    crate::signal::clean();
    if let Some(props) = this.props(Some(&PROCESS)) {
        props.long(c"pid", i64::from(worker.pid));
        if let Some(pipe) = worker.pipe {
            props.long(c"pipe", i64::from(pipe));
        }
    }
    let coroutine = this
        .custom::<Process>()
        .is_some_and(|process| process.enable_coroutine.get());
    if swoole_rs_zend::bailout::pending() || (coroutine && !host::reactor_init()) {
        drop(callable);
        swoole_rs_zend::bailout::finish();
        return;
    }
    // `$this` is the callable's one argument.
    let called = callable.call(&[this], coroutine);
    let bailed = swoole_rs_zend::bailout::pending();
    if !called && !bailed {
        notice(
            E_WARNING,
            &message!(
                "{}->onStart handler error",
                this.value_name().to_string_lossy()
            ),
        );
    }
    if coroutine && !bailed {
        host::event_wait();
    }
    drop(callable);
    swoole_rs_zend::bailout::finish();
    // SAFETY: equivalent to exit: Zend unwinds to its bailout point; this frame
    // owns nothing that needs a drop.
    unsafe { _zend_bailout(c"crates/phpprocess/src/process.rs".as_ptr(), line!()) }
}

fn construct(call: &Call) {
    let Some((process, object)) = this(call).zip(call.this_value()) else {
        return;
    };
    let name = object.value_name().to_string_lossy();
    let refuse = |text| {
        call.throw_error(message!("{text}").as_cstr());
        call.set_bool(false);
    };
    if process.worker.get().is_some() {
        call.throw_constructed_twice();
        call.set_bool(false);
        return;
    }
    if !cli() {
        return refuse(format!("{name} can only be used in PHP CLI mode"));
    }
    match host::construct_guard() {
        Err(host::ConstructRefusal::Master) => {
            return refuse(format!("{name} can't be used in master process"));
        }
        Err(host::ConstructRefusal::AsyncThreads) => {
            return refuse(format!("unable to create {name} with async-io threads"));
        }
        Ok(()) => {}
    }
    drop(name);
    // Upstream makes the worker before parsing, so a failed parse leaves it set.
    // A new worker the object owns from here.
    process.worker.set(worker::new());
    let Some(worker) = process.worker.get() else {
        return;
    };
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
    if !worker::share(worker) {
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
            let Some(fd) = worker::open_pipe(worker, socket_type) else {
                call.throw(
                    swoole_rs_phpswoole::exception(),
                    c"failed to create unix soccket",
                    i64::from(errno()),
                );
                call.set_bool(false);
                return;
            };
            call.update_property_long(Some(&PROCESS), c"pipe", i64::from(fd));
        }
    }
    call.update_property_long(Some(&PROCESS), c"id", i64::from(view(worker).id));
    if let Some((props, callback)) = object.props(Some(&PROCESS)).zip(call.value(1)) {
        props.value(c"callback", callback);
    }
    configure(process, enable_coroutine, pipe_type.c_cast::<c_int>());
}

fn destruct(_call: &Call) {}

fn wait(call: &Call) {
    let Ok(blocking) = call.arity(0, 1).and_then(|()| call.bool(1, true)) else {
        call.set_bool(false);
        return;
    };
    let mut status = 0;
    let options = if blocking { 0 } else { libc::WNOHANG };
    let pid = swoole_rs_corebase::sys::waitpid(-1, Some(&mut status), options);
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
    let process = this(call);
    let Some(worker) = checked(process) else {
        return;
    };
    if key > 0 && !key_is_valid(key) {
        fatal(
            E_WARNING,
            &message!("message queue key is out of range [{key}]"),
        );
        call.set_bool(false);
        return;
    }
    if key <= 0 {
        key = i64::from(leased::executed_file_key(call.php()));
    }
    let blocking = mode & IPC_NOWAIT == 0;
    if !blocking {
        mode &= !IPC_NOWAIT;
    }
    let mut queue = QueueRecord::new(key.c_cast::<i32>(), true, 0);
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
    if let Some(process) = process {
        process.worker.queue.own(worker, Box::new(queue));
    }
    let mode = mode.c_cast::<u8>();
    worker.set_msgqueue_mode(mode);
    call.update_property_long(Some(&PROCESS), c"msgQueueId", i64::from(id));
    call.update_property_long(Some(&PROCESS), c"msgQueueKey", key);
    call.set_bool(true);
}

fn stat_queue(call: &Call) {
    let Some(worker) = checked(this(call)) else {
        return;
    };
    let Some(stat) = with_queue(worker, QueueRecord::stat) else {
        fatal(
            E_WARNING,
            &message!("no queue, can't get stats of the queue"),
        );
        call.set_bool(false);
        return;
    };
    let Some((num, bytes)) = stat else {
        call.set_bool(false);
        return;
    };
    call.set_array();
    call.add_assoc_long(c"queue_num", num.c_cast::<i64>());
    #[expect(clippy::cast_possible_wrap, reason = "as above")]
    call.add_assoc_long(c"queue_bytes", bytes as i64);
}

fn free_queue(call: &Call) {
    let process = this(call);
    let Some(worker) = checked(process) else {
        return;
    };
    call.set_bool(process.is_some_and(|process| process.worker.queue.free(worker)));
}

/// The worker a Process owns (none before its constructor ran) and the queue it
/// owns, freed together when the slot drops, the worker first; a queue the worker
/// borrows (the pool's) is never freed here.
#[derive(Default)]
struct WorkerSlot {
    worker: worker::Slot,
    queue: QueueSlot,
}

impl WorkerSlot {
    fn get(&self) -> Option<Worker<'_>> {
        self.worker.get()
    }

    fn set(&self, worker: WorkerRecord) -> bool {
        self.worker.set(worker)
    }
}

/// The queue a Process owns, if any. A worker's queue pointer is set from here
/// or borrowed from a pool; only the box held here is ever freed, by dropping it.
#[derive(Default)]
struct QueueSlot(RefCell<Option<Box<QueueRecord>>>);

impl QueueSlot {
    /// Takes `queue` and points `worker` at it; a queue it owned before is dropped.
    fn own(&self, worker: Worker<'_>, queue: Box<QueueRecord>) {
        let mut owned = self.0.borrow_mut();
        // Point at the box once it is in place: moving it would invalidate an earlier pointer.
        let queue = owned.insert(queue);
        let queue = ptr::from_mut(&mut **queue).cast();
        worker.set_queue(queue);
    }

    /// `freeQueue` on `worker`: destroys and drops a queue this slot owns, and only
    /// detaches one the worker borrows (the pool's).
    fn free(&self, worker: Worker<'_>) -> bool {
        if queue_of(worker).is_null() {
            return false;
        }
        let mut owned = self.0.borrow_mut();
        if owned.as_mut().is_some_and(|queue| !queue.destroy()) {
            return false;
        }
        worker.set_queue(ptr::null_mut());
        *owned = None;
        true
    }
}

fn kill(call: &Call) {
    let parsed = call
        .arity(1, 2)
        .and_then(|()| Ok((call.long(1, 0)?, call.long(2, i64::from(libc::SIGTERM))?)));
    let Ok((pid, sig)) = parsed else {
        call.set_bool(false);
        return;
    };
    let (pid, sig) = (pid.c_cast::<c_int>(), sig.c_cast::<c_int>());
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
    if !cli() {
        fatal(E_ERROR, &message!("cannot use Swoole\\Process::alarm here"));
        call.set_bool(false);
        return;
    }
    // `SwooleTG.timer != nullptr`.
    if swoole_rs_timer::swoole_timer_is_available() {
        fatal(
            E_WARNING,
            &message!("cannot use both 'timer' and 'alarm' at the same time"),
        );
        call.set_bool(false);
        return;
    }
    // Zero is a disarmed timer.
    let usec = usec.max(0);
    #[cfg_attr(
        not(target_vendor = "apple"),
        expect(
            trivial_numeric_casts,
            reason = "suseconds_t is i64 here, i32 on macOS"
        )
    )]
    let interval = libc::timeval {
        tv_sec: usec / 1_000_000,
        tv_usec: (usec % 1_000_000) as libc::suseconds_t,
    };
    let timer = libc::itimerval {
        it_interval: interval,
        it_value: interval,
    };
    let set = swoole_rs_corebase::sys::setitimer(kind.c_cast::<c_int>(), &timer);
    if set < 0 {
        sys(E_WARNING, &message!("setitimer() failed"));
        call.set_bool(false);
        return;
    }
    call.set_bool(true);
}

fn start(call: &Call) {
    let Some(worker) = checked(this(call)) else {
        return;
    };
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
            // Upstream names Server as the scope; the property is public, so
            // the scope does not change which slot is written.
            call.update_property_long(Some(&PROCESS), c"pid", i64::from(pid));
            call.set_long(i64::from(pid));
        }
        std::cmp::Ordering::Equal => {
            // The child returns here only when it could not start.
            if let Some(this) = call.this_value() {
                start_child(worker, this);
            }
            call.set_bool(false);
        }
    }
}

fn read(call: &Call) {
    let Ok(size) = call.arity(0, 1).and_then(|()| call.long(1, 8192)) else {
        call.set_bool(false);
        return;
    };
    let process = this(call);
    let Some(worker) = checked(process) else {
        return;
    };
    if view(worker).pipe.is_none() {
        fatal(E_WARNING, &message!("no pipe, cannot read from pipe"));
        call.set_bool(false);
        return;
    }
    let size = size.c_cast::<usize>();
    let mut buffer = ZBuf::with_capacity(size);
    let Some(length) = worker::pipe_read(worker, &mut buffer, blocking(process)) else {
        call.set_bool(false);
        return;
    };
    // SAFETY: the read wrote `length <= size` bytes.
    unsafe { buffer.set_len(length) };
    call.set_zstring(buffer.into_zstring());
}

/// The object's `blocking` flag.
fn blocking(process: Option<&Process>) -> bool {
    process.is_some_and(|process| process.blocking.get())
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
    let Some(worker) = checked(process) else {
        return;
    };
    if view(worker).pipe.is_none() {
        fatal(E_WARNING, &message!("no pipe, cannot write into pipe"));
        call.set_bool(false);
        return;
    }
    let Some(written) = worker::pipe_write(worker, data, blocking(process)) else {
        sys(E_WARNING, &message!("write() failed"));
        call.set_bool(false);
        return;
    };
    call.set_long(written.c_cast::<i64>());
}

fn export_socket(call: &Call) {
    let Some(process) = this(call) else {
        return;
    };
    let pipe = process.worker.get().and_then(|worker| view(worker).pipe);
    let Some(pipe) = pipe else {
        fatal(E_WARNING, &message!("no pipe, cannot export stream"));
        call.set_bool(false);
        return;
    };
    let mut socket = process.socket.borrow_mut();
    if socket.is_none() {
        let stream = i64::from(process.pipe_type.get()) == PIPE_TYPE_STREAM;
        *socket = host::dup_socket(pipe, stream);
    }
    match socket.as_ref() {
        // The object keeps its reference; the return value takes a new one.
        Some(socket) => call.set_owned(socket.copy()),
        None => call.set_bool(false),
    }
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
    let Some(worker) = checked(this(call)) else {
        return;
    };
    // `QueueNode`: the type, then the payload.
    let mut node = (c_long::from(view(worker).id) + 1).to_ne_bytes().to_vec();
    node.extend_from_slice(data);
    let Some(pushed) = with_queue(worker, |queue| queue.push(&node)) else {
        fatal(E_WARNING, &message!("no msgqueue, cannot use push()"));
        call.set_bool(false);
        return;
    };
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
    let Some(worker) = checked(this(call)) else {
        return;
    };
    let balance = i64::from(worker.msgqueue_mode()) == MSGQUEUE_BALANCE;
    let kind: c_long = if balance {
        0
    } else {
        c_long::from(view(worker).id) + 1
    };
    // `QueueNode`: the type, then room for `size` payload bytes.
    let mut node = kind.to_ne_bytes().to_vec();
    node.resize(node.len() + size, 0);
    let Some(received) = with_queue(worker, |queue| queue.pop(&mut node)) else {
        fatal(E_WARNING, &message!("no msgqueue, cannot use pop()"));
        call.set_bool(false);
        return;
    };
    let Some(length) = received else {
        call.set_bool(false);
        return;
    };
    let data = node.get(size_of::<c_long>()..).unwrap_or_default();
    call.set_bytes(data.get(..length).unwrap_or(data));
}

fn exec(call: &Call) {
    let parsed = call
        .arity(2, 2)
        .and_then(|()| Ok((call.string(1, b"")?, call.array_ref(2)?)));
    let Ok((file, args)) = parsed else {
        call.set_bool(false);
        return;
    };
    if file.is_empty() {
        fatal(E_WARNING, &message!("exec file name is empty"));
        call.set_bool(false);
        return;
    }
    // The argument's string is NUL-terminated in place.
    let path = file.as_ptr().cast::<c_char>();
    let mut strings = Vec::with_capacity(args.count() as usize);
    for (_, value) in args.entries() {
        let string = value.string();
        if bailout::pending() {
            // Upstream's conversion bailed out here.
            return;
        }
        strings.push(string);
    }
    let argv: Vec<*const c_char> = std::iter::once(path)
        .chain(
            strings
                .iter()
                .map(|string| string_data(string.as_ptr()).cast_const()),
        )
        .chain(std::iter::once(ptr::null()))
        .collect();
    // SAFETY: a NUL-terminated path and a null-terminated argument vector.
    let failed = unsafe { libc::execv(path, argv.as_ptr()) } < 0;
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
            call.value(3).map(|_| call.array_ref(3)).transpose()?,
        ))
    });
    let Ok((nochdir, noclose, pipes)) = parsed else {
        call.set_bool(false);
        return;
    };
    if let Some(pipes) = pipes {
        for ((_, value), fd) in pipes.entries().zip(0..=2) {
            if value.kind() != IS_NULL {
                // C++ resolves a stream, socket or int to its fd.
                if let Some(from) = host::convert_to_fd(value) {
                    worker::dup(from, fd);
                }
            }
        }
    }
    call.set_bool(
        swoole_rs_ossupport::swoole_daemon(c_int::from(nochdir), c_int::from(noclose)) == 0,
    );
}

fn exit(call: &Call) {
    let Ok(code) = call.arity(0, 1).and_then(|()| call.long(1, 0)) else {
        call.set_bool(false);
        return;
    };
    let Some(worker) = checked(this(call)) else {
        return;
    };
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
    // The event loop's teardown.
    if swoole_rs_reactor::event_api::swoole_event_is_available() {
        swoole_rs_reactor::swoole_event_free();
    }
    #[expect(clippy::cast_possible_truncation, reason = "0..=255")]
    std::process::exit(code as c_int)
}

fn close(call: &Call) {
    let Ok(which) = call.arity(0, 1).and_then(|()| call.long(1, 0)) else {
        call.set_bool(false);
        return;
    };
    let Some(worker) = checked(this(call)) else {
        return;
    };
    let view = view(worker);
    if view.pipe.is_none() {
        fatal(E_WARNING, &message!("no pipe, cannot close the pipe"));
        call.set_bool(false);
        return;
    }
    if !view.pipe_object {
        fatal(E_WARNING, &message!("cannot close the pipe"));
        call.set_bool(false);
        return;
    }
    if !worker::pipe_close(worker, which) {
        sys(E_WARNING, &message!("close() failed"));
        call.set_bool(false);
        return;
    }
    call.set_bool(true);
}

fn set(call: &Call) {
    let Ok(options) = call.arity(1, 1).and_then(|()| call.array_ref(1)) else {
        call.set_bool(false);
        return;
    };
    let process = this(call);
    checked(process);
    if let Some(process) = process
        && let Some(value) = options.find(b"enable_coroutine")
        && value.kind() != IS_NULL
    {
        process.enable_coroutine.set(value.truthy());
    }
}

fn set_timeout(call: &Call) {
    let Ok(seconds) = call.arity(1, 1).and_then(|()| call.double(1, 0.0)) else {
        call.set_bool(false);
        return;
    };
    let Some(worker) = checked(this(call)) else {
        return;
    };
    if view(worker).pipe.is_none() {
        fatal(E_WARNING, &message!("no pipe, cannot setTimeout the pipe"));
        call.set_bool(false);
        return;
    }
    call.set_bool(worker::pipe_timeout(worker, seconds));
}

fn set_blocking(call: &Call) {
    let Ok(blocking) = call.arity(1, 1).and_then(|()| call.bool(1, false)) else {
        call.set_bool(false);
        return;
    };
    let Some(process) = this(call) else {
        return;
    };
    let Some(worker) = process
        .worker
        .get()
        .filter(|worker| view(*worker).pipe.is_some())
    else {
        fatal(E_WARNING, &message!("no pipe, cannot setBlocking the pipe"));
        call.set_bool(false);
        return;
    };
    process.blocking.set(blocking);
    call.set_bool(worker::pipe_blocking(worker, blocking));
}

/// `SW_CHECK_PRIORITY_WHO`: a null `$who` means this process for
/// `PRIO_PROCESS`; otherwise it is an error.
fn priority_who(call: &Call, which: i64, who: Option<i64>) -> Option<i64> {
    if let Some(who) = who {
        return Some(who);
    }
    if which == i64::from(libc::PRIO_PROCESS) {
        return Some(i64::from(view(checked(this(call))?).pid));
    }
    fatal(E_WARNING, &message!("$who parameter must not be null"));
    set_last_error(INVALID_PARAMS);
    call.set_bool(false);
    None
}

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
    let who = who.c_cast::<libc::id_t>();
    if swoole_rs_corebase::sys::setpriority(
        which.c_cast::<c_int>(),
        who,
        priority.c_cast::<c_int>(),
    ) < 0
    {
        set_last_error(errno());
        call.set_bool(false);
        return;
    }
    call.set_bool(true);
}

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
    let priority =
        swoole_rs_corebase::sys::getpriority(which.c_cast::<c_int>(), who.c_cast::<libc::id_t>());
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

    // These drop a `WorkerSlot`, the part of a Process that owns its worker and
    // queue: the Process's own drop also releases its socket through PHP, which
    // unit-test binaries don't link.

    /// A Process's worker slot whose worker borrows `queue`, as one from
    /// `Pool::getProcess` does.
    fn borrowing(queue: &mut QueueRecord) -> WorkerSlot {
        let slot = WorkerSlot::default();
        assert!(slot.set(worker::new()), "a new slot takes a worker");
        worker(&slot).set_queue(ptr::from_mut(queue).cast());
        slot
    }

    /// The worker `slot` was given.
    fn worker(slot: &WorkerSlot) -> Worker<'_> {
        slot.get().expect("the slot has a worker")
    }

    #[test]
    fn a_borrowed_queue_outlives_the_process() {
        let mut queue = QueueRecord::new(libc::IPC_PRIVATE, true, 0);
        drop(borrowing(&mut queue));
        assert!(queue.stat().is_some(), "the owner's queue still exists");
    }

    #[test]
    fn free_queue_detaches_a_borrowed_queue() {
        let mut queue = QueueRecord::new(libc::IPC_PRIVATE, true, 0);
        let slot = borrowing(&mut queue);
        let worker = worker(&slot);
        assert!(slot.queue.free(worker), "detached");
        assert!(queue_of(worker).is_null(), "the worker no longer has it");
        assert!(!slot.queue.free(worker), "nothing left to free");
        drop(slot);
        assert!(queue.stat().is_some(), "the owner's queue still exists");
    }

    #[test]
    fn free_queue_destroys_an_owned_queue() {
        let slot = WorkerSlot::default();
        assert!(slot.set(worker::new()), "a new slot takes a worker");
        let worker = worker(&slot);
        let queue = QueueRecord::new(libc::IPC_PRIVATE, true, 0);
        let id = queue.id();
        slot.queue.own(worker, Box::new(queue));
        assert_eq!(
            with_queue(worker, QueueRecord::id),
            Some(id),
            "the worker points at it"
        );
        assert!(slot.queue.free(worker), "destroyed");
        assert!(queue_of(worker).is_null(), "and detached");
    }

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
}
