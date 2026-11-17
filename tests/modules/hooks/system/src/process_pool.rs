//! Upstream's `core-tests/src/os/process_pool.cpp`, one function per TEST with the same inputs
//! and expected values, over the Rust ProcessPool and Worker (crates/process) under their
//! upstream symbols. `ProcessPool pool{}` and `Worker worker{}` are zeroed mirrors of the
//! records Rust owns, checked against the fixture's sizes. The callbacks are fns of
//! crates/process's hook types, as upstream's are captureless lambdas, set with
//! `PoolRef::install`; a failed assertion inside one aborts the process it runs in (the
//! build's panics abort). `SwooleWG` is crates/process's `worker_globals`;
//! `swoole_set_isolation` is crates/corebase/src/ossupport's `set_isolation`. core-tests' shared counters are `sw_mem_pool()` memory
//! (crates/memory's `members::pool::alloc`), so workers' increments reach the master, as
//! upstream's do.

use std::ffi::{CStr, c_char, c_int, c_long, c_uint, c_void};
use std::io::{Read, Write};
use std::net::{Shutdown, TcpStream};
use std::os::unix::net::UnixStream;
use std::ptr::{self, null_mut};
use std::sync::atomic::{AtomicI32, AtomicPtr, Ordering::SeqCst};
use std::time::{SystemTime, UNIX_EPOCH};
use swoole_rs_corebase::constants::SW_IPC_MAX_SIZE;
use swoole_rs_corebase::swoole_thread_clean;
use swoole_rs_memory::members::pool as mem_pool;
use swoole_rs_process::methods as pm;
use swoole_rs_process::methods::{ExitStatus, swoole_get_worker_symbol, swoole_set_worker_type};
use swoole_rs_process::process_pool::{
    ErrorFn, EventFn, Hooks, MainLoopFn, MessageFn, NotFoundFn, PoolFn, PoolRef, Protocol,
    RecvData, TaskFn, WorkerFn,
};
use swoole_rs_process::task_file::Packed;
use swoole_rs_process::worker_record::{
    Worker as WorkerHandle, swr_v1_worker_init, swr_v1_worker_shutdown,
};

use super::timer::{after, tick};
use super::{SIGKILL, TEST_STR, last_error, random_bytes};
use swoole_rs_reactor::signal_upstream::swoole_signal_clear;
use swoole_rs_timer::{swoole_timer_clear, swoole_timer_free, swoole_timer_is_available};

const SW_OK: c_int = 0;
const SW_ERR: c_int = -1;
const SW_ERROR_INVALID_PARAMS: c_int = 505;
const SW_ERROR_OPERATION_NOT_SUPPORT: c_int = 507;
const SW_ERROR_WRONG_OPERATION: c_int = 509;
const SW_ERROR_PROTOCOL_ERROR: c_int = 508;
const SW_PIPE_MASTER: c_int = 1;
const SW_TASK_TMPFILE: u16 = 1;
const SW_MSGMAX: u32 = 65536;
const IPC_PRIVATE: c_int = 0;
// swIPCMode, ProtocolType, swWorkerType, swWorkerStatus.
const SW_IPC_NONE: c_int = 0;
const SW_IPC_UNIXSOCK: c_int = 1;
const SW_IPC_MSGQUEUE: c_int = 2;
const SW_IPC_SOCKET: c_int = 3;
const SW_MASTER: c_int = 1;
const SW_WORKER: c_int = 2;
const SW_MANAGER: c_int = 3;
const SW_TASK_WORKER: c_int = 4;
const SW_USER_WORKER: c_int = 5;
const SW_WORKER_BUSY: u8 = 1;
const SW_WORKER_IDLE: u8 = 2;
const SW_WORKER_EXIT: u8 = 3;
const SW_WORKER_MESSAGE_STOP: u8 = 1;
/// Linux's SW_IPC_MAX_SIZE (8192) less the DataHead.
const SW_IPC_BUFFER_SIZE: usize = 8192 - 40;
const TEST_HOST: &CStr = c"127.0.0.1";
const TEST_PORT: c_int = 9501;
const TEST_SOCK_FILE: &CStr = c"/tmp/swoole-core-tests.sock";
const TEST_COUNTER_NUM: usize = 32;
const MAGIC_NUMBER: c_int = 99_900_011;
// glibc's values (the core-tests build is Linux).
const EACCES: c_int = 13;
const ECHILD: c_int = 10;
const SIGIO: c_int = 29;
const SIGPIPE: c_int = 13;
const SIGTERM: c_int = 15;
const SIGWINCH: c_int = 28;
const SIG_DFL: usize = 0;
const SIG_IGN: usize = 1;
const IPC_CREAT: c_int = 0o1000;
const SHUT_RDWR: c_int = 2;

/// `swoole::DataHead`.
#[repr(C)]
#[derive(Clone, Copy)]
struct DataHead {
    fd: i64,
    msg_id: u64,
    len: u32,
    reactor_id: i16,
    kind: u8,
    flags: u8,
    server_fd: u16,
    ext_flags: u16,
    reserved: u32,
    time: f64,
}

/// `swoole::EventData`.
#[repr(C)]
struct EventData {
    info: DataHead,
    data: [u8; SW_IPC_BUFFER_SIZE],
}

impl EventData {
    /// `EventData msg{}`.
    fn new() -> Box<Self> {
        // SAFETY: integers and bytes, for which all-zero is valid.
        Box::new(unsafe { std::mem::zeroed() })
    }

    fn payload(&self) -> &[u8] {
        &self.data[..self.info.len as usize]
    }

    /// `size()`: the header and the payload.
    fn size(&self) -> usize {
        size_of::<DataHead>() + self.info.len as usize
    }

    /// The first `n` bytes of the struct, as upstream sends `reinterpret_cast<char *>(&event)`.
    fn bytes(&self, n: usize) -> &[u8] {
        // SAFETY: `n` is at most the struct's size.
        unsafe { std::slice::from_raw_parts(ptr::from_ref(self).cast::<u8>(), n) }
    }
}

/// `WEXITSTATUS` of a worker's exit status.
fn exit_code(status: &ExitStatus) -> c_int {
    (status.status() >> 8) & 0xff
}

/// `WTERMSIG` of a worker's exit status.
fn term_signal(status: &ExitStatus) -> c_int {
    status.status() & 0x7f
}

/// A task's payload: the bytes after its head.
fn payload(task: Packed<'_>) -> &[u8] {
    &task.bytes()[size_of::<DataHead>()..]
}

/// An `onWorkerMessage`'s bytes.
type Message = [u8; SW_IPC_MAX_SIZE as usize];
type Handler = extern "C" fn(c_int);

/// `swoole::ProcessPool`'s fields (crates/process's PoolRecord).
#[repr(C)]
struct Pool {
    running: bool,
    reload_init: bool,
    read_message: bool,
    started: bool,
    schedule_by_sysvmsg: bool,
    is_async: bool,
    ipc_mode: u8,
    protocol_type: c_int,
    master_pid: c_int,
    max_wait_time: u32,
    reload_count: u64,
    reload_last_time: i64,
    kind: u8,
    start_id: u16,
    use_msgqueue: u8,
    use_socket: u8,
    max_packet_size: u32,
    msgqueue_key: c_int,
    worker_num: u32,
    max_request: u32,
    max_request_grace: u32,
    scheduler_warning: u8,
    warning_time: i64,
    on_start: Option<PoolFn>,
    on_shutdown: Option<PoolFn>,
    on_before_reload: Option<PoolFn>,
    on_after_reload: Option<PoolFn>,
    on_task: Option<TaskFn>,
    on_worker_start: Option<WorkerFn>,
    on_message: Option<MessageFn>,
    on_worker_exit: Option<WorkerFn>,
    on_worker_stop: Option<WorkerFn>,
    on_worker_error: Option<ErrorFn>,
    on_worker_message: Option<EventFn>,
    on_worker_not_found: Option<NotFoundFn>,
    main_loop: Option<MainLoopFn>,
    round_id: u32,
    workers: *mut Worker,
    pipes: *mut c_void,
    registry: *mut c_void,
    queue: *mut c_void,
    ipc: *mut c_void,
    message_box: *mut c_void,
    message_bus: *mut c_void,
    reload_task: *mut c_void,
    ptr: *mut c_void,
    /// `kill_timer`, an `Option<ArmedTimer>`: its timer generation, id and delete function.
    kill_timer: [usize; 3],
    /// `ptr_key`, the key `ptr` is held under.
    ptr_key: *const c_void,
}

/// `swoole::Worker`'s fields (crates/process's WorkerRecord).
#[repr(C)]
struct Worker {
    pid: c_int,
    id: u32,
    pool: *mut c_void,
    queue: *mut c_void,
    shared: bool,
    redirect_stdout: bool,
    redirect_stdin: bool,
    redirect_stderr: bool,
    status: u8,
    kind: u8,
    msgqueue_mode: u8,
    child_process: u8,
    concurrency: u32,
    start_time: i64,
    dispatch_count: i64,
    request_count: i64,
    response_count: i64,
    coroutine_num: usize,
    lock: *mut c_void,
    pipe_object: *mut c_void,
    pipe_master: *mut c_void,
    pipe_worker: *mut c_void,
    pipe_current: *mut c_void,
    ptr: *mut c_void,
}

unsafe extern "C" {
    static swr_case_process_layout: [usize; 6];

    #[link_name = "_Z17swoole_signal_setiPFviEb"]
    fn swoole_signal_set(signo: c_int, handler: Handler, safety: bool) -> *mut c_void;
    #[link_name = "_Z23swoole_signal_block_allv"]
    fn swoole_signal_block_all();
    #[link_name = "_Z20swoole_system_randomii"]
    fn swoole_system_random(min: c_int, max: c_int) -> c_int;
    #[link_name = "_Z17swoole_random_intv"]
    fn swoole_random_int() -> u64;
    #[link_name = "_Z18swoole_thread_initb"]
    fn swoole_thread_init(main_thread: bool);

    fn fork() -> c_int;
    fn kill(pid: c_int, sig: c_int) -> c_int;
    fn getpid() -> c_int;
    fn usleep(usec: c_uint) -> c_int;
    fn sleep(sec: c_uint) -> c_uint;
    fn waitpid(pid: c_int, status: *mut c_int, options: c_int) -> c_int;
    fn exit(code: c_int) -> !;
    fn _exit(code: c_int) -> !;
    fn signal(signo: c_int, handler: usize) -> usize;
    fn sysv_signal(signo: c_int, handler: usize) -> usize;
    fn msgget(key: c_int, flags: c_int) -> c_int;
    fn shutdown(fd: c_int, how: c_int) -> c_int;
    fn __libc_current_sigrtmin() -> c_int;
}

// The `ProcessPool` and `Worker` methods: crates/process's Rust functions over the mirrors,
// whose layout the fixture checks against the records'.
unsafe fn pool_create(pool: *mut Pool, worker_num: c_uint, key: c_int, ipc_mode: c_int) -> c_int {
    // SAFETY: as the caller guarantees.
    let created = unsafe {
        pm::create(
            swoole_rs_server::process_pool_native(),
            swoole_rs_process::process_pool::PoolRef::new(pool.cast()),
            worker_num,
            key,
            ipc_mode,
        )
    };
    if created { SW_OK } else { SW_ERR }
}

unsafe fn pool_listen_host(
    pool: *const Pool,
    host: *const c_char,
    port: c_int,
    backlog: c_int,
) -> c_int {
    // SAFETY: as the caller guarantees; 1 is SW_SOCK_TCP.
    let listened = unsafe {
        pm::listen_socket(
            swoole_rs_process::process_pool::PoolRef::new(pool.cast_mut().cast()),
            1,
            std::ffi::CStr::from_ptr(host),
            port,
            backlog,
        )
    };
    if listened { SW_OK } else { SW_ERR }
}

unsafe fn pool_listen_file(pool: *const Pool, file: *const c_char, backlog: c_int) -> c_int {
    // SAFETY: as the caller guarantees.
    let listened = unsafe {
        pm::listen_unix(
            swoole_rs_process::process_pool::PoolRef::new(pool.cast_mut().cast()),
            std::ffi::CStr::from_ptr(file),
            backlog,
        )
    };
    if listened { SW_OK } else { SW_ERR }
}

unsafe fn pool_set_protocol(pool: *mut Pool, protocol: Protocol) {
    // SAFETY: as the caller guarantees.
    unsafe {
        pm::set_protocol(
            swoole_rs_server::process_pool_native(),
            swoole_rs_process::process_pool::PoolRef::new(pool.cast()),
            protocol,
        )
    }
}

unsafe fn pool_dispatch_sync(
    pool: *mut Pool,
    data: *mut EventData,
    worker_id: *mut c_int,
) -> c_int {
    // SAFETY: as the caller guarantees: `data` is a whole `EventData`.
    let bytes = unsafe { std::slice::from_raw_parts(data.cast::<u8>(), size_of::<EventData>()) };
    let Some(task) = swoole_rs_process::task_file::Packed::from_bytes(bytes) else {
        return SW_ERR;
    };
    // SAFETY: as the caller guarantees.
    let sent = unsafe {
        pm::dispatch(
            swoole_rs_server::process_pool_native(),
            swoole_rs_process::process_pool::PoolRef::new(pool.cast()),
            task,
            u32::try_from(*worker_id).ok(),
            true,
        )
    };
    if sent { SW_OK } else { SW_ERR }
}

unsafe fn pool_start(pool: *mut Pool) -> c_int {
    // SAFETY: as the caller guarantees.
    let started = unsafe {
        pm::start(
            swoole_rs_server::process_pool_native(),
            swoole_rs_process::process_pool::PoolRef::new(pool.cast()),
        )
    };
    if started { SW_OK } else { SW_ERR }
}

unsafe fn pool_wait(pool: *mut Pool) -> c_int {
    // SAFETY: as the caller guarantees.
    unsafe {
        pm::wait(
            swoole_rs_server::process_pool_native(),
            swoole_rs_process::process_pool::PoolRef::new(pool.cast()),
        )
    };
    SW_OK
}

unsafe fn pool_destroy(pool: *mut Pool) {
    // SAFETY: as the caller guarantees.
    unsafe {
        pm::destroy(
            swoole_rs_server::process_pool_native(),
            swoole_rs_process::process_pool::PoolRef::new(pool.cast()),
        )
    }
}

unsafe fn pool_shutdown(pool: *mut Pool) -> bool {
    // SAFETY: as the caller guarantees.
    unsafe {
        pm::shutdown(
            swoole_rs_server::process_pool_native(),
            swoole_rs_process::process_pool::PoolRef::new(pool.cast()),
        )
    }
}

unsafe fn pool_reload(pool: *mut Pool) -> bool {
    // SAFETY: as the caller guarantees.
    unsafe {
        pm::reload(
            swoole_rs_server::process_pool_native(),
            swoole_rs_process::process_pool::PoolRef::new(pool.cast()),
        )
    }
}

unsafe fn pool_detach(pool: *mut Pool) -> bool {
    // SAFETY: as the caller guarantees.
    unsafe {
        pm::detach(
            swoole_rs_server::process_pool_native(),
            swoole_rs_process::process_pool::PoolRef::new(pool.cast()),
        )
    }
}

unsafe fn pool_reopen_logger(pool: *mut Pool) {
    // SAFETY: as the caller guarantees.
    unsafe {
        pm::reopen_logger(
            swoole_rs_server::process_pool_native(),
            swoole_rs_process::process_pool::PoolRef::new(pool.cast()),
        )
    }
}

unsafe fn pool_stop(pool: *mut Pool, worker: *mut Worker) {
    // SAFETY: as the caller guarantees.
    unsafe {
        pm::stop(
            swoole_rs_server::process_pool_native(),
            swoole_rs_process::process_pool::PoolRef::new(pool.cast()),
            worker.cast(),
        )
    }
}

unsafe fn pool_create_message_bus(pool: *mut Pool) -> c_int {
    // SAFETY: as the caller guarantees.
    let created = unsafe {
        pm::create_message_bus(
            swoole_rs_server::process_pool_native(),
            swoole_rs_process::process_pool::PoolRef::new(pool.cast()),
        )
    };
    if created { SW_OK } else { SW_ERR }
}

unsafe fn pool_push_message(
    pool: *const Pool,
    kind: u8,
    data: *const c_void,
    length: usize,
) -> c_int {
    // SAFETY: as the caller guarantees.
    let pushed = unsafe {
        pm::push_message(
            swoole_rs_server::process_pool_native(),
            swoole_rs_process::process_pool::PoolRef::new(pool.cast_mut().cast()),
            kind,
            std::slice::from_raw_parts(data.cast(), length),
        )
    };
    if pushed { SW_OK } else { SW_ERR }
}

unsafe fn pool_send_message(
    pool: *const Pool,
    worker_id: u32,
    message: *const c_char,
    len: usize,
) -> bool {
    // SAFETY: as the caller guarantees.
    unsafe {
        pm::send_message(
            swoole_rs_server::process_pool_native(),
            swoole_rs_process::process_pool::PoolRef::new(pool.cast_mut().cast()),
            worker_id,
            std::slice::from_raw_parts(message.cast(), len),
        )
    }
}

unsafe fn pool_response(pool: *const Pool, data: *const c_char, length: u32) -> c_int {
    let data = if data.is_null() {
        &[]
    } else {
        // SAFETY: as the caller guarantees.
        unsafe { std::slice::from_raw_parts(data.cast(), length as usize) }
    };
    // SAFETY: as the caller guarantees.
    let written = pm::response(
        unsafe { swoole_rs_process::process_pool::PoolRef::new(pool.cast_mut().cast()) },
        data,
    );
    if written { SW_OK } else { SW_ERR }
}

unsafe fn pool_add_worker(pool: *const Pool, worker: *mut Worker) {
    // SAFETY: as the caller guarantees: a live pool and worker.
    pm::add_worker(unsafe { &*pool.cast() }, unsafe {
        WorkerHandle::from(&*(worker.cast()))
    });
}

unsafe fn pool_del_worker(pool: *const Pool, worker: *const Worker) -> bool {
    // SAFETY: as the caller guarantees: a live pool and worker.
    pm::del_worker(unsafe { &*pool.cast() }, unsafe {
        WorkerHandle::from(&*(worker.cast()))
    })
}

unsafe fn pool_get_worker_by_pid(pool: *const Pool, pid: c_int) -> *mut Worker {
    // SAFETY: as the caller guarantees: a live pool.
    pm::get_worker_by_pid(unsafe { &*pool.cast() }, pid).cast()
}

unsafe fn worker_init(worker: *mut Worker) {
    // SAFETY: as the caller guarantees: a live worker.
    swr_v1_worker_init(unsafe { WorkerHandle::from(&*(worker.cast())) });
}

unsafe fn worker_send_pipe_message(
    worker: *const Worker,
    data: *const u8,
    n: usize,
    flags: c_int,
) -> Option<usize> {
    // SAFETY: as the caller guarantees: a live worker and `n` bytes at `data`.
    let (worker, data) = unsafe {
        (
            WorkerHandle::from(&*(worker.cast())),
            std::slice::from_raw_parts(data, n),
        )
    };
    pm::send_pipe_message(swoole_rs_server::process_pool_native(), worker, data, flags)
}

unsafe fn worker_shutdown(worker: *mut Worker) {
    // SAFETY: as the caller guarantees: a live worker.
    swr_v1_worker_shutdown(unsafe { WorkerHandle::from(&*(worker.cast())) });
}

static CURRENT_POOL: AtomicPtr<Pool> = AtomicPtr::new(null_mut());
static CURRENT_WORKER: AtomicPtr<Worker> = AtomicPtr::new(null_mut());
static COUNTERS: AtomicPtr<AtomicI32> = AtomicPtr::new(null_mut());

/// The fd of a pool's stream listener, or -1 without one.
fn listener_fd(listener: Option<&swoole_rs_process::pool::Connection>) -> c_int {
    listener
        .and_then(|listener| listener.downcast_ref::<swoole_rs_reactor::OwnedSocket>())
        .map_or(-1, |listener| listener.socket().fd())
}

fn current_pool() -> *mut Pool {
    CURRENT_POOL.load(SeqCst)
}

/// `ProcessPool pool{}`.
fn new_pool() -> *mut Pool {
    // SAFETY: integers, bools, raw pointers and nullable function pointers, for which
    // all-zero is valid. Leaked: workers and timers keep the pointer.
    Box::into_raw(Box::new(unsafe { std::mem::zeroed::<Pool>() }))
}

/// `Worker worker{}`.
fn new_worker() -> Box<Worker> {
    // SAFETY: as in `new_pool`.
    Box::new(unsafe { std::mem::zeroed() })
}

fn get_worker(pool: *mut Pool, id: u32) -> *mut Worker {
    // SAFETY: `get_worker`: an index into the created pool's workers.
    unsafe {
        (*pool)
            .workers
            .add((id - u32::from((*pool).start_id)) as usize)
    }
}

/// The `String` the case keeps in `pool->ptr`.
fn pool_data<'a>(pool: *mut Pool) -> &'a [u8] {
    // SAFETY: the case stored a live Vec<u8> there.
    unsafe { &*(*pool).ptr.cast::<Vec<u8>>() }
}

fn messages<'a>(pool: *mut Pool) -> &'a mut Vec<Vec<u8>> {
    // SAFETY: the case stored a live Vec<Vec<u8>> there.
    unsafe { &mut *(*pool).ptr.cast::<Vec<Vec<u8>>>() }
}

fn create(pool: *mut Pool, worker_num: c_uint, key: c_int, ipc_mode: c_int) -> c_int {
    // SAFETY: a zeroed pool.
    unsafe { pool_create(pool, worker_num, key, ipc_mode) }
}

fn listen_host(pool: *mut Pool, host: &CStr, port: c_int) -> c_int {
    // SAFETY: a created pool.
    unsafe { pool_listen_host(pool, host.as_ptr(), port, 128) }
}

fn listen_file(pool: *mut Pool, file: &CStr) -> c_int {
    // SAFETY: a created pool.
    unsafe { pool_listen_file(pool, file.as_ptr(), 128) }
}

fn start(pool: *mut Pool) -> c_int {
    // SAFETY: a configured pool.
    unsafe { pool_start(pool) }
}

fn wait(pool: *mut Pool) -> c_int {
    // SAFETY: a started pool.
    unsafe { pool_wait(pool) }
}

fn destroy(pool: *mut Pool) {
    // SAFETY: a created pool, destroyed once.
    unsafe { pool_destroy(pool) };
}

fn create_message_bus(pool: *mut Pool) -> c_int {
    // SAFETY: a created pool.
    unsafe { pool_create_message_bus(pool) }
}

fn set_protocol(pool: *mut Pool, protocol: Protocol) {
    // SAFETY: a created pool.
    unsafe { pool_set_protocol(pool, protocol) };
}

fn dispatch_sync(pool: *mut Pool, data: &mut EventData) -> c_int {
    let mut worker_id = -1;
    // SAFETY: a created pool and a live event.
    unsafe { pool_dispatch_sync(pool, data, &raw mut worker_id) }
}

fn response(pool: *mut Pool, data: *const u8, length: u32) -> c_int {
    // SAFETY: `data` is null or holds `length` bytes.
    unsafe { pool_response(pool, data.cast(), length) }
}

fn main_loop(pool: *mut Pool, worker: *mut Worker) -> c_int {
    // SAFETY: a created pool, which set the loop, and one of its workers.
    let (pool, worker) = unsafe {
        (
            PoolRef::new(pool.cast()),
            WorkerHandle::from(&*(worker.cast())),
        )
    };
    pool.call_main_loop(worker).unwrap()
}

/// `pool.onX = fn` for each hook `hooks` names.
fn install(pool: *mut Pool, hooks: Hooks) {
    // SAFETY: the case's pool.
    unsafe { PoolRef::new(pool.cast()) }.install(&hooks);
}

fn sysv(signo: c_int, handler: usize) {
    // SAFETY: SIG_DFL, SIG_IGN or an extern "C" handler.
    unsafe { sysv_signal(signo, handler) };
}

fn handler(f: Handler) -> usize {
    f as usize
}

/// `test::counter_init()`: core-tests' main allocates the counters from `sw_mem_pool()`.
fn counter_init() {
    let mut counters = COUNTERS.load(SeqCst);
    if counters.is_null() {
        counters = mem_pool::alloc((size_of::<c_int>() * TEST_COUNTER_NUM) as u32).cast();
        COUNTERS.store(counters, SeqCst);
    }
    for index in 0..TEST_COUNTER_NUM {
        counter_set(index, 0);
    }
}

fn counter(index: usize) -> &'static AtomicI32 {
    assert!(index < TEST_COUNTER_NUM);
    // SAFETY: `counter_init` allocated TEST_COUNTER_NUM ints.
    unsafe { &*COUNTERS.load(SeqCst).add(index) }
}

fn counter_incr(index: usize) -> c_int {
    counter(index).fetch_add(1, SeqCst) + 1
}

fn counter_get(index: usize) -> c_int {
    counter(index).load(SeqCst)
}

fn counter_set(index: usize, value: c_int) {
    counter(index).store(value, SeqCst);
}

fn microtime() -> f64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_secs_f64()
}

fn ms_sleep(usec: c_uint) {
    // SAFETY: usleep(3).
    unsafe { usleep(usec) };
}

/// `test::spawn_exec(fn)`: fork, run `fn` in the child, `exit(0)`.
fn spawn_exec(f: impl FnOnce()) -> c_int {
    // SAFETY: fork(2); the child runs `f` and exits.
    let pid = unsafe { fork() };
    assert!(pid >= 0);
    if pid == 0 {
        f();
        // SAFETY: the child's end.
        unsafe { exit(0) };
    }
    pid
}

/// `test::wait_all_child_processes()`.
fn wait_all_child_processes() {
    loop {
        let mut status = 0;
        // SAFETY: a writable status.
        if unsafe { waitpid(-1, &raw mut status, 0) } < 0 {
            assert_eq!(std::io::Error::last_os_error().raw_os_error(), Some(ECHILD));
            break;
        }
    }
}

/// A `String rmem(size)` holding `size` random bytes (`append_random_bytes`).
fn random_string(size: usize) -> Vec<u8> {
    let mut data = vec![0u8; size];
    assert_eq!(random_bytes(&mut data), size);
    data
}

/// 2048 bytes: 2047 random ones and a NUL, as `listen` and `listen_unixsock` build them.
fn random_c_string() -> Vec<u8> {
    let mut data = random_string(2047);
    data.push(0);
    data
}

fn test_func(pool: *mut Pool) {
    let mut data = EventData::new();
    // SAFETY: swoole_system_random.
    let size = unsafe { swoole_system_random(1024, 4096.min(SW_IPC_BUFFER_SIZE as c_int)) };
    let size = usize::try_from(size).unwrap();
    let mut rmem = random_string(size);

    data.info.len = size as u32;
    data.data[..size].copy_from_slice(&rmem);

    assert_eq!(dispatch_sync(pool, &mut data), SW_OK);

    // SAFETY: the case's pool; `rmem` outlives the loop.
    unsafe {
        worker_init(get_worker(pool, 0));
        (*pool).running = true;
        (*pool).ptr = (&raw mut rmem).cast();
        PoolRef::new(pool.cast())
            .call_on_worker_start(WorkerHandle::from(&*(get_worker(pool, 0).cast())));
    }
    main_loop(pool, get_worker(pool, 0));
    destroy(pool);
}

fn task_protocol_on_task(pool: PoolRef<'_>, _: WorkerHandle<'_>, task: Packed<'_>) -> c_int {
    let pool = pool.raw().cast::<Pool>();
    // SAFETY: the running pool.
    unsafe { (*pool).running = false };
    ms_sleep(10000);
    assert_eq!(pool_data(pool), payload(task));
    0
}

fn test_func_task_protocol(pool: *mut Pool) {
    set_protocol(pool, Protocol::Task);
    // SAFETY: the case's pool.
    install(
        pool,
        Hooks {
            on_task: Some(task_protocol_on_task),
            ..Hooks::default()
        },
    );
    test_func(pool);
}

fn message_protocol_on_message(pool: PoolRef<'_>, rdata: &RecvData<'_>) {
    let pool = pool.raw().cast::<Pool>();
    // SAFETY: the running pool.
    unsafe { (*pool).running = false };
    ms_sleep(10000);
    // SAFETY: the pool's message.
    assert_eq!(pool_data(pool), rdata.data());
}

fn test_func_message_protocol(pool: *mut Pool) {
    set_protocol(pool, Protocol::Message);
    // SAFETY: the case's pool.
    install(
        pool,
        Hooks {
            on_message: Some(message_protocol_on_message),
            ..Hooks::default()
        },
    );
    test_func(pool);
}

fn stream_protocol_on_message(pool: PoolRef<'_>, rdata: &RecvData<'_>) {
    let pool = pool.raw().cast::<Pool>();
    // SAFETY: the running pool.
    unsafe { (*pool).running = false };
    // SAFETY: the message is an EventData's bytes, which it lends for the call.
    let msg = unsafe { rdata.data().as_ptr().cast::<EventData>().read_unaligned() };
    ms_sleep(10000);
    assert_eq!(pool_data(pool), msg.payload());
}

fn test_func_stream_protocol(pool: *mut Pool) {
    set_protocol(pool, Protocol::Stream);
    // SAFETY: the case's pool.
    install(
        pool,
        Hooks {
            on_message: Some(stream_protocol_on_message),
            ..Hooks::default()
        },
    );
    test_func(pool);
}

fn tcp() {
    let pool = new_pool();
    let svr_port = TEST_PORT + 86;
    assert_eq!(create(pool, 1, 0, SW_IPC_SOCKET), SW_OK);
    assert_eq!(listen_host(pool, TEST_HOST, svr_port), SW_OK);

    test_func_task_protocol(pool);
}

fn unix_sock() {
    let pool = new_pool();
    // SAFETY: signal(2).
    unsafe { signal(SIGPIPE, SIG_IGN) };
    assert_eq!(create(pool, 1, 0, SW_IPC_UNIXSOCK), SW_OK);
    assert_eq!(listen_host(pool, TEST_HOST, TEST_PORT), SW_ERR);
    assert_eq!(last_error(), SW_ERROR_OPERATION_NOT_SUPPORT);
    assert_eq!(listen_file(pool, TEST_SOCK_FILE), SW_ERR);
    assert_eq!(last_error(), SW_ERROR_OPERATION_NOT_SUPPORT);

    test_func_task_protocol(pool);
}

fn tcp_raw_on_message(pool: PoolRef<'_>, rdata: &RecvData<'_>) {
    let pool = pool.raw().cast::<Pool>();
    // SAFETY: the running pool.
    unsafe { (*pool).running = false };
    let data = pool_data(pool);
    assert_eq!(rdata.info().len as usize, data.len());
    assert_eq!(data, rdata.data());
}

fn tcp_raw() {
    const SIZE: u32 = 2 * 1024 * 1024;
    let mut data = random_string(SIZE as usize);

    let pool = new_pool();
    let svr_port = TEST_PORT + 111;
    assert_eq!(create(pool, 1, 0, SW_IPC_SOCKET), SW_OK);
    assert_eq!(listen_host(pool, TEST_HOST, svr_port), SW_OK);
    set_protocol(pool, Protocol::Stream);
    // SAFETY: the case's pool; `data` outlives the loop.
    unsafe {
        (*pool).max_packet_size = SIZE;
        (*pool).running = true;
        (*pool).ptr = (&raw mut data).cast();
        install(
            pool,
            Hooks {
                on_message: Some(tcp_raw_on_message),
                ..Hooks::default()
            },
        );
    }

    // The sender must run alongside the receive loop because the frame may exceed the
    // available socket buffers.
    let ipc = unsafe { (*pool).ipc } as usize;
    let (bytes, length) = (data.as_ptr() as usize, data.len() as u32);
    let sender = std::thread::spawn(move || {
        // SAFETY: a thread of this process; the pool's IPC and `data` outlive the join.
        unsafe {
            swoole_thread_init(false);
            let dispatch_result = if swoole_rs_process::pool::swr_v1_process_pool_ipc_send_sync(
                (ipc as *const swoole_rs_process::pool::PoolIpc).as_ref(),
                std::slice::from_raw_parts(bytes as *const u8, length as usize),
            ) {
                SW_OK
            } else {
                SW_ERR
            };
            if dispatch_result != SW_OK {
                // On Linux, shutting the listener down makes the blocked accept() return so the
                // receive loop can exit.
                let listener = swoole_rs_process::pool::swr_v1_process_pool_ipc_listener(
                    &*(ipc as *const swoole_rs_process::pool::PoolIpc),
                );
                shutdown(listener_fd(listener), SHUT_RDWR);
            }
            swoole_thread_clean(false);
            dispatch_result
        }
    });

    // SAFETY: the created pool's worker.
    unsafe { worker_init(get_worker(pool, 0)) };
    main_loop(pool, get_worker(pool, 0));
    let dispatch_result = sender.join().unwrap();

    assert_eq!(dispatch_result, SW_OK);
    // SAFETY: the case's pool.
    assert!(!unsafe { (*pool).running });
    destroy(pool);
}

fn msgqueue() {
    let pool = new_pool();
    assert_eq!(create(pool, 1, 0x9501, SW_IPC_MSGQUEUE), SW_OK);

    test_func_task_protocol(pool);
}

fn msgqueue_2() {
    let key = 0x9501 + 154;
    // SAFETY: msgget(2).
    let msg_id = unsafe { msgget(key, IPC_CREAT) };
    assert!(msg_id >= 0);

    // spawn_exec_and_wait: upstream discards the child's status, and so does this.
    let pid = spawn_exec(|| {
        let pool = new_pool();
        // swoole_set_isolation("", "nobody", "").
        swoole_rs_corebase::ossupport::set_isolation(c"", c"nobody", c"");
        assert_eq!(create(pool, 1, key, SW_IPC_MSGQUEUE), SW_ERR);
        assert_eq!(last_error(), EACCES);
    });
    let mut status = 0;
    // SAFETY: our child.
    unsafe { waitpid(pid, &raw mut status, 0) };
}

fn message_protocol() {
    let pool = new_pool();
    assert_eq!(create(pool, 1, 0, SW_IPC_UNIXSOCK), SW_OK);

    test_func_message_protocol(pool);
}

extern "C" fn stop_current_pool(_: c_int) {
    // SAFETY: the case set the pool before the signal could arrive.
    unsafe { (*current_pool()).running = false };
}

fn with_timer_on_worker_start(pool: PoolRef<'_>, _: WorkerHandle<'_>) {
    let pool = pool.raw().cast::<Pool>();
    CURRENT_POOL.store(pool, SeqCst);
    after(50, |_| {
        // SAFETY: kill(2) on this process.
        unsafe { kill(getpid(), SIGTERM) };
    });
}

fn with_timer_on_message(pool: PoolRef<'_>, rdata: &RecvData<'_>) {
    let pool = pool.raw().cast::<Pool>();
    ms_sleep(10000);
    // SAFETY: the pool's message.
    assert_eq!(pool_data(pool), rdata.data());
}

fn message_protocol_with_timer() {
    let pool = new_pool();
    assert_eq!(create(pool, 1, 0, SW_IPC_UNIXSOCK), SW_OK);

    set_protocol(pool, Protocol::Message);

    // SAFETY: an extern "C" handler.
    unsafe { swoole_signal_set(SIGTERM, stop_current_pool, false) };

    install(
        pool,
        Hooks {
            on_worker_start: Some(with_timer_on_worker_start),
            on_message: Some(with_timer_on_message),
            ..Hooks::default()
        },
    );

    test_func(pool);
}

fn stream_protocol() {
    let pool = new_pool();
    assert_eq!(create(pool, 1, 0, SW_IPC_UNIXSOCK), SW_OK);

    test_func_stream_protocol(pool);
}

fn stream_protocol_with_msgq() {
    let pool = new_pool();
    assert_eq!(create(pool, 1, 0x9501, SW_IPC_MSGQUEUE), SW_OK);

    test_func_stream_protocol(pool);
}

fn shutdown_on_worker_start(pool: PoolRef<'_>, _: WorkerHandle<'_>) {
    let pool = pool.raw().cast::<Pool>();
    // SAFETY: the case's shared int.
    unsafe { *(*pool).ptr.cast::<c_int>() = MAGIC_NUMBER };
    ms_sleep(1);
}

fn shutdown_on_task(pool: PoolRef<'_>, _: WorkerHandle<'_>, _: Packed<'_>) -> c_int {
    let pool = pool.raw().cast::<Pool>();
    ms_sleep(1000);
    // SAFETY: kill(2) on the master.
    unsafe { kill((*pool).master_pid, SIGTERM) };
    0
}

fn dispatch_random_128(pool: PoolRef<'_>) {
    let pool = pool.raw().cast::<Pool>();
    let mut msg = EventData::new();
    msg.info.len = 128;
    // `swoole_random_string(msg.data, msg.info.len)`: 128 characters and a NUL.
    swoole_rs_corebase::swoole_random_string(&mut msg.data[..=128]);
    dispatch_sync(pool, &mut msg);
}

fn shutdown_() {
    let pool = new_pool();
    let shm_value = mem_pool::alloc(size_of::<c_int>() as u32).cast::<c_int>();
    assert_eq!(create(pool, 1, 0x9501, SW_IPC_MSGQUEUE), SW_OK);

    // init
    set_protocol(pool, Protocol::Task);
    // SAFETY: the case's pool.
    unsafe {
        (*pool).max_packet_size = 8192;
        (*pool).ptr = shm_value.cast();
        install(
            pool,
            Hooks {
                on_worker_start: Some(shutdown_on_worker_start),
                on_task: Some(shutdown_on_task),
                on_start: Some(dispatch_random_128),
                ..Hooks::default()
            },
        );
    }

    CURRENT_POOL.store(pool, SeqCst);
    sysv(SIGTERM, handler(stop_current_pool));

    assert_eq!(start(pool), SW_OK);
    assert_eq!(wait(pool), SW_OK);

    destroy(pool);

    // SAFETY: the shared int.
    assert_eq!(unsafe { *shm_value }, MAGIC_NUMBER);

    sysv(SIGTERM, SIG_DFL);
}

extern "C" fn reopen_current_logger(_: c_int) {
    // SAFETY: the worker's pool.
    unsafe { pool_reopen_logger(current_pool()) };
}

fn reload_on_worker_start(pool: PoolRef<'_>, _: WorkerHandle<'_>) {
    let pool = pool.raw().cast::<Pool>();
    counter_incr(0);
    CURRENT_POOL.store(pool, SeqCst);

    sysv(SIGTERM, SIG_IGN);
    // SAFETY: the C library's SIGRTMIN.
    sysv(
        unsafe { __libc_current_sigrtmin() },
        handler(reopen_current_logger),
    );
    sysv(SIGWINCH, handler(reopen_current_logger));

    loop {
        // SAFETY: sleep(3).
        unsafe { sleep(10000) };
    }
}

fn reload_on_start(pool: PoolRef<'_>) {
    let pool = pool.raw().cast::<Pool>();
    // SAFETY: the started pool.
    unsafe { pool_reopen_logger(pool) };
    after(50, move |_| {
        // SAFETY: kill(2) on worker 0; the C library's SIGRTMIN.
        unsafe { kill((*get_worker(pool, 0)).pid, __libc_current_sigrtmin()) };
    });
    after(100, move |_| {
        // SAFETY: the started pool.
        unsafe { pool_reload(pool) };
    });
}

fn reload_on_before_reload(_: PoolRef<'_>) {}

fn reload_on_after_reload(pool: PoolRef<'_>) {
    let pool = pool.raw().cast::<Pool>();
    after(100, move |_| {
        // SAFETY: the started pool.
        unsafe { pool_shutdown(pool) };
    });
}

fn reload_on_worker_error(_: PoolRef<'_>, _: WorkerHandle<'_>, exit_status: &ExitStatus) {
    assert_eq!(term_signal(exit_status), SIGKILL);
}

fn reload_on_worker_message(_: PoolRef<'_>, msg: &Message) {
    // SAFETY: the bytes of an EventData.
    let msg = unsafe { ptr::from_ref(msg).cast::<EventData>().read_unaligned() };
    assert_eq!(msg.info.kind, SW_WORKER_MESSAGE_STOP + 1);
    let len = msg.info.len as usize;
    assert_eq!(msg.payload(), &TEST_STR[..len]);
}

fn reload_on_worker_not_found(pool: PoolRef<'_>, exit_status: &ExitStatus) {
    let pool = pool.raw().cast::<Pool>();
    assert_eq!(exit_status.pid(), counter_get(20));
    assert_eq!(exit_code(exit_status), 123);
    // SAFETY: TEST_STR's bytes.
    let pushed = unsafe {
        pool_push_message(
            pool,
            SW_WORKER_MESSAGE_STOP + 1,
            TEST_STR.as_ptr().cast(),
            TEST_STR.len(),
        )
    };
    assert_eq!(pushed, SW_OK);
}

extern "C" fn read_current_message(_: c_int) {
    // SAFETY: the case set the pool first.
    unsafe { (*current_pool()).read_message = true };
}

fn reload() {
    let pool = new_pool();
    counter_init();
    assert_eq!(create(pool, 2, 0, SW_IPC_NONE), SW_OK);

    // SAFETY: the case's pool.
    unsafe {
        (*pool).max_packet_size = 8192;
        (*pool).max_wait_time = 1;
        install(
            pool,
            Hooks {
                on_worker_start: Some(reload_on_worker_start),
                on_start: Some(reload_on_start),
                on_before_reload: Some(reload_on_before_reload),
                on_after_reload: Some(reload_on_after_reload),
                ..Hooks::default()
            },
        );
    }

    let other_child_pid = spawn_exec(|| {
        ms_sleep(10000);
        // SAFETY: the child's end.
        unsafe { exit(123) };
    });
    counter_set(20, other_child_pid);

    install(
        pool,
        Hooks {
            on_worker_error: Some(reload_on_worker_error),
            on_worker_message: Some(reload_on_worker_message),
            on_worker_not_found: Some(reload_on_worker_not_found),
            ..Hooks::default()
        },
    );

    CURRENT_POOL.store(pool, SeqCst);
    sysv(SIGTERM, handler(stop_current_pool));
    sysv(SIGIO, handler(read_current_message));

    assert_eq!(start(pool), SW_OK);
    assert_eq!(wait(pool), SW_OK);

    destroy(pool);

    assert_eq!(counter_get(0), 4);

    sysv(SIGTERM, SIG_DFL);
}

extern "C" fn async_stop_current_worker(_: c_int) {
    counter_incr(0);
    // SAFETY: the worker's pool and record.
    unsafe { pool_stop(current_pool(), CURRENT_WORKER.load(SeqCst)) };
}

fn async_on_start(pool: PoolRef<'_>) {
    let pool = pool.raw().cast::<Pool>();
    CURRENT_POOL.store(pool, SeqCst);
    sysv(SIGTERM, handler(stop_current_pool));
}

fn async_on_worker_start(pool: PoolRef<'_>, worker: WorkerHandle<'_>) {
    let pool = pool.raw().cast::<Pool>();
    let worker = worker.raw().cast::<Worker>();
    counter_set(0, MAGIC_NUMBER);
    CURRENT_WORKER.store(worker, SeqCst);
    CURRENT_POOL.store(pool, SeqCst);
    sysv(SIGTERM, handler(stop_current_pool));

    // SAFETY: an extern "C" handler.
    unsafe { swoole_signal_set(SIGTERM, async_stop_current_worker, false) };

    ms_sleep(10);
}

fn async_on_message(pool: PoolRef<'_>, _: &RecvData<'_>) {
    let pool = pool.raw().cast::<Pool>();
    counter_incr(0);
    // SAFETY: kill(2) on the master.
    unsafe { kill((*pool).master_pid, SIGTERM) };
}

fn async_() {
    let pool = new_pool();
    assert_eq!(create(pool, 1, 0, SW_IPC_UNIXSOCK), SW_OK);

    // init
    set_protocol(pool, Protocol::Task);
    counter_init();
    // SAFETY: the case's pool.
    unsafe {
        (*pool).max_packet_size = 8192;
        (*pool).is_async = true;
        install(
            pool,
            Hooks {
                on_start: Some(async_on_start),
                on_worker_start: Some(async_on_worker_start),
                on_message: Some(async_on_message),
                ..Hooks::default()
            },
        );
    }

    assert_eq!(start(pool), SW_OK);

    let mut msg = EventData::new();
    msg.info.len = 128;
    // `swoole_random_string(msg.data, msg.info.len)`: 128 characters and a NUL.
    swoole_rs_corebase::swoole_random_string(&mut msg.data[..=128]);
    dispatch_sync(pool, &mut msg);

    assert_eq!(wait(pool), SW_OK);

    destroy(pool);

    assert_eq!(counter_get(0), MAGIC_NUMBER + 2);

    swoole_signal_clear();
    sysv(SIGTERM, SIG_DFL);
}

/// `test::counter_incr_and_put_log(0, msg)`; the log is DEBUG output, off under VERBOSE=0.
fn counter_incr_and_put_log() {
    counter_incr(0);
}

extern "C" fn async_mb_stop_worker(_: c_int) {
    counter_incr_and_put_log();
    // SAFETY: the worker's pool and its current worker.
    unsafe { pool_stop(current_pool(), current_worker()) };
}

fn async_mb_on_worker_start(pool: PoolRef<'_>, worker: WorkerHandle<'_>) {
    let pool = pool.raw().cast::<Pool>();
    let worker = worker.raw().cast::<Worker>();
    CURRENT_WORKER.store(worker, SeqCst);
    CURRENT_POOL.store(pool, SeqCst);

    counter_incr_and_put_log();

    // SAFETY: an extern "C" handler.
    unsafe { swoole_signal_set(SIGTERM, async_mb_stop_worker, false) };

    ms_sleep(10);
}

fn async_mb_on_worker_stop(pool: PoolRef<'_>, worker: WorkerHandle<'_>) {
    let pool = pool.raw().cast::<Pool>();
    let worker = worker.raw().cast::<Worker>();
    CURRENT_WORKER.store(worker, SeqCst);
    CURRENT_POOL.store(pool, SeqCst);

    counter_incr_and_put_log();
}

fn async_mb_on_worker_exit(_: PoolRef<'_>, _: WorkerHandle<'_>) {
    counter_incr_and_put_log();
}

fn send_message(pool: *mut Pool, message: &[u8]) {
    // SAFETY: the started pool; `message`'s bytes.
    unsafe { pool_send_message(pool, 0, message.as_ptr().cast(), message.len()) };
}

fn async_mb_on_start(pool: PoolRef<'_>) {
    let pool = pool.raw().cast::<Pool>();
    CURRENT_POOL.store(pool, SeqCst);
    // SAFETY: extern "C" handlers.
    unsafe {
        swoole_signal_set(SIGTERM, stop_current_pool, false);
        swoole_signal_set(SIGIO, read_current_message, false);
    }

    counter_incr_and_put_log();

    after(100, move |_| {
        send_message(pool, b"detach");

        after(100, move |_| send_message(pool, b"shutdown"));
    });
}

fn async_mb_on_shutdown(_: PoolRef<'_>) {
    counter_incr_and_put_log();
}

fn async_mb_on_message(pool: PoolRef<'_>, msg: &RecvData<'_>) {
    let pool = pool.raw().cast::<Pool>();
    // SAFETY: the pool's message.
    let req = msg.data();

    if req == b"detach" {
        counter_incr_and_put_log();
        // SAFETY: the worker's pool.
        assert!(unsafe { pool_detach(pool) });
    } else if req == b"shutdown" {
        counter_incr_and_put_log();
        // SAFETY: the worker's pool.
        unsafe { pool_shutdown(pool) };
    }
}

fn async_mb() {
    let pool = new_pool();
    assert_eq!(create(pool, 1, 0, SW_IPC_UNIXSOCK), SW_OK);
    assert_eq!(create_message_bus(pool), SW_OK);

    if swoole_timer_is_available() {
        swoole_timer_free();
    }
    swoole_signal_clear();

    // init
    set_protocol(pool, Protocol::Task);
    counter_init();
    // SAFETY: the case's pool.
    unsafe {
        (*pool).max_packet_size = 8192;
        (*pool).is_async = true;
        install(
            pool,
            Hooks {
                on_worker_start: Some(async_mb_on_worker_start),
                on_worker_stop: Some(async_mb_on_worker_stop),
                on_worker_exit: Some(async_mb_on_worker_exit),
                on_start: Some(async_mb_on_start),
                on_shutdown: Some(async_mb_on_shutdown),
                on_message: Some(async_mb_on_message),
                ..Hooks::default()
            },
        );
    }

    assert_eq!(start(pool), SW_OK);
    assert_eq!(wait(pool), SW_OK);

    destroy(pool);

    assert!(counter_get(0) >= 8);

    swoole_signal_clear();
    sysv(SIGTERM, SIG_DFL);
    sysv(SIGIO, SIG_DFL);
}

fn mb1() {
    let pool = new_pool();
    assert_eq!(create(pool, 1, 0, SW_IPC_NONE), SW_OK);
    assert_eq!(create_message_bus(pool), SW_ERR);
    assert_eq!(last_error(), SW_ERROR_OPERATION_NOT_SUPPORT);

    destroy(pool);
}

fn mb2() {
    let pool = new_pool();
    assert_eq!(create(pool, 1, 0, SW_IPC_UNIXSOCK), SW_OK);
    assert_eq!(create_message_bus(pool), SW_OK);
    assert_eq!(create_message_bus(pool), SW_ERR);
    assert_eq!(last_error(), SW_ERROR_WRONG_OPERATION);

    destroy(pool);
}

fn socket() {
    let pool = new_pool();
    assert_eq!(create(pool, 1, 0, SW_IPC_SOCKET), SW_OK);
    assert_eq!(start(pool), SW_ERR);
    assert_eq!(last_error(), SW_ERROR_WRONG_OPERATION);

    destroy(pool);
}

fn listen_on_message(pool: PoolRef<'_>, _: &RecvData<'_>) {
    let pool = pool.raw().cast::<Pool>();
    let wmem = pool_data(pool);
    assert_eq!(response(pool, wmem.as_ptr(), wmem.len() as u32), SW_OK);
    assert_eq!(response(pool, ptr::null(), 999), SW_ERR);
    assert_eq!(last_error(), SW_ERROR_INVALID_PARAMS);
    assert_eq!(response(pool, wmem.as_ptr(), 0), SW_ERR);
    assert_eq!(last_error(), SW_ERROR_INVALID_PARAMS);
}

/// The client half of `listen` and `listen_unixsock`: a length-framed request, then the
/// framed response compared with `wmem`.
fn exchange(c: &mut (impl Read + Write), rmem: &[u8], wmem: &[u8]) {
    c.write_all(&(rmem.len() as u32).to_be_bytes()).unwrap();
    c.write_all(rmem).unwrap();
    let mut buf = [0u8; 4096];

    let mut pkt_len = [0u8; 4];
    c.read_exact(&mut pkt_len).unwrap();
    let length = u32::from_be_bytes(pkt_len) as usize;
    c.read_exact(&mut buf[..length]).unwrap();

    assert_eq!(&buf[..wmem.len()], wmem);
}

fn listen() {
    let pool = new_pool();
    let port = TEST_PORT + 515;
    assert_eq!(create(pool, 1, 0, SW_IPC_SOCKET), SW_OK);
    assert_eq!(listen_host(pool, c"127.0.0.1", port), SW_OK);

    set_protocol(pool, Protocol::Stream);

    let rmem = random_c_string();
    let mut wmem = random_c_string();

    // SAFETY: the case's pool; `wmem` outlives it.
    unsafe {
        (*pool).ptr = (&raw mut wmem).cast();
        install(
            pool,
            Hooks {
                on_message: Some(listen_on_message),
                ..Hooks::default()
            },
        );
    }

    CURRENT_POOL.store(pool, SeqCst);
    sysv(SIGTERM, handler(stop_current_pool));

    assert_eq!(start(pool), SW_OK);

    let shared = pool as usize;
    let client_wmem = wmem.clone();
    let t1 = std::thread::spawn(move || {
        // SAFETY: this thread's signal mask.
        unsafe { swoole_signal_block_all() };

        let mut c = TcpStream::connect(("127.0.0.1", port as u16)).unwrap();
        exchange(&mut c, &rmem, &client_wmem);

        let pool = shared as *mut Pool;
        assert_eq!(
            response(pool, client_wmem.as_ptr(), client_wmem.len() as u32),
            SW_ERR
        );
        assert_eq!(last_error(), SW_ERROR_INVALID_PARAMS);

        drop(c);

        // SAFETY: kill(2) on this process.
        unsafe { kill(getpid(), SIGTERM) };
    });

    assert_eq!(wait(pool), SW_OK);
    destroy(pool);

    sysv(SIGTERM, SIG_DFL);

    t1.join().unwrap();
}

fn truncated_stream_on_message(pool: PoolRef<'_>, msg: &RecvData<'_>) {
    let pool = pool.raw().cast::<Pool>();
    let messages = messages(pool);
    // SAFETY: the pool's message.
    messages.push(msg.data().to_vec());
    if messages.last().unwrap() == b"sentinel" {
        // SAFETY: the running pool.
        unsafe { (*pool).running = false };
    }
}

/// `test::get_random_port()`.
fn get_random_port() -> c_int {
    // SAFETY: swoole_random_int.
    TEST_PORT + (unsafe { swoole_random_int() } % 10000) as c_int
}

fn connect(port: c_int) -> TcpStream {
    TcpStream::connect(("127.0.0.1", port as u16)).unwrap()
}

/// `SyncClient::send`: one send(2), its result.
fn send(c: &mut TcpStream, data: &[u8]) -> usize {
    c.write(data).unwrap()
}

/// Runs worker 0's loop in this process over the case's queued connections.
fn run_worker_zero(pool: *mut Pool) {
    let worker = get_worker(pool, 0);
    // SAFETY: the created pool's worker.
    unsafe {
        worker_init(worker);
        (*pool).running = true;
    }
    main_loop(pool, worker);
    destroy(pool);
}

fn listen_rejects_truncated_stream_packet() {
    let pool = new_pool();
    let port = get_random_port();
    assert_eq!(create(pool, 1, 0, SW_IPC_SOCKET), SW_OK);
    assert_eq!(listen_host(pool, c"127.0.0.1", port), SW_OK);

    set_protocol(pool, Protocol::Stream);

    let mut messages: Vec<Vec<u8>> = Vec::new();
    // SAFETY: the case's pool; `messages` outlives it.
    unsafe {
        (*pool).max_packet_size = 64;
        (*pool).ptr = (&raw mut messages).cast();
        install(
            pool,
            Hooks {
                on_message: Some(truncated_stream_on_message),
                ..Hooks::default()
            },
        );
    }

    let mut c1 = connect(port);
    let mut c2 = connect(port);
    let mut c3 = connect(port);

    let packet_len = 8u32.to_be_bytes();
    assert_eq!(send(&mut c1, &packet_len), 4);
    assert_eq!(send(&mut c1, b"message1"), 8);
    assert_eq!(send(&mut c2, &packet_len), 4);
    assert_eq!(send(&mut c2, b"xx"), 2);
    c2.shutdown(Shutdown::Write).unwrap();
    assert_eq!(send(&mut c3, &packet_len), 4);
    assert_eq!(send(&mut c3, b"sentinel"), 8);

    run_worker_zero(pool);

    assert_eq!(messages.len(), 2);
    assert_eq!(messages[0], b"message1");
    assert_eq!(messages[1], b"sentinel");
}

fn truncated_task_on_task(pool: PoolRef<'_>, _: WorkerHandle<'_>, task: Packed<'_>) -> c_int {
    let pool = pool.raw().cast::<Pool>();
    let messages = messages(pool);
    messages.push(payload(task).to_vec());
    if messages.last().unwrap() == b"sentinel" {
        // SAFETY: the running pool.
        unsafe { (*pool).running = false };
    }
    SW_OK
}

fn listen_rejects_truncated_task_packet() {
    let pool = new_pool();
    let port = get_random_port();
    assert_eq!(create(pool, 1, 0, SW_IPC_SOCKET), SW_OK);
    assert_eq!(listen_host(pool, c"127.0.0.1", port), SW_OK);
    set_protocol(pool, Protocol::Task);

    let mut messages: Vec<Vec<u8>> = Vec::new();
    // SAFETY: the case's pool; `messages` outlives it.
    unsafe {
        (*pool).ptr = (&raw mut messages).cast();
        install(
            pool,
            Hooks {
                on_task: Some(truncated_task_on_task),
                ..Hooks::default()
            },
        );
    }

    let (mut event1, mut event2, mut event3) =
        (EventData::new(), EventData::new(), EventData::new());
    event1.info.len = 8;
    event2.info.len = 8;
    event3.info.len = 8;
    event1.data[..8].copy_from_slice(b"message1");
    event2.data[..2].copy_from_slice(b"xx");
    event3.data[..8].copy_from_slice(b"sentinel");

    let mut c1 = connect(port);
    let mut c2 = connect(port);
    let mut c3 = connect(port);

    let head = size_of::<DataHead>();
    assert_eq!(send(&mut c1, &(event1.size() as u32).to_be_bytes()), 4);
    assert_eq!(send(&mut c1, event1.bytes(event1.size())), event1.size());

    assert_eq!(send(&mut c2, &(event2.size() as u32).to_be_bytes()), 4);
    assert_eq!(send(&mut c2, event2.bytes(head + 2)), head + 2);
    c2.shutdown(Shutdown::Write).unwrap();

    assert_eq!(send(&mut c3, &(event3.size() as u32).to_be_bytes()), 4);
    assert_eq!(send(&mut c3, event3.bytes(event3.size())), event3.size());

    run_worker_zero(pool);

    assert_eq!(messages.len(), 2);
    assert_eq!(messages[0], b"message1");
    assert_eq!(messages[1], b"sentinel");
}

fn malformed_on_task(pool: PoolRef<'_>, _: WorkerHandle<'_>, task: Packed<'_>) -> c_int {
    let pool = pool.raw().cast::<Pool>();
    // SAFETY: the case's counter.
    unsafe { *(*pool).ptr.cast::<usize>() += 1 };
    if payload(task) == b"x" {
        // SAFETY: the running pool.
        unsafe { (*pool).running = false };
    }
    SW_OK
}

/// `get_worker(0)->send_pipe_message(event, n, SW_PIPE_MASTER)`.
fn send_to_worker(pool: *mut Pool, event: &EventData, n: usize) -> Option<usize> {
    // SAFETY: the created pool's worker and `n` of the event's bytes.
    unsafe {
        worker_send_pipe_message(
            get_worker(pool, 0),
            event.bytes(n).as_ptr(),
            n,
            SW_PIPE_MASTER,
        )
    }
}

fn task_protocol_rejects_malformed_packets() {
    let pool = new_pool();
    assert_eq!(create(pool, 1, 0, SW_IPC_UNIXSOCK), SW_OK);
    set_protocol(pool, Protocol::Task);
    let mut dispatched = 0usize;
    // SAFETY: the case's pool and worker; `dispatched` outlives the loop.
    unsafe {
        worker_init(get_worker(pool, 0));
        (*pool).running = true;
        (*pool).ptr = (&raw mut dispatched).cast();
        install(
            pool,
            Hooks {
                on_task: Some(malformed_on_task),
                ..Hooks::default()
            },
        );
    }
    let head = size_of::<DataHead>();

    let mut task = EventData::new();
    task.info.len = u32::MAX;
    assert_eq!(send_to_worker(pool, &task, head - 1), Some(head - 1));

    task.info.len = 5000;
    assert_eq!(send_to_worker(pool, &task, 100), Some(100));

    task.info.ext_flags = SW_TASK_TMPFILE;
    task.info.len = 1;
    task.data[0] = b't';
    assert_eq!(send_to_worker(pool, &task, head + 1), Some(head + 1));

    task.info.ext_flags = 0;
    task.info.len = SW_IPC_BUFFER_SIZE as u32;
    let whole = size_of::<EventData>();
    assert_eq!(send_to_worker(pool, &task, whole), Some(whole));

    task.info.len = 1;
    task.data[0] = b'x';
    assert_eq!(send_to_worker(pool, &task, head + 1), Some(head + 1));

    main_loop(pool, get_worker(pool, 0));

    assert_eq!(dispatched, 2);
    assert_eq!(last_error(), SW_ERROR_PROTOCOL_ERROR);
    destroy(pool);
}

fn task_protocol_rejects_malformed_socket_packet() {
    const SOCKET_FILE: &CStr = c"/tmp/swoole_process_pool_task.sock";
    // SAFETY: signal(2).
    unsafe { signal(SIGPIPE, SIG_IGN) };
    let pool = new_pool();
    assert_eq!(create(pool, 1, 0, SW_IPC_SOCKET), SW_OK);
    assert_eq!(listen_file(pool, SOCKET_FILE), SW_OK);
    set_protocol(pool, Protocol::Task);
    let mut dispatched = 0usize;
    // SAFETY: the case's pool and worker; `dispatched` outlives the loop.
    unsafe {
        worker_init(get_worker(pool, 0));
        (*pool).running = true;
        (*pool).ptr = (&raw mut dispatched).cast();
        install(
            pool,
            Hooks {
                on_task: Some(malformed_on_task),
                ..Hooks::default()
            },
        );
    }

    let address = pool as usize;
    let sender = std::thread::spawn(move || {
        let pool = address as *mut Pool;
        // SAFETY: a thread of this process.
        unsafe { swoole_thread_init(false) };
        let mut acknowledgment_received = SW_ERR as isize;
        let mut acknowledgment = -1;
        let client = UnixStream::connect(SOCKET_FILE.to_str().unwrap());
        assert!(client.is_ok());
        if let Ok(mut client) = client {
            client
                .set_read_timeout(Some(std::time::Duration::from_secs(1)))
                .unwrap();
            let mut invalid_task = EventData::new();
            invalid_task.info.len = u32::MAX;
            let head = size_of::<DataHead>();
            let packet_length = ((head - 1) as u32).to_be_bytes();
            assert_eq!(client.write(&packet_length).unwrap(), size_of::<u32>());
            assert_eq!(
                client.write(invalid_task.bytes(head - 1)).unwrap(),
                head - 1
            );
            let mut ack = [0u8; 4];
            if client.read_exact(&mut ack).is_ok() {
                acknowledgment_received = ack.len() as isize;
                acknowledgment = c_int::from_ne_bytes(ack);
            }
        }

        let mut valid_task = EventData::new();
        valid_task.info.len = 1;
        valid_task.data[0] = b'x';
        let valid_packet_sent = dispatch_sync(pool, &mut valid_task);
        if valid_packet_sent != SW_OK {
            // SAFETY: the pool's stream listener, which outlives the join.
            unsafe {
                let listener = swoole_rs_process::pool::swr_v1_process_pool_ipc_listener(
                    &*(*pool).ipc.cast::<swoole_rs_process::pool::PoolIpc>(),
                );
                shutdown(listener_fd(listener), SHUT_RDWR);
            }
        }
        swoole_thread_clean(false);
        (acknowledgment_received, acknowledgment, valid_packet_sent)
    });

    main_loop(pool, get_worker(pool, 0));
    let (acknowledgment_received, acknowledgment, valid_packet_sent) = sender.join().unwrap();

    assert_eq!(acknowledgment_received, size_of::<c_int>() as isize);
    assert_eq!(acknowledgment, 0);
    assert_eq!(valid_packet_sent, SW_OK);
    assert_eq!(dispatched, 1);
    destroy(pool);
}

/// The header's inline `set_max_packet_size`.
fn set_max_packet_size(pool: *mut Pool, size: u32) {
    // SAFETY: a created pool.
    unsafe {
        (*pool).max_packet_size = if (*pool).use_msgqueue != 0 {
            SW_MSGMAX
        } else {
            size
        }
    };
}

fn small_packet_on_message(pool: PoolRef<'_>, rdata: &RecvData<'_>) {
    let pool = pool.raw().cast::<Pool>();
    assert_eq!(rdata.info().len, 1);
    assert_eq!(rdata.data()[0], b'x');
    // SAFETY: the running pool.
    unsafe { (*pool).running = false };
}

fn stream_protocol_small_packet() {
    // The one-byte allocation exposes the former QueueNode write under ASAN.
    let pool = new_pool();
    assert_eq!(create(pool, 1, 0, SW_IPC_UNIXSOCK), SW_OK);
    set_max_packet_size(pool, 1);
    set_protocol(pool, Protocol::Stream);
    // SAFETY: the case's pool and worker.
    unsafe {
        worker_init(get_worker(pool, 0));
        (*pool).running = true;
        install(
            pool,
            Hooks {
                on_message: Some(small_packet_on_message),
                ..Hooks::default()
            },
        );
    }

    // SAFETY: the created pool's worker and one byte.
    let sent =
        unsafe { worker_send_pipe_message(get_worker(pool, 0), b"x".as_ptr(), 1, SW_PIPE_MASTER) };
    assert_eq!(sent, Some(1));
    main_loop(pool, get_worker(pool, 0));
    // SAFETY: the case's pool.
    assert!(!unsafe { (*pool).running });
    destroy(pool);
}

fn msgqueue_package_size_on_message(pool: PoolRef<'_>, rdata: &RecvData<'_>) {
    let pool = pool.raw().cast::<Pool>();
    messages(pool).push(rdata.data().to_vec());
    if rdata.info().len == 0 {
        // SAFETY: the running pool.
        unsafe { (*pool).running = false };
    }
}

fn stream_protocol_msgqueue_ignores_package_size() {
    let sent: Vec<Vec<u8>> = vec![
        vec![b'a'; 1025],
        vec![b'b'; 1024],
        b"x".to_vec(),
        Vec::new(),
    ];
    for limit in [0, 1, 1024, SW_MSGMAX + 1] {
        let pool = new_pool();
        assert_eq!(
            create(pool, 1, IPC_PRIVATE, SW_IPC_MSGQUEUE),
            SW_OK,
            "limit {limit}"
        );
        set_max_packet_size(pool, limit);
        set_protocol(pool, Protocol::Stream);
        let mut received: Vec<Vec<u8>> = Vec::new();
        // SAFETY: the case's pool and worker; `received` outlives the loop.
        unsafe {
            worker_init(get_worker(pool, 0));
            (*pool).running = true;
            (*pool).ptr = (&raw mut received).cast();
            install(
                pool,
                Hooks {
                    on_message: Some(msgqueue_package_size_on_message),
                    ..Hooks::default()
                },
            );
        }

        // `QueueNode message{}`: its mtype, then SW_MSGMAX bytes.
        let mut message = vec![0u8; size_of::<c_long>() + SW_MSGMAX as usize];
        let mut mtype: c_long = 0;
        for data in &sent {
            mtype += 1;
            message[..size_of::<c_long>()].copy_from_slice(&mtype.to_ne_bytes());
            message[size_of::<c_long>()..][..data.len()].copy_from_slice(data);
            // SAFETY: the pool's queue, which `create` opened.
            let queue = unsafe {
                &*(*pool)
                    .queue
                    .cast::<swoole_rs_corebase::ossupport::QueueRecord>()
            };
            let pushed = queue.push(&message[..size_of::<c_long>() + data.len()]);
            assert!(pushed, "limit {limit}");
        }
        main_loop(pool, get_worker(pool, 0));

        assert_eq!(received, sent, "limit {limit}");
        destroy(pool);
    }
}

const TEST_SOCK: &CStr = c"/tmp/swoole_process_pool.sock";

fn listen_unixsock_on_message(pool: PoolRef<'_>, _: &RecvData<'_>) {
    let pool = pool.raw().cast::<Pool>();
    let wmem = pool_data(pool);
    response(pool, wmem.as_ptr(), wmem.len() as u32);
}

fn listen_unixsock() {
    let pool = new_pool();
    assert_eq!(create(pool, 1, 0, SW_IPC_SOCKET), SW_OK);
    assert_eq!(listen_file(pool, TEST_SOCK), SW_OK);

    set_protocol(pool, Protocol::Stream);

    let rmem = random_c_string();
    let mut wmem = random_c_string();

    // SAFETY: the case's pool; `wmem` outlives it.
    unsafe {
        (*pool).ptr = (&raw mut wmem).cast();
        install(
            pool,
            Hooks {
                on_message: Some(listen_unixsock_on_message),
                ..Hooks::default()
            },
        );
    }

    CURRENT_POOL.store(pool, SeqCst);
    sysv(SIGTERM, handler(stop_current_pool));

    assert_eq!(start(pool), SW_OK);

    let client_wmem = wmem.clone();
    let t1 = std::thread::spawn(move || {
        // SAFETY: this thread's signal mask.
        unsafe { swoole_signal_block_all() };

        let mut c = UnixStream::connect(TEST_SOCK.to_str().unwrap()).unwrap();
        exchange(&mut c, &rmem, &client_wmem);
        drop(c);

        // SAFETY: kill(2) on this process.
        unsafe { kill(getpid(), SIGTERM) };
    });

    assert_eq!(wait(pool), SW_OK);

    destroy(pool);

    sysv(SIGTERM, SIG_DFL);

    t1.join().unwrap();
}

fn worker_status(worker: &Worker) -> u8 {
    worker.status
}

fn set_worker_type(kind: c_int) -> u8 {
    swoole_set_worker_type(kind);
    u8::from_ne_bytes(swoole_get_worker_symbol().to_ne_bytes())
}

fn worker() {
    let mut worker = new_worker();
    // SAFETY: a zeroed record.
    unsafe { worker_init(&raw mut *worker) };

    assert!(pm::worker_is_running());
    assert!(worker.start_time > 0);
    pm::worker_set_max_request(1000, 200);

    let max_request = swoole_rs_process::worker_globals::with_globals(|globals| {
        globals
            .max_request
            .load(std::sync::atomic::Ordering::Relaxed)
    });
    assert!(max_request > 1000);
    assert!(max_request <= 1200);

    // SAFETY: the record.
    unsafe { worker_shutdown(&raw mut *worker) };
    assert!(pm::worker_is_shutdown());

    assert_eq!(set_worker_type(SW_USER_WORKER), b'@');
    assert_eq!(set_worker_type(SW_TASK_WORKER), b'^');
    assert_eq!(set_worker_type(SW_WORKER), b'*');
    assert_eq!(set_worker_type(SW_MASTER), b'#');
    assert_eq!(set_worker_type(SW_MANAGER), b'$');

    worker.status = SW_WORKER_IDLE;
    assert_eq!(worker_status(&worker), SW_WORKER_IDLE);
    assert_ne!(worker_status(&worker), SW_WORKER_BUSY);

    worker.status = SW_WORKER_BUSY;
    assert_ne!(worker_status(&worker), SW_WORKER_IDLE);
    assert_eq!(worker_status(&worker), SW_WORKER_BUSY);

    worker.status = SW_WORKER_EXIT;
    assert_ne!(worker_status(&worker), SW_WORKER_IDLE);
    assert_ne!(worker_status(&worker), SW_WORKER_BUSY);
}

fn add_worker() {
    let mut worker = new_worker();
    // SAFETY: getpid(2).
    worker.pid = unsafe { getpid() };

    let pool = new_pool();
    assert_eq!(create(pool, 1, 0, SW_IPC_UNIXSOCK), SW_OK);

    // SAFETY: a created pool and a live record.
    unsafe { pool_add_worker(pool, &raw mut *worker) };

    // SAFETY: as above.
    let worker2 = unsafe { pool_get_worker_by_pid(pool, getpid()) };
    assert_eq!(ptr::from_mut(&mut *worker), worker2);

    // SAFETY: as above.
    assert!(unsafe { pool_del_worker(pool, worker2) });
}

extern "C" fn shutdown_current_worker(_: c_int) {
    // SAFETY: this worker process's record.
    unsafe { worker_shutdown(current_worker()) };
}

fn child_exit_on_worker_start(_: PoolRef<'_>, worker: WorkerHandle<'_>) {
    let worker = worker.raw().cast::<Worker>();
    // SAFETY: an extern "C" handler.
    unsafe { swoole_signal_set(SIGTERM, shutdown_current_worker, false) };
    // SAFETY: the worker's record.
    counter_incr(unsafe { (*worker).id } as usize);
    while !pm::worker_is_shutdown() {
        ms_sleep(1000);
    }
}

fn child_exit_on_worker_not_found(pool: PoolRef<'_>, status: &ExitStatus) {
    let pool = pool.raw().cast::<Pool>();
    // SAFETY: the started pool.
    if status.pid() != counter_get(6) || counter_incr(27) != 1 || !unsafe { pool_reload(pool) } {
        counter_incr(26);
    }
}

fn child_exit_on_before_reload(_: PoolRef<'_>) {
    counter_incr(24);
}

fn child_exit_on_after_reload(pool: PoolRef<'_>) {
    let pool = pool.raw().cast::<Pool>();
    // SAFETY: the started pool's worker 1.
    if unsafe { (*get_worker(pool, 1)).pid } == counter_get(5) {
        counter_incr(26);
    }
    counter_incr(25);
}

fn reload_from_child_exit_callback() {
    let pool = new_pool();
    counter_init();
    assert_eq!(create(pool, 2, 0, SW_IPC_NONE), SW_OK);
    // SAFETY: the case's pool.
    unsafe { (*pool).max_wait_time = 1 };
    // SAFETY: the case's pool.
    unsafe { PoolRef::new(pool.cast()) }.clear_main_loop();
    install(
        pool,
        Hooks {
            on_worker_start: Some(child_exit_on_worker_start),
            on_worker_not_found: Some(child_exit_on_worker_not_found),
            on_before_reload: Some(child_exit_on_before_reload),
            on_after_reload: Some(child_exit_on_after_reload),
            ..Hooks::default()
        },
    );
    let watchdog = after(5000, move |_| {
        counter_set(31, 1);
        // SAFETY: the case's pool.
        unsafe { (*pool).running = false };
    });
    let completion = tick(10, move |tnode| {
        if counter_get(25) == 1 && counter_get(0) == 2 && counter_get(1) == 2 {
            swoole_rs_timer::del_id(tnode);
            // SAFETY: the case's pool.
            unsafe { (*pool).running = false };
        }
    });
    assert_eq!(start(pool), SW_OK);
    // SAFETY: the started pool's worker 1.
    counter_set(5, unsafe { (*get_worker(pool, 1)).pid });
    let deadline = microtime() + 3.0;
    while (counter_get(0) != 1 || counter_get(1) != 1) && microtime() < deadline {
        ms_sleep(1000);
    }
    // Reaping an unrelated child invokes this callback after reload initialization in wait().
    // SAFETY: fork(2); the child exits at once.
    let child_pid = unsafe { fork() };
    assert!(child_pid >= 0);
    if child_pid == 0 {
        // SAFETY: the child's end.
        unsafe { _exit(0) };
    }
    counter_set(6, child_pid);
    assert_eq!(wait(pool), SW_OK);
    wait_all_child_processes();
    _ = swoole_timer_clear(watchdog);
    if counter_get(31) != 0 {
        _ = swoole_timer_clear(completion);
    }
    assert_eq!(counter_get(31), 0);
    assert_eq!(counter_get(26), 0);
    assert_eq!(counter_get(24), 1);
    assert_eq!(counter_get(25), 1);
    assert_eq!(counter_get(27), 1);
    assert_eq!(counter_get(0), 2);
    assert_eq!(counter_get(1), 2);
    // SAFETY: once.
    unsafe { pool_destroy(pool) };
    swoole_timer_free();
    swoole_signal_clear();
}

/// `SwooleWG.worker`, as the mirror.
fn current_worker() -> *mut Worker {
    swoole_rs_process::worker_globals::current_worker().cast()
}

/// The mirrors' layout, checked against the fixture before any case runs.
pub(super) fn check_layout() {
    let mirrored = [
        size_of::<Pool>(),
        std::mem::offset_of!(Pool, main_loop),
        std::mem::offset_of!(Pool, ptr),
        size_of::<Worker>(),
        std::mem::offset_of!(Worker, start_time),
        size_of::<EventData>(),
    ];
    // SAFETY: a constant table in the fixture.
    assert_eq!(mirrored, unsafe {
        ptr::read(&raw const swr_case_process_layout)
    });
}

pub(super) const CASES: super::Cases = &[
    ("tcp", tcp),
    ("unix_sock", unix_sock),
    (
        "task_protocol_rejects_malformed_packets",
        task_protocol_rejects_malformed_packets,
    ),
    (
        "task_protocol_rejects_malformed_socket_packet",
        task_protocol_rejects_malformed_socket_packet,
    ),
    ("tcp_raw", tcp_raw),
    ("msgqueue", msgqueue),
    ("msgqueue_2", msgqueue_2),
    ("message_protocol", message_protocol),
    ("message_protocol_with_timer", message_protocol_with_timer),
    ("stream_protocol", stream_protocol),
    ("stream_protocol_small_packet", stream_protocol_small_packet),
    (
        "stream_protocol_msgqueue_ignores_package_size",
        stream_protocol_msgqueue_ignores_package_size,
    ),
    ("stream_protocol_with_msgq", stream_protocol_with_msgq),
    ("shutdown", shutdown_),
    ("reload", reload),
    ("async", async_),
    ("async_mb", async_mb),
    ("mb1", mb1),
    ("mb2", mb2),
    ("socket", socket),
    ("listen", listen),
    (
        "listen_rejects_truncated_stream_packet",
        listen_rejects_truncated_stream_packet,
    ),
    (
        "listen_rejects_truncated_task_packet",
        listen_rejects_truncated_task_packet,
    ),
    ("listen_unixsock", listen_unixsock),
    ("worker", worker),
    ("add_worker", add_worker),
    (
        "reload_from_child_exit_callback",
        reload_from_child_exit_callback,
    ),
];
