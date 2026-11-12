//! Upstream's `core-tests/src/server/server.cpp` TESTs that reload, kill and restart a
//! started Server's workers, one Rust function per TEST with the same inputs and expected
//! values. The callbacks are upstream's lambdas; what they captured is a static here, and
//! `test::counter_*` is shared memory every forked process sees, as core-tests' is.

use super::*;
use crate::TEST_STR;
use case::TimerNode;
use std::sync::atomic::{AtomicI64, AtomicPtr, AtomicU16, AtomicU64};
use swoole_rs_clients::client::{
    Client, Handle, ON_CLOSE, ON_CONNECT, ON_ERROR, ON_RECEIVE, Op, listen, swr_v1_clients_new,
};
use swoole_rs_process::methods::{ExitStatus, send_pipe_message, swoole_get_worker_type};
use swoole_rs_process::worker_record::{Worker, WorkerRecord};
use swoole_rs_server::callbacks as cb;
use swoole_rs_server::{event_worker_ops, manager_ops, task_worker_ops, worker_methods};

const SW_ERROR_OPERATION_NOT_SUPPORT: c_int = 507;
const SW_ERROR_WRONG_OPERATION: c_int = 509;
const TASK_IPC_MSGQUEUE: u8 = 2;
const SW_TASK_WORKER: c_int = 4;
// glibc's values (the core-tests build is Linux).
const SIGUSR1: c_int = 10;
const SIGUSR2: c_int = 12;
const SHUT_RDWR: c_int = 2;
const PROT_READ_WRITE: c_int = 3;
const MAP_SHARED_ANONYMOUS: c_int = 0x21;
const TEST_COUNTER_NUM: usize = 32;
/// The shared slot after the counters: expectations that failed in any process.
const FAILURES: usize = TEST_COUNTER_NUM;
/// The shared slot `lock_wait` and `unlock` use.
const STARTED: usize = TEST_COUNTER_NUM + 1;
const SW_PIPE_MASTER: c_int = 1;
const SW_PIPE_NONBLOCK: c_int = 2;
const SW_SERVER_EVENT_SHUTDOWN: u8 = 16;
const SW_SERVER_EVENT_CLOSE: u8 = 4;
const SW_ERROR_SESSION_NOT_EXIST: c_int = 1005;
const SW_ERROR_SERVER_INVALID_COMMAND: c_int = 9012;
// Server's text members (SWR_SERVER_TEXT_*), after http1.rs's.
const TEXT_PID_FILE: usize = 3;

unsafe extern "C" {
    fn mmap(
        addr: *mut c_void,
        length: usize,
        prot: c_int,
        flags: c_int,
        fd: c_int,
        offset: i64,
    ) -> *mut c_void;
    fn _exit(status: c_int) -> !;
    fn __libc_current_sigrtmin() -> c_int;
    fn pipe(fds: *mut [c_int; 2]) -> c_int;
    fn read(fd: c_int, buf: *mut c_void, count: usize) -> isize;
    fn write(fd: c_int, buf: *const c_void, count: usize) -> isize;
    #[link_name = "_Z17swoole_random_intv"]
    fn swoole_random_int() -> u64;
    #[link_name = "_Z18swoole_event_deferPFvPvES_"]
    fn swoole_event_defer(call: unsafe extern "C" fn(*mut c_void), data: *mut c_void);
}

unsafe extern "C-unwind" {
    #[link_name = "_Z17swoole_timer_ticklPFvPN6swoole5TimerEPNS_9TimerNodeEEPv"]
    fn swoole_timer_tick(ms: c_long, callback: TimerFn, data: *mut c_void) -> c_long;
    #[link_name = "_Z18swoole_timer_clearl"]
    fn swoole_timer_clear(timer_id: c_long) -> bool;
    fn swr_v1_core_set_last_error(code: c_int);
}

static COUNTERS: AtomicPtr<AtomicI32> = AtomicPtr::new(ptr::null_mut());

/// `test::counter_ptr()`: main's `sw_mem_pool()->alloc(sizeof(int) * TEST_COUNTER_NUM)`,
/// shared memory, mapped by the first call, before the case's server forks.
fn counters() -> &'static [AtomicI32] {
    let mut counters = COUNTERS.load(Ordering::SeqCst);
    if counters.is_null() {
        // SAFETY: a new anonymous shared mapping.
        let mapped = unsafe {
            mmap(
                ptr::null_mut(),
                size_of::<AtomicI32>() * (TEST_COUNTER_NUM + 2),
                PROT_READ_WRITE,
                MAP_SHARED_ANONYMOUS,
                -1,
                0,
            )
        };
        assert_ne!(mapped as isize, -1, "mmap");
        counters = mapped.cast();
        COUNTERS.store(counters, Ordering::SeqCst);
    }
    // SAFETY: the mapping holds the zeroed counters, FAILURES and STARTED, never unmapped.
    unsafe { std::slice::from_raw_parts(counters, TEST_COUNTER_NUM + 2) }
}

fn counter_init() {
    for counter in counters() {
        counter.store(0, Ordering::SeqCst);
    }
}

/// `test::counter_incr(index, add)`: the new value.
fn counter_incr(index: usize, add: i32) -> i32 {
    counters()[index].fetch_add(add, Ordering::SeqCst) + add
}

fn counter_get(index: usize) -> i32 {
    counters()[index].load(Ordering::SeqCst)
}

fn counter_set(index: usize, value: i32) {
    counters()[index].store(value, Ordering::SeqCst);
}

/// `EXPECT_*` in a callback, which may run in a forked worker: also counted in the
/// shared FAILURES slot, so the case fails on it once the server stops, where gtest's
/// parent never saw a worker's.
fn expect(held: bool, what: &str) {
    super::expect(held, what);
    if !held {
        counters()[FAILURES].fetch_add(1, Ordering::SeqCst);
    }
}

/// `ASSERT_*` in a callback: as [`expect`], and the callback returns when it failed.
fn check(held: bool, what: &str) -> bool {
    expect(held, what);
    held
}

/// Fails the case on an expectation a callback recorded, in this process or a worker.
fn assert_no_failure() {
    assert!(
        !FAILED.load(Ordering::SeqCst) && counters()[FAILURES].load(Ordering::SeqCst) == 0,
        "a server-side expectation failed"
    );
}

fn worker_id(worker: *mut WorkerRecord) -> u32 {
    // SAFETY: the worker the server passed the callback.
    unsafe { Worker::new(worker) }.id()
}

fn manager_pid(serv: Server) -> c_int {
    serv.view()
        .counters()
        .manager_pid
        .load(core::sync::atomic::Ordering::Relaxed)
}

fn master_pid(serv: Server) -> c_int {
    serv.view()
        .counters()
        .master_pid
        .load(core::sync::atomic::Ordering::Relaxed)
}

fn signal(pid: c_int, signo: c_int) {
    // SAFETY: kill(2).
    unsafe { kill(pid, signo) };
}

fn pid() -> c_int {
    // SAFETY: getpid(2).
    unsafe { getpid() }
}

/// `serv->reload(reload_all_workers)`.
fn reload(serv: Server, reload_all_workers: bool) -> bool {
    manager_ops::reload(serv.view(), reload_all_workers)
}

fn shutdown(serv: Server) -> bool {
    server_methods::shutdown(serv.view())
}

/// `serv->is_task_worker()`.
fn is_task_worker() -> bool {
    swoole_get_worker_type() == SW_TASK_WORKER
}

type TimerFn = unsafe extern "C" fn(*mut c_void, *mut TimerNode);

/// `swoole_timer_after(ms, [serv](TIMER_PARAMS) {...})`.
fn timer_after(ms: c_long, callback: TimerFn, serv: Server) {
    // SAFETY: a callback of the timer's signature, over the running server.
    unsafe { case::swoole_timer_after(ms, callback, serv.0) };
}

/// `swoole_timer_tick(ms, [serv](TIMER_PARAMS) {...})`.
fn timer_tick(ms: c_long, callback: TimerFn, serv: Server) -> c_long {
    // SAFETY: as above.
    unsafe { swoole_timer_tick(ms, callback, serv.0) }
}

/// The server a timer of [`timer_after`] or [`timer_tick`] captured.
fn timer_server(tnode: *mut TimerNode) -> Server {
    // SAFETY: the node the timer passes its callback.
    Server(unsafe { (*tnode).data })
}

unsafe extern "C" fn shutdown_timer(_: *mut c_void, tnode: *mut TimerNode) {
    shutdown(timer_server(tnode));
}

unsafe extern "C" fn idle_timer(_: *mut c_void, _: *mut TimerNode) {}

/// `serv.onX = [](Server *, ...) -> int { return 0; }`, for `onTask` and `onReceive`.
unsafe extern "C" fn return_ok(_: *mut c_void, _: *mut c_void, _: *mut c_void) -> c_int {
    SW_OK
}

/// `microtime()`.
fn now() -> f64 {
    // SAFETY: reads the clock.
    unsafe { microtime() }
}

fn sleep_forever() -> ! {
    loop {
        sleep(1.0);
    }
}

/// `new Worker()`, as the server's user worker: the id `add_worker` returns is the one it sets.
fn add_user_worker(serv: Server) -> *mut WorkerRecord {
    let worker = Box::into_raw(WorkerRecord::boxed());
    // SAFETY: the server before `create`, and a live worker it keeps.
    let id = unsafe { worker_methods::add_worker(serv.view(), worker) };
    assert_eq!(id, worker_id(worker).cast_signed());
    worker
}

/// `delete worker`.
fn delete_worker(worker: *mut WorkerRecord) {
    // SAFETY: made by add_user_worker, freed once.
    drop(unsafe { Box::from_raw(worker) });
}

/// `TEST(server, create_task_worker_fail)`.
fn create_task_worker_fail() {
    let serv = Server::new(MODE_PROCESS);
    serv.config().worker_num = 1;
    serv.config().task_worker_num = 2;
    serv.config().task_enable_coroutine = true;
    serv.config().task_ipc_mode = TASK_IPC_MSGQUEUE;
    set_log_warning();

    assert!(serv.add_port(SW_SOCK_TCP, TEST_HOST, 0).is_some());
    assert_eq!(serv.create(), SW_ERR);
    assert_eq!(last_error(), SW_ERROR_WRONG_OPERATION);
    assert_no_failure();
    serv.delete();
}

unsafe extern "C" fn reload_single_timer(_: *mut c_void, tnode: *mut TimerNode) {
    let serv = timer_server(tnode);
    if !check(!reload(serv, true), "!serv->reload(true)")
        || !check(
            last_error() == SW_ERROR_OPERATION_NOT_SUPPORT,
            "ASSERT_ERREQ(SW_ERROR_OPERATION_NOT_SUPPORT)",
        )
    {
        return;
    }
    timer_after(80, shutdown_timer, serv);
}

unsafe extern "C" fn reload_single_worker_start(
    _: *mut c_void,
    serv: *mut c_void,
    worker: *mut WorkerRecord,
) {
    if worker_id(worker) == 0 {
        timer_after(50, reload_single_timer, Server(serv));
    }
    counter_incr(1, 1);
}

/// `TEST(server, reload_single_process)`.
fn reload_single_process() {
    let serv = Server::new(MODE_BASE);
    serv.config().worker_num = 1;
    counter_init();

    assert!(serv.add_port(SW_SOCK_TCP, TEST_HOST, 0).is_some());

    serv.set_callback(cb::CB_TASK as usize, return_ok as *const c_void);
    serv.set_callback(CB_WORKER_START, reload_single_worker_start as *const c_void);
    serv.set_callback(CB_RECEIVE, return_ok as *const c_void);

    assert_eq!(serv.create(), SW_OK);
    assert_eq!(serv.start(), SW_OK);

    assert_eq!(counter_get(1), 1); // onWorkerStart
    assert_no_failure();
    serv.delete();
}

unsafe extern "C" fn reload_no_task_timer(_: *mut c_void, tnode: *mut TimerNode) {
    let serv = timer_server(tnode);
    if !check(reload(serv, false), "serv->reload(false)") {
        return;
    }
    timer_after(80, shutdown_timer, serv);
}

unsafe extern "C" fn reload_no_task_worker_start(
    _: *mut c_void,
    serv: *mut c_void,
    worker: *mut WorkerRecord,
) {
    if worker_id(worker) == 0 {
        timer_after(50, reload_no_task_timer, Server(serv));
    }
    counter_incr(1, 1);
}

/// `TEST(server, reload_no_task_worker)`.
fn reload_no_task_worker() {
    let serv = Server::new(MODE_BASE);
    serv.config().worker_num = 2;
    counter_init();

    assert!(serv.add_port(SW_SOCK_TCP, TEST_HOST, 0).is_some());

    serv.set_callback(cb::CB_TASK as usize, return_ok as *const c_void);
    serv.set_callback(
        CB_WORKER_START,
        reload_no_task_worker_start as *const c_void,
    );
    serv.set_callback(CB_RECEIVE, return_ok as *const c_void);

    assert_eq!(serv.create(), SW_OK);
    assert_eq!(serv.start(), SW_OK);

    assert_eq!(counter_get(1), 2); // onWorkerStart
    assert_no_failure();
    serv.delete();
}

unsafe extern "C" fn before_reload(_: *mut c_void, _: *mut c_void) {
    counter_incr(10, 1);
}

unsafe extern "C" fn after_reload(_: *mut c_void, _: *mut c_void) {
    counter_incr(11, 1);
}

/// `reload_all_workers`' and `reload_all_workers2`'s `onWorkerStart`: worker 1 records its
/// pid in `filename` and signals `target` to reload the task workers, then all of them;
/// its replacement checks it is a new process, then stops the server.
fn reload_on_worker_start(serv: Server, worker: *mut WorkerRecord, filename: &str, target: c_int) {
    if worker_id(worker) != 1 {
        return;
    }
    if std::fs::File::open(filename).is_err() {
        std::fs::write(filename, pid().to_string()).unwrap();
        signal(target, SIGUSR2);
        sleep(1.0);
        signal(target, SIGUSR1);
    } else {
        // `char buf[10]; file >> buf; stringstream(buf) >> oldPid`.
        let old_pid = std::fs::read_to_string(filename)
            .ok()
            .and_then(|text| text.split_whitespace().next()?.parse::<c_int>().ok())
            .unwrap_or(0);
        expect(old_pid != pid(), "oldPid != getpid()");
        sleep(1.0);
        let _ = std::fs::remove_file(filename);
        signal(master_pid(serv), SIGTERM);
    }
}

unsafe extern "C" fn reload_all_workers_start(
    _: *mut c_void,
    serv: *mut c_void,
    worker: *mut WorkerRecord,
) {
    let serv = Server(serv);
    reload_on_worker_start(serv, worker, "/tmp/worker_1.pid", manager_pid(serv));
}

unsafe extern "C" fn reload_all_workers2_start(
    _: *mut c_void,
    serv: *mut c_void,
    worker: *mut WorkerRecord,
) {
    let serv = Server(serv);
    reload_on_worker_start(serv, worker, "/tmp/worker_2.pid", master_pid(serv));
}

/// `reload_all_workers`' and `reload_all_workers2`'s Server: two workers and two task
/// workers, created.
fn reload_server(task_enable_coroutine: bool) -> Server {
    let serv = Server::new(MODE_PROCESS);
    serv.config().worker_num = 2;
    serv.config().task_worker_num = 2;
    serv.config().max_wait_time = 1;
    serv.config().task_enable_coroutine = task_enable_coroutine;
    counter_init();
    set_log_warning();

    serv.add_port(SW_SOCK_TCP, TEST_HOST, 0);
    serv.set_callback(cb::CB_TASK as usize, return_ok as *const c_void);
    serv.set_callback(CB_RECEIVE, return_ok as *const c_void);

    assert_eq!(serv.create(), SW_OK);

    serv.set_callback(
        cb::CB_BEFORE_RELOAD as usize,
        before_reload as *const c_void,
    );
    serv.set_callback(cb::CB_AFTER_RELOAD as usize, after_reload as *const c_void);
    serv
}

/// `TEST(server, reload_all_workers)`: the manager is signalled.
fn reload_all_workers() {
    let serv = reload_server(true);
    serv.set_callback(CB_WORKER_START, reload_all_workers_start as *const c_void);

    assert_eq!(serv.start(), 0);
    assert_eq!(counter_get(10), 2); // onBeforeReload called
    assert_eq!(counter_get(11), 2); // onAfterReload called
    assert_no_failure();
    serv.delete();
}

/// `TEST(server, reload_all_workers2)`: the master is signalled.
fn reload_all_workers2() {
    let serv = reload_server(false);
    serv.set_callback(CB_WORKER_START, reload_all_workers2_start as *const c_void);

    assert_eq!(serv.start(), 0);
    assert_eq!(counter_get(10), 2); // onBeforeReload called
    assert_eq!(counter_get(11), 2); // onAfterReload called
    assert_no_failure();
    serv.delete();
}

unsafe extern "C" fn kill_user_workers_user_start(
    _: *mut c_void,
    _: *mut c_void,
    worker: *mut WorkerRecord,
) {
    expect(worker_id(worker) > 0, "worker->id > 0");
    sleep_forever();
}

unsafe extern "C" fn sleeping_task(_: *mut c_void, _: *mut c_void, _: *mut c_void) -> c_int {
    sleep_forever();
}

unsafe extern "C" fn kill_user_workers_start(
    _: *mut c_void,
    serv: *mut c_void,
    worker: *mut WorkerRecord,
) {
    if worker_id(worker) == 1 {
        sleep(1.0);
        signal(manager_pid(Server(serv)), SIGTERM);
    }
}

/// `TEST(server, kill_user_workers)`.
fn kill_user_workers() {
    counters();
    let serv = Server::new(MODE_BASE);
    serv.config().worker_num = 1;
    serv.config().task_worker_num = 2;
    serv.config().max_wait_time = 1;
    set_log_warning();

    let worker1 = add_user_worker(serv);
    let worker2 = add_user_worker(serv);
    assert!(serv.add_port(SW_SOCK_TCP, TEST_HOST, 0).is_some());

    assert_eq!(serv.create(), SW_OK);

    serv.set_callback(
        cb::CB_USER_WORKER_START as usize,
        kill_user_workers_user_start as *const c_void,
    );
    serv.set_callback(cb::CB_TASK as usize, sleeping_task as *const c_void);
    serv.set_callback(CB_WORKER_START, kill_user_workers_start as *const c_void);
    serv.set_callback(CB_RECEIVE, return_ok as *const c_void);

    assert_eq!(serv.start(), 0);
    delete_worker(worker1);
    delete_worker(worker2);
    assert_no_failure();
    serv.delete();
}

unsafe extern "C" fn force_kill_user_start(_: *mut c_void, _: *mut c_void, _: *mut WorkerRecord) {
    counter_incr(1, 1);
    sleep_forever();
}

unsafe extern "C" fn force_kill_worker_start(
    _: *mut c_void,
    serv: *mut c_void,
    _: *mut WorkerRecord,
) {
    counter_incr(1, 1);
    if is_task_worker() {
        sleep_forever();
    }
    timer_tick(10000, idle_timer, Server(serv));
}

unsafe extern "C" fn force_kill_manager_start(_: *mut c_void, serv: *mut c_void) {
    timer_after(200, shutdown_timer, Server(serv));
}

/// `TEST(server, force_kill_all_workers)`.
fn force_kill_all_workers() {
    counters();
    let serv = Server::new(MODE_PROCESS);
    serv.config().worker_num = 2;
    serv.config().task_worker_num = 3;
    serv.config().max_wait_time = 1;
    set_log_warning();

    let worker1 = add_user_worker(serv);
    let worker2 = add_user_worker(serv);
    assert!(serv.add_port(SW_SOCK_TCP, TEST_HOST, 0).is_some());

    assert_eq!(serv.create(), SW_OK);

    serv.set_callback(
        cb::CB_USER_WORKER_START as usize,
        force_kill_user_start as *const c_void,
    );
    serv.set_callback(cb::CB_TASK as usize, return_ok as *const c_void);
    serv.set_callback(CB_WORKER_START, force_kill_worker_start as *const c_void);
    serv.set_callback(CB_RECEIVE, return_ok as *const c_void);
    serv.set_callback(
        cb::CB_MANAGER_START as usize,
        force_kill_manager_start as *const c_void,
    );

    assert_eq!(serv.start(), 0);
    assert_eq!(counter_get(1), 7);

    delete_worker(worker1);
    delete_worker(worker2);
    assert_no_failure();
    serv.delete();
}

unsafe extern "C" fn kill_user_workers1_user_start(
    _: *mut c_void,
    _: *mut c_void,
    worker: *mut WorkerRecord,
) {
    expect(worker_id(worker) > 0, "worker->id > 0");
}

unsafe extern "C" fn spinning_task(_: *mut c_void, _: *mut c_void, _: *mut c_void) -> c_int {
    loop {
        std::hint::spin_loop();
    }
}

unsafe extern "C" fn kill_user_workers1_start(
    _: *mut c_void,
    serv: *mut c_void,
    worker: *mut WorkerRecord,
) {
    if worker_id(worker) == 1 {
        sleep(1.0);
        signal(master_pid(Server(serv)), SIGTERM);
    }
}

/// `TEST(server, kill_user_workers1)`.
fn kill_user_workers1() {
    counters();
    let serv = Server::new(MODE_PROCESS);
    serv.config().worker_num = 1;
    serv.config().task_worker_num = 2;
    serv.config().max_wait_time = 1;
    set_log_warning();

    add_user_worker(serv);
    add_user_worker(serv);

    assert!(serv.add_port(SW_SOCK_TCP, TEST_HOST, 0).is_some());

    assert_eq!(serv.create(), SW_OK);

    serv.set_callback(
        cb::CB_USER_WORKER_START as usize,
        kill_user_workers1_user_start as *const c_void,
    );
    serv.set_callback(cb::CB_TASK as usize, spinning_task as *const c_void);
    serv.set_callback(CB_WORKER_START, kill_user_workers1_start as *const c_void);
    serv.set_callback(CB_RECEIVE, return_ok as *const c_void);

    assert_eq!(serv.start(), 0);
    assert_no_failure();
    serv.delete();
}

unsafe extern "C" fn count_worker_start(_: *mut c_void, _: *mut c_void, _: *mut WorkerRecord) {
    counter_incr(1, 1);
}

// `reload_thread_2`'s and `reload_thread_3`'s `flags[...]`, as counters.
const RELOAD: usize = 12;
const ON_BEFORE_RELOAD: usize = 13;
const ON_AFTER_RELOAD: usize = 14;
const ON_MANAGER_STOP: usize = 15;
const SHUTDOWN: usize = 16;
const WORKER_STARTS: usize = 17;
const USER_WORKER_STARTS: usize = 4;

unsafe extern "C" fn reload_thread_timer(_: *mut c_void, tnode: *mut TimerNode) {
    counter_set(RELOAD, 1);
    expect(reload(timer_server(tnode), true), "serv->reload(true)");
}

/// `onManagerStart`: reload every worker after 500 ms.
unsafe extern "C" fn reload_thread_manager_start(_: *mut c_void, serv: *mut c_void) {
    timer_after(500, reload_thread_timer, Server(serv));
}

unsafe extern "C" fn reload_thread_shutdown(_: *mut c_void, tnode: *mut TimerNode) {
    counter_set(SHUTDOWN, 1);
    shutdown(timer_server(tnode));
}

unsafe extern "C" fn reload_thread_manager_stop(_: *mut c_void, _: *mut c_void) {
    counter_set(ON_MANAGER_STOP, 1);
}

unsafe extern "C" fn reload_thread_before_reload(_: *mut c_void, _: *mut c_void) {
    counter_set(ON_BEFORE_RELOAD, 1);
}

unsafe extern "C" fn count_thread_worker_start(_: *mut c_void, _: *mut c_void, _: *mut c_void) {
    counter_incr(WORKER_STARTS, 1);
}

/// `reload_thread_2`'s `onAfterReload`: shut down 500 ms later.
unsafe extern "C" fn reload_thread_2_after_reload(_: *mut c_void, serv: *mut c_void) {
    counter_set(ON_AFTER_RELOAD, 1);
    timer_after(500, reload_thread_shutdown, Server(serv));
}

unsafe extern "C" fn reload_thread_2_user_start(_: *mut c_void, _: *mut c_void, _: *mut c_void) {
    std::thread::sleep(Duration::from_millis(500));
    counter_incr(USER_WORKER_STARTS, 1);
}

/// A thread-mode Server of two workers on a port, with `onWorkerStart` counted and the
/// manager's 500 ms reload; `create` comes after `prepare`.
fn reload_thread_server(prepare: impl FnOnce(Server)) -> Server {
    let serv = Server::new(MODE_THREAD);
    serv.config().worker_num = 2;
    counter_init();
    case::log_level(case::SW_LOG_INFO);

    assert!(serv.add_port(SW_SOCK_TCP, TEST_HOST, 0).is_some());
    prepare(serv);
    assert_eq!(serv.create(), SW_OK);

    serv.set_callback(CB_WORKER_START, count_thread_worker_start as *const c_void);
    serv.set_callback(CB_RECEIVE, return_ok as *const c_void);
    serv.set_callback(
        cb::CB_MANAGER_START as usize,
        reload_thread_manager_start as *const c_void,
    );
    serv.set_callback(
        cb::CB_MANAGER_STOP as usize,
        reload_thread_manager_stop as *const c_void,
    );
    serv
}

/// `TEST(server, reload_thread_2)`: two task workers and a user worker too. Its `lock`,
/// which the last `onWorkerStart` unlocks, has no other user, so it is left out.
fn reload_thread_2() {
    let mut user_worker = ptr::null_mut();
    let serv = reload_thread_server(|serv| {
        serv.config().task_worker_num = 2;
        user_worker = add_user_worker(serv);
    });
    serv.set_callback(cb::CB_TASK as usize, return_ok as *const c_void);
    serv.set_callback(
        cb::CB_USER_WORKER_START as usize,
        reload_thread_2_user_start as *const c_void,
    );
    serv.set_callback(
        cb::CB_BEFORE_RELOAD as usize,
        reload_thread_before_reload as *const c_void,
    );
    serv.set_callback(
        cb::CB_AFTER_RELOAD as usize,
        reload_thread_2_after_reload as *const c_void,
    );

    assert_eq!(serv.start(), SW_OK);

    assert_eq!(counter_get(ON_BEFORE_RELOAD), 1);
    assert_eq!(counter_get(ON_AFTER_RELOAD), 1);
    assert_eq!(counter_get(ON_MANAGER_STOP), 1);
    assert_eq!(counter_get(RELOAD), 1);
    assert_eq!(counter_get(SHUTDOWN), 1);
    assert!(counter_get(USER_WORKER_STARTS) >= 2);
    assert_no_failure();
    serv.delete();
    delete_worker(user_worker);
}

/// `reload_thread`'s `onUserWorkerStart`: until the server stops.
unsafe extern "C" fn reload_thread_user_start(_: *mut c_void, serv: *mut c_void, _: *mut c_void) {
    let config = Server(serv).config();
    // SAFETY: the running server's flag, which shutdown clears from another thread; read
    // anew each time, as upstream's loop reads `serv->running`.
    while unsafe { ptr::read_volatile(&raw const config.running) } {
        sleep(0.1);
    }
}

unsafe extern "C" fn unlock_on_manager_start(_: *mut c_void, _: *mut c_void) {
    unlock();
}

/// `TEST(server, reload_thread)`: a client thread reloads every worker once the manager
/// starts, and a second reload is refused while the first runs. Its trace logging is
/// left out.
fn reload_thread() {
    let mut user_worker = ptr::null_mut();
    let serv = reload_thread_server(|serv| {
        serv.config().task_worker_num = 2;
        user_worker = add_user_worker(serv);
    });
    client_thread(
        move || master_pid(serv),
        move || {
            // Blocks every signal, so the shutdown's SIGTERM reaches the master's
            // signalfd rather than killing the process here.
            swoole_rs_corebase::swoole_thread_init(false);
            lock_wait();
            sleep(0.01);
            expect(reload(serv, true), "serv.reload(true)");
            expect(!reload(serv, true), "!serv.reload(true)");
            expect(
                last_error() == SW_ERROR_OPERATION_NOT_SUPPORT,
                "EXPECT_ERREQ(SW_ERROR_OPERATION_NOT_SUPPORT)",
            );
            sleep(1.0);
            expect(shutdown(serv), "serv.shutdown()");
            swoole_rs_corebase::swoole_thread_clean(false);
        },
    );
    serv.set_callback(cb::CB_TASK as usize, return_ok as *const c_void);
    serv.set_callback(
        cb::CB_USER_WORKER_START as usize,
        reload_thread_user_start as *const c_void,
    );
    serv.set_callback(
        cb::CB_MANAGER_START as usize,
        unlock_on_manager_start as *const c_void,
    );

    assert_eq!(serv.start(), SW_OK);
    join_client();

    // Each of the two workers and two task workers starts twice.
    assert_eq!(counter_get(WORKER_STARTS), 4 * 2);
    assert_no_failure();
    serv.delete();
    delete_worker(user_worker);
}

/// `reload_thread_3`'s `onAfterReload`: a task-worker reload is refused (there are none),
/// then the server shuts down 500 ms later.
unsafe extern "C" fn reload_thread_3_after_reload(_: *mut c_void, serv: *mut c_void) {
    counter_set(ON_AFTER_RELOAD, 1);
    let serv = Server(serv);
    expect(!reload(serv, false), "!serv->reload(false)");
    expect(
        last_error() == SW_ERROR_OPERATION_NOT_SUPPORT,
        "EXPECT_ERREQ(SW_ERROR_OPERATION_NOT_SUPPORT)",
    );
    timer_after(500, reload_thread_shutdown, serv);
}

/// `TEST(server, reload_thread_3)`.
fn reload_thread_3() {
    let serv = reload_thread_server(|_| {});
    serv.set_callback(
        cb::CB_AFTER_RELOAD as usize,
        reload_thread_3_after_reload as *const c_void,
    );

    assert_eq!(serv.start(), SW_OK);

    assert_eq!(counter_get(ON_AFTER_RELOAD), 1);
    assert_eq!(counter_get(ON_MANAGER_STOP), 1);
    assert_eq!(counter_get(RELOAD), 1);
    assert_eq!(counter_get(SHUTDOWN), 1);
    assert!(counter_get(WORKER_STARTS) >= 2 * 2);
    assert_no_failure();
    serv.delete();
}

/// `TEST(server, restart_worker_process)`.
fn restart_worker_process() {
    let serv = Server::new(MODE_BASE);
    serv.config().worker_num = 1;
    serv.config().task_worker_num = 2;
    counter_init();

    assert!(serv.add_port(SW_SOCK_TCP, TEST_HOST, 0).is_some());

    serv.set_callback(CB_WORKER_START, count_worker_start as *const c_void);

    assert_eq!(serv.create(), SW_OK);

    let fake_exit = ExitStatus::new(pid(), 0);
    let pool = serv.view().task_pool();
    let worker = serv.view().worker(2);
    // SAFETY: the created server's event pool, which follows its task pool, and its
    // event workers; then the task worker, and the task pool it joins.
    unsafe {
        (*pool.add(1)).set_workers(serv.config().workers.cast());
        Worker::new(worker).set_pid(pid());
        swoole_rs_process::methods::add_worker(&*pool, Worker::new(worker));
    }
    _ = manager_ops::restart_worker_process(serv.view(), &fake_exit);

    wait_all_child_processes();

    assert_eq!(counter_get(1), 1);
    assert_no_failure();
    serv.delete();
}

static TASK_ONLY: AtomicBool = AtomicBool::new(false);
static WATCHDOG_DEADLINE: AtomicU64 = AtomicU64::new(0);

/// `reload_watchdog`'s `completed`.
fn reload_completed() -> bool {
    let task_only = TASK_ONLY.load(Ordering::SeqCst);
    counter_get(25) == 1 && counter_get(0) == if task_only { 2 } else { 3 } && counter_get(1) == 2
}

unsafe extern "C" fn reload_watchdog_tick(_: *mut c_void, tnode: *mut TimerNode) {
    let serv = timer_server(tnode);
    if !reload_completed() {
        if now() < f64::from_bits(WATCHDOG_DEADLINE.load(Ordering::SeqCst)) {
            return;
        }
        counter_set(31, 1);
        eprintln!(
            "reload timeout: before={} after={}",
            counter_get(24),
            counter_get(25)
        );
        let config = serv.config();
        for i in 0..config.worker_num + config.task_worker_num {
            let worker = serv.view().worker(u16::try_from(i).unwrap());
            // SAFETY: one of the server's workers.
            let worker_pid = unsafe { Worker::new(worker) }.pid();
            eprintln!(
                "worker#{i} pid={worker_pid} starts={}",
                counter_get(i as usize)
            );
        }
    }
    // SAFETY: this timer's own node.
    unsafe { case::swoole_timer_del(tnode) };
    shutdown(serv);
}

/// `reload_watchdog(serv, completed)`: stops the server once the reload completed, or
/// after five seconds, recording the timeout in counter 31.
fn reload_watchdog(serv: Server) {
    WATCHDOG_DEADLINE.store((now() + 5.0).to_bits(), Ordering::SeqCst);
    timer_tick(10, reload_watchdog_tick, serv);
}

unsafe extern "C" fn worker_error_start(_: *mut c_void, _: *mut c_void, worker: *mut WorkerRecord) {
    let id = worker_id(worker);
    if counter_incr(id as usize, 1) == 1 && id == 0 {
        while counter_get(8) == 0 {
            sleep(0.001);
        }
        // SAFETY: _exit(2).
        unsafe { _exit(42) };
    }
}

unsafe extern "C" fn worker_error_manager_start(_: *mut c_void, _: *mut c_void) {
    counter_set(8, 1);
}

unsafe extern "C" fn worker_error_on_error(
    _: *mut c_void,
    serv: *mut c_void,
    _: *mut WorkerRecord,
    status: *const ExitStatus,
) {
    // SAFETY: the exit status the server passed the callback.
    let code = (unsafe { &*status }.status() >> 8) & 0xff;
    if code != 42
        || counter_incr(27, 1) != 1
        || !reload(Server(serv), !TASK_ONLY.load(Ordering::SeqCst))
    {
        counter_incr(26, 1);
    }
}

unsafe extern "C" fn worker_error_before_reload(_: *mut c_void, _: *mut c_void) {
    // The error callback requests reload before its crashed worker is respawned.
    // Wait until the replacement and task worker have installed their signal handlers.
    let deadline = now() + 3.0;
    while (counter_get(0) < 2 || counter_get(1) < 1) && now() < deadline {
        sleep(0.001);
    }
    counter_incr(24, 1);
}

unsafe extern "C" fn worker_error_after_reload(_: *mut c_void, _: *mut c_void) {
    if counter_get(24) != 1 {
        counter_incr(26, 1);
    }
    counter_incr(25, 1);
}

unsafe extern "C" fn worker_error_start_server(_: *mut c_void, serv: *mut c_void) {
    reload_watchdog(Server(serv));
}

/// `test_reload_from_worker_error(task_only)`: a reload requested by a manager callback
/// arrives after this iteration's initialization phase.
fn test_reload_from_worker_error(task_only: bool) {
    TASK_ONLY.store(task_only, Ordering::SeqCst);
    let serv = Server::new(MODE_PROCESS);
    serv.config().worker_num = 1;
    serv.config().task_worker_num = 1;
    serv.config().reload_async = false;
    serv.config().max_wait_time = 1;
    counter_init();
    assert!(serv.add_port(SW_SOCK_TCP, TEST_HOST, 0).is_some());
    assert_eq!(serv.create(), SW_OK);
    serv.set_callback(cb::CB_TASK as usize, return_ok as *const c_void);
    serv.set_callback(CB_RECEIVE, return_ok as *const c_void);
    serv.set_callback(CB_WORKER_START, worker_error_start as *const c_void);
    serv.set_callback(
        cb::CB_MANAGER_START as usize,
        worker_error_manager_start as *const c_void,
    );
    serv.set_callback(
        cb::CB_WORKER_ERROR as usize,
        worker_error_on_error as *const c_void,
    );
    serv.set_callback(
        cb::CB_BEFORE_RELOAD as usize,
        worker_error_before_reload as *const c_void,
    );
    serv.set_callback(
        cb::CB_AFTER_RELOAD as usize,
        worker_error_after_reload as *const c_void,
    );
    serv.set_callback(CB_START, worker_error_start_server as *const c_void);
    assert_eq!(serv.start(), SW_OK);
    assert_eq!(counter_get(31), 0);
    assert_eq!(counter_get(26), 0);
    assert_eq!(counter_get(24), 1);
    assert_eq!(counter_get(25), 1);
    assert_eq!(counter_get(27), 1);
    assert_eq!(counter_get(0), if task_only { 2 } else { 3 });
    assert_eq!(counter_get(1), 2);
    assert_no_failure();
    serv.delete();
}

/// `TEST(server, reload_all_from_worker_error)`.
fn reload_all_from_worker_error() {
    test_reload_from_worker_error(false);
}

/// `TEST(server, reload_tasks_from_worker_error)`.
fn reload_tasks_from_worker_error() {
    test_reload_from_worker_error(true);
}

/// `swoole::Mutex lock(true); lock.lock()`: the client thread waits on it, and `onStart`
/// unlocks it, both in the master.
/// The process-shared `Mutex lock(true)`: in `MODE_BASE`, `onStart` and
/// `onWorkerStart` run in forked processes, so the flag lives in the shared counters,
/// which the case maps before its server starts.
fn lock_wait() {
    while counters()[STARTED].load(Ordering::SeqCst) == 0 {
        sleep(0.001);
    }
}

fn unlock() {
    counters()[STARTED].store(1, Ordering::SeqCst);
}

/// `sw_worker()`: this thread's worker, or null.
fn current_worker() -> *mut WorkerRecord {
    swoole_rs_process::worker_globals::current_worker()
}

fn kill_worker(serv: Server, worker_id: c_int) -> bool {
    event_worker_ops::kill_worker(serv.view(), worker_id)
}

/// `test_kill_worker`'s `Options`.
#[derive(Clone, Copy)]
struct Options {
    reload_async: bool,
    worker_exit_callback: bool,
    test_shutdown_event: bool,
}

const OPTIONS: Options = Options {
    reload_async: true,
    worker_exit_callback: false,
    test_shutdown_event: false,
};

static KILL_OPTIONS: Mutex<Options> = Mutex::new(OPTIONS);
static TEST_TIMER: AtomicI64 = AtomicI64::new(0);

fn kill_options() -> Options {
    *KILL_OPTIONS.lock().unwrap()
}

unsafe extern "C" fn kill_on_connect(_: *mut c_void, _: *mut c_void, ev: *mut DataHead) {
    // SAFETY: the event head the server passed the callback.
    counter_set(4, unsafe { (*ev).fd } as i32);
    counter_set(5, worker_id(current_worker()).cast_signed());
}

unsafe extern "C" fn kill_on_receive(
    _: *mut c_void,
    serv: *mut c_void,
    req: *mut RecvData,
) -> c_int {
    let (session, _) = request(req);
    Server(serv).send(session, b"OK");
    counter_incr(0, 1);
    SW_OK
}

unsafe extern "C" fn kill_on_worker_stop(_: *mut c_void, serv: *mut c_void, _: *mut WorkerRecord) {
    server_methods::close(Server(serv).view(), c_long::from(counter_get(4)), false);
    event_worker_ops::drain_worker_pipe(Server(serv).view());
}

unsafe extern "C" fn kill_self_on_worker_stop(
    _: *mut c_void,
    serv: *mut c_void,
    _: *mut WorkerRecord,
) {
    server_methods::close(Server(serv).view(), c_long::from(counter_get(4)), false);
}

unsafe extern "C" fn kill_on_close(_: *mut c_void, _: *mut c_void, _: *mut DataHead) {
    counter_incr(2, 1);
}

unsafe extern "C" fn kill_on_worker_start(
    _: *mut c_void,
    serv: *mut c_void,
    worker: *mut WorkerRecord,
) {
    let options = kill_options();
    let c = counter_incr(1, 1) - 1;
    if options.worker_exit_callback {
        TEST_TIMER.store(timer_tick(5000, idle_timer, Server(serv)), Ordering::SeqCst);
    }

    if c < 2 && options.test_shutdown_event && worker_id(worker) == 0 {
        // SAFETY: zeroed is `ev.info = {}`.
        let mut ev: DataHead = unsafe { std::mem::zeroed() };
        ev.type_ = SW_SERVER_EVENT_SHUTDOWN;
        ev.len = 0;
        let dst = Server(serv).view().worker(1);
        // SAFETY: worker 1 of the running server; the event's head.
        let sent = unsafe {
            send_pipe_message(
                dst,
                (&raw const ev).cast(),
                size_of::<DataHead>(),
                SW_PIPE_MASTER | SW_PIPE_NONBLOCK,
            )
        };
        check(sent > 0, "send_to_worker_from_worker(1, &ev, ...) > 0");
    }
}

unsafe extern "C" fn kill_on_worker_exit(_: *mut c_void, _: *mut c_void, _: *mut WorkerRecord) {
    // SAFETY: clears this worker's timer by its id.
    unsafe { swoole_timer_clear(TEST_TIMER.load(Ordering::SeqCst)) };
    counter_incr(6, 1);
}

unsafe extern "C" fn kill_on_start(_: *mut c_void, serv: *mut c_void) {
    if current_worker().is_null() && !check(!kill_worker(Server(serv), -1), "!kill_worker(-1)") {
        return;
    }
    unlock();
}

unsafe extern "C" fn kill_self_on_worker_start(
    _: *mut c_void,
    _: *mut c_void,
    _: *mut WorkerRecord,
) {
    counter_incr(1, 1);
}

/// `network::SyncClient::recv` into `String rbuf(1024)`: the bytes read, 0 at EOF, -1 on
/// an error.
fn sync_recv(c: &mut std::net::TcpStream) -> isize {
    let mut rbuf = [0u8; 1024];
    std::io::Read::read(c, &mut rbuf).map_or(-1, |n| n.cast_signed())
}

/// `test_kill_worker`'s and `test_kill_self`'s client thread: one request, then the
/// worker `victim` names is killed and the connection must close.
fn kill_client(serv: Server, port: u16, victim: fn() -> c_int) -> JoinHandle<()> {
    std::thread::spawn(move || {
        block_all_signals();

        lock_wait();

        sleep(0.05);

        let c = std::net::TcpStream::connect((TEST_HOST, port));
        expect(c.is_ok(), "c.connect(TEST_HOST, port->port)");
        let Ok(mut c) = c else { return };

        expect(
            std::io::Write::write(&mut c, TEST_STR.as_bytes()).ok() == Some(TEST_STR.len()),
            "c.send(TEST_STR) == strlen(TEST_STR)",
        );

        let rn = sync_recv(&mut c);
        expect(rn == 2, "rn == 2");

        kill_worker(serv, victim());

        let rn = sync_recv(&mut c);
        expect(rn == 0, "rn == 0");

        counter_incr(3, 1);

        sleep(0.05);

        shutdown(serv);
    })
}

/// `test_kill_worker(mode, options)`.
fn test_kill_worker(mode: c_int, options: Options) {
    *KILL_OPTIONS.lock().unwrap() = options;
    let serv = Server::new(mode);
    serv.config().worker_num = 2;
    serv.config().reload_async = options.reload_async;

    counter_init();

    let port = serv.add_port(SW_SOCK_TCP, TEST_HOST, 0);
    assert!(port.is_some());

    assert_eq!(serv.create(), SW_OK);

    serv.set_callback(CB_CONNECT, kill_on_connect as *const c_void);
    serv.set_callback(CB_RECEIVE, kill_on_receive as *const c_void);
    serv.set_callback(
        cb::CB_WORKER_STOP as usize,
        kill_on_worker_stop as *const c_void,
    );
    serv.set_callback(CB_CLOSE, kill_on_close as *const c_void);
    serv.set_callback(CB_WORKER_START, kill_on_worker_start as *const c_void);
    if options.worker_exit_callback {
        serv.set_callback(
            cb::CB_WORKER_EXIT as usize,
            kill_on_worker_exit as *const c_void,
        );
    }
    serv.set_callback(CB_START, kill_on_start as *const c_void);

    let t = kill_client(serv, port.unwrap().port(), || 1 - counter_get(5));

    assert_eq!(serv.start(), SW_OK);
    t.join().unwrap();

    assert_eq!(counter_get(0), 1); // Client receive
    assert_eq!(
        counter_get(1),
        if options.test_shutdown_event { 4 } else { 3 }
    ); // Server onWorkerStart
    assert_eq!(counter_get(2), 1); // Server onClose
    assert_eq!(counter_get(3), 1); // Client close
    // counter[4] is the client fd
    // counter[5] is the worker id
    // counter[6] is the worker exit count

    if options.worker_exit_callback {
        assert_eq!(counter_get(6), 3); // Worker exit
    }
    assert_no_failure();
    serv.delete();
}

/// `TEST(server, kill_worker_1)`.
fn kill_worker_1() {
    test_kill_worker(
        MODE_BASE,
        Options {
            reload_async: true,
            ..OPTIONS
        },
    );
}

/// `TEST(server, kill_worker_2)`.
fn kill_worker_2() {
    test_kill_worker(
        MODE_PROCESS,
        Options {
            reload_async: true,
            ..OPTIONS
        },
    );
}

/// `TEST(server, kill_worker_3)`.
fn kill_worker_3() {
    test_kill_worker(
        MODE_THREAD,
        Options {
            reload_async: true,
            ..OPTIONS
        },
    );
}

/// `TEST(server, kill_worker_4)`.
fn kill_worker_4() {
    test_kill_worker(
        MODE_BASE,
        Options {
            reload_async: false,
            ..OPTIONS
        },
    );
}

/// `TEST(server, kill_worker_5)`.
fn kill_worker_5() {
    test_kill_worker(
        MODE_PROCESS,
        Options {
            reload_async: false,
            ..OPTIONS
        },
    );
}

/// `TEST(server, kill_worker_6)`.
fn kill_worker_6() {
    test_kill_worker(
        MODE_THREAD,
        Options {
            reload_async: false,
            ..OPTIONS
        },
    );
}

/// `TEST(server, worker_exit)`.
fn worker_exit() {
    test_kill_worker(
        MODE_PROCESS,
        Options {
            worker_exit_callback: true,
            ..OPTIONS
        },
    );
}

/// `TEST(server, shutdown_event)`.
fn shutdown_event() {
    test_kill_worker(
        MODE_PROCESS,
        Options {
            test_shutdown_event: true,
            ..OPTIONS
        },
    );
}

/// `test_kill_self(mode)`: its counters are `sw_mem_pool()` memory, as `test::counter_*`'s.
fn test_kill_self(mode: c_int) {
    let serv = Server::new(mode);
    serv.config().worker_num = 2;

    counters();

    let port = serv.add_port(SW_SOCK_TCP, TEST_HOST, 0);
    assert!(port.is_some());

    assert_eq!(serv.create(), SW_OK);

    serv.set_callback(CB_CONNECT, kill_on_connect as *const c_void);
    serv.set_callback(CB_RECEIVE, kill_on_receive as *const c_void);
    serv.set_callback(
        cb::CB_WORKER_STOP as usize,
        kill_self_on_worker_stop as *const c_void,
    );
    serv.set_callback(CB_CLOSE, kill_on_close as *const c_void);
    serv.set_callback(CB_WORKER_START, kill_self_on_worker_start as *const c_void);
    serv.set_callback(CB_START, kill_on_start as *const c_void);

    let t = kill_client(serv, port.unwrap().port(), || counter_get(5));

    assert_eq!(serv.start(), SW_OK);
    t.join().unwrap();

    assert_eq!(counter_get(0), 1); // Client receive
    assert_eq!(counter_get(1), 3); // Server onWorkerStart
    assert_eq!(counter_get(2), 1); // Server onClose
    assert_eq!(counter_get(3), 1); // Client close
    assert_no_failure();
    serv.delete();
}

/// `TEST(server, kill_self)`.
fn kill_self() {
    test_kill_self(MODE_BASE);
}

const PACKET: &[u8] = b"hello world\n";
const PID_FILE: &str = "/tmp/swoole-core-tests.pid";

/// `swoole_clear_last_error()`.
fn clear_last_error() {
    // SAFETY: sets this thread's last error.
    unsafe { swr_v1_core_set_last_error(0) };
}

/// `packet`, answered with `Server: ` before it.
fn server_response() -> Vec<u8> {
    [b"Server: ".as_slice(), PACKET].concat()
}

/// `ListenPort::get_connection_num()` of the primary port.
fn primary_port_connection_num(serv: Server) -> usize {
    // SAFETY: the running server's primary port.
    serv.view().port_connection_num(
        unsafe { swoole_rs_ffi::view::Port::from_raw(serv.get_primary_port().0) }.unwrap(),
    )
}

/// One `SyncClient` request: connect, send `packet`, read up to 1024 bytes, close.
fn send_packet(port: u16) {
    if let Ok(mut c) = std::net::TcpStream::connect((TEST_HOST, port)) {
        let _ = std::io::Write::write_all(&mut c, PACKET);
        let mut buf = [0u8; 1024];
        let _ = std::io::Read::read(&mut c, &mut buf);
    }
}

unsafe extern "C" fn before_shutdown(_: *mut c_void, _: *mut c_void) {
    counter_incr(10, 1);
}

unsafe extern "C" fn base_worker_start(_: *mut c_void, _: *mut c_void, _: *mut WorkerRecord) {
    unlock();
}

unsafe extern "C" fn base_receive(_: *mut c_void, serv: *mut c_void, req: *mut RecvData) -> c_int {
    let serv = Server(serv);
    let (session, data) = request(req);
    expect(data == PACKET, "string(req->data, req->info.len) == packet");
    let conn = serv.get_connection_by_session_id(session);
    // SAFETY: the running server's live connection, laid out as the crate's record.
    let conn =
        unsafe { swoole_rs_server::Connection::from_ptr(ptr::from_ref(conn).cast_mut().cast()) };
    let conn = conn.unwrap_or_else(|| std::process::abort());
    let addr = swoole_rs_server::local_addr::get_local_addr(serv.view(), conn);
    expect(addr != c"unknown", "serv->get_local_addr(conn) != unknown");

    let resp = server_response();
    serv.send(session, &resp);

    let finished = task_worker_ops::finish(serv.view(), &resp, 0, None);
    expect(!finished, "!serv->finish(resp)");

    expect(
        serv.view().connection_num() == 1,
        "get_connection_num() == 1",
    );
    expect(
        primary_port_connection_num(serv) == 1,
        "get_primary_port()->get_connection_num() == 1",
    );

    // SAFETY: the base server's message bus.
    let packet = unsafe { swoole_rs_server::bus_move_packet(serv.view().message_bus()) };
    expect(
        packet.is_null(),
        "get_worker_message_bus()->move_packet() == nullptr",
    );

    // session not exists
    let client_fd: c_long = 9999;
    clear_last_error();
    expect(!serv.send(client_fd, &resp), "!serv->send(client_fd)");
    expect(
        last_error() == SW_ERROR_SESSION_NOT_EXIST,
        "SW_ERROR_SESSION_NOT_EXIST",
    );

    clear_last_error();
    expect(
        !server_methods::close(serv.view(), client_fd, false),
        "!serv->close(client_fd)",
    );
    expect(
        last_error() == SW_ERROR_SESSION_NOT_EXIST,
        "SW_ERROR_SESSION_NOT_EXIST",
    );

    clear_last_error();
    let sd = swoole_rs_server::SendData {
        info: swoole_rs_corebase::DataHead {
            fd: client_fd,
            kind: SW_SERVER_EVENT_CLOSE,
            ..Default::default()
        },
        data: ptr::null(),
    };
    // SAFETY: a `SendData` with no data.
    let sent = unsafe { server_methods::send_to_connection(serv.view(), &sd) };
    expect(sent == SW_ERR, "send_to_connection(&sd) == SW_ERR");
    expect(
        last_error() == SW_ERROR_SESSION_NOT_EXIST,
        "SW_ERROR_SESSION_NOT_EXIST",
    );

    SW_OK
}

unsafe extern "C" fn base_on_start(_: *mut c_void, _: *mut c_void) {
    check(
        std::fs::File::open(PID_FILE).is_ok(),
        "access(serv->pid_file, R_OK) == 0",
    );
}

/// `test_base()`. Its `Server::HOOK_WORKER_START` hook, which asserted `is_started()`,
/// left the build with `Server::add_hook` (#1535).
fn test_base() {
    let serv = Server::new(MODE_BASE);
    serv.config().worker_num = 1;
    serv.set_text(TEXT_PID_FILE, PID_FILE);

    counter_init();
    set_log_warning();

    let port = serv.add_port(SW_SOCK_TCP, TEST_HOST, 0);
    assert!(port.is_some());
    let port = port.unwrap().port();

    assert_eq!(serv.create(), SW_OK);

    clear_last_error();
    assert!(!shutdown(serv));
    assert_eq!(last_error(), SW_ERROR_WRONG_OPERATION);

    let t1 = std::thread::spawn(move || {
        block_all_signals();

        lock_wait();

        send_packet(port);

        // http1.rs's `kill(getpid(), SIGTERM)`, not this module's `TEST(server, kill_self)`.
        super::kill_self();
    });

    serv.set_callback(CB_WORKER_START, base_worker_start as *const c_void);
    serv.set_callback(CB_RECEIVE, base_receive as *const c_void);
    serv.set_callback(CB_START, base_on_start as *const c_void);
    serv.set_callback(
        cb::CB_BEFORE_SHUTDOWN as usize,
        before_shutdown as *const c_void,
    );

    serv.start();
    t1.join().unwrap();

    assert!(std::fs::File::open(PID_FILE).is_err());
    assert_eq!(counter_get(10), 1); // onBeforeShutdown called
    assert_no_failure();
    serv.delete();
}

/// `TEST(server, base)`.
fn base() {
    test_base();
}

/// The pipe `test_process`'s `Mutex *lock` stands for: shared with the forked worker 0,
/// which unlocks it, while the master's client thread waits.
static PROCESS_LOCK: Mutex<[c_int; 2]> = Mutex::new([-1, -1]);
static PROCESS_T1: Mutex<Option<JoinHandle<()>>> = Mutex::new(None);

unsafe extern "C" fn process_kill_timer(_: *mut c_void, tnode: *mut TimerNode) {
    // SAFETY: random.
    let id = 1 + unsafe { swoole_random_int() } % 3;
    kill_worker(timer_server(tnode), id as c_int);
}

unsafe extern "C" fn process_receive(
    _: *mut c_void,
    serv: *mut c_void,
    req: *mut RecvData,
) -> c_int {
    let serv = Server(serv);
    let (session, data) = request(req);
    expect(data == PACKET, "string(req->data, req->info.len) == packet");

    serv.send(session, &server_response());

    expect(
        serv.view().connection_num() == 1,
        "get_connection_num() == 1",
    );
    expect(
        primary_port_connection_num(serv) == 1,
        "get_primary_port()->get_connection_num() == 1",
    );

    timer_after(100, process_kill_timer, serv);

    SW_OK
}

unsafe extern "C" fn process_on_start(_: *mut c_void, serv: *mut c_void) {
    let serv = Server(serv);
    let t1 = std::thread::spawn(move || {
        block_all_signals();

        let fd = PROCESS_LOCK.lock().unwrap()[0];
        let mut byte = 0u8;
        // SAFETY: the lock's read end; one byte.
        unsafe { read(fd, (&raw mut byte).cast(), 1) };

        // SAFETY: worker 0 of the running server.
        let pid = unsafe { Worker::new(serv.view().worker(0)) }.pid();
        // SAFETY: __libc_current_sigrtmin(3).
        signal(pid, unsafe { __libc_current_sigrtmin() });

        send_packet(serv.get_primary_port().port());

        sleep(2.0);

        signal(master_pid(serv), SIGTERM);
    });
    *PROCESS_T1.lock().unwrap() = Some(t1);

    // command tests
    clear_last_error();
    let message = crate::TEST_STR;
    swoole_rs_server::swr_v1_server_command_result(serv.view(), 9999, message.as_bytes());
    check(
        last_error() == SW_ERROR_SERVER_INVALID_COMMAND,
        "ASSERT_ERREQ(SW_ERROR_SERVER_INVALID_COMMAND)",
    );
}

unsafe extern "C" fn process_worker_start(
    _: *mut c_void,
    _: *mut c_void,
    worker: *mut WorkerRecord,
) {
    if worker_id(worker) == 0 {
        let fd = PROCESS_LOCK.lock().unwrap()[1];
        // SAFETY: the lock's write end; one byte.
        unsafe { write(fd, b"1".as_ptr().cast(), 1) };
    }
    counter_incr(3, 1);
}

unsafe extern "C" fn manager_start_stop(_: *mut c_void, _: *mut c_void) {
    counter_incr(1, 1);
}

unsafe extern "C" fn on_shutdown(_: *mut c_void, _: *mut c_void) {
    counter_incr(9, 1);
}

/// `test_process(single_thread)`. Upstream's `manager_alarm` and its
/// `Server::HOOK_MANAGER_TIMER` hook, which counted `counter[2]`, left the build with the
/// other Server hooks (#1535), so nothing counts the manager timer.
/// `call_command_handler_in_master(9999, ...)` has no entry point left: its body is part
/// of `Server::command`, which looks the command up before it can be reached.
fn test_process(single_thread: bool) {
    let serv = Server::new(MODE_PROCESS);
    serv.config().worker_num = 1;
    serv.config().single_thread = single_thread;
    serv.config().task_worker_num = 3;
    set_log_warning();

    counter_init();

    let mut fds = [-1; 2];
    // SAFETY: a writable pair.
    assert_eq!(unsafe { pipe(&mut fds) }, 0);
    *PROCESS_LOCK.lock().unwrap() = fds;

    if serv.add_port(SW_SOCK_TCP, TEST_HOST, 0).is_none() {
        eprintln!("listen failed, [error={}]", last_error());
        std::process::exit(2);
    }

    assert_eq!(serv.create(), SW_OK);

    clear_last_error();
    assert!(serv.add_port(SW_SOCK_TCP, TEST_HOST, 0).is_none());
    assert_eq!(last_error(), SW_ERROR_WRONG_OPERATION);

    clear_last_error();
    let fake_worker = Box::into_raw(WorkerRecord::boxed());
    // SAFETY: the created server, and a live worker it refuses.
    assert_eq!(
        unsafe { worker_methods::add_worker(serv.view(), fake_worker) },
        SW_ERR
    );
    assert_eq!(last_error(), SW_ERROR_WRONG_OPERATION);
    delete_worker(fake_worker);

    serv.set_callback(CB_START, process_on_start as *const c_void);
    serv.set_callback(CB_WORKER_START, process_worker_start as *const c_void);
    serv.set_callback(CB_RECEIVE, process_receive as *const c_void);
    serv.set_callback(cb::CB_TASK as usize, return_ok as *const c_void);
    serv.set_callback(
        cb::CB_MANAGER_START as usize,
        manager_start_stop as *const c_void,
    );
    serv.set_callback(
        cb::CB_MANAGER_STOP as usize,
        manager_start_stop as *const c_void,
    );
    serv.set_callback(
        cb::CB_BEFORE_SHUTDOWN as usize,
        before_shutdown as *const c_void,
    );
    serv.set_callback(cb::CB_SHUTDOWN as usize, on_shutdown as *const c_void);

    assert_eq!(serv.start(), 0);

    let t1 = PROCESS_T1.lock().unwrap().take().expect("onStart ran");
    t1.join().unwrap();
    assert_eq!(counter_get(1), 2); // manager callback
    assert!(counter_get(3) >= 4); // worker start
    assert_eq!(counter_get(9), 1); // onShutdown called
    assert_eq!(counter_get(10), 1); // onBeforeShutdown called
    assert_no_failure();
    serv.delete();
}

/// `TEST(server, process)`.
fn process() {
    test_process(false);
    wait_all_child_processes();
}

/// `TEST(server, process_single_thread)`.
fn process_single_thread() {
    test_process(true);
    wait_all_child_processes();
}

/// `test_clean_worker`'s `AsyncClient ac(SW_SOCK_TCP)`, made before the server forks, so
/// a worker's `ac.get_client()` is the master's client, over the same socket.
static CLEAN_CLIENT: AtomicPtr<Client> = AtomicPtr::new(ptr::null_mut());
static CLEAN_PORT: AtomicU16 = AtomicU16::new(0);

/// `ac.get_client()`.
fn clean_client() -> Handle {
    // SAFETY: the client `test_clean_worker` made, live until it frees it.
    unsafe { Handle::new(CLEAN_CLIENT.load(Ordering::SeqCst)) }
}

/// `ac.get_client()->op(...)`: its result.
fn client_run(op: Op<'_>) -> i64 {
    clean_client().run(op)
}

/// The client's `on_connect`, `on_close`, `on_error` and `on_receive`.
fn clean_client_event(event: c_int, _: *const c_char, error: usize) {
    match event {
        ON_CONNECT => {
            client_run(Op::Send {
                data: TEST_STR.as_bytes(),
                flags: 0,
            });
        }
        ON_ERROR => eprintln!("WARNING: connect failed, error={error}"),
        ON_RECEIVE => {
            counter_incr(3, 1);
        }
        _ => {}
    }
}

unsafe extern "C" fn clean_connect_timer(_: *mut c_void, _: *mut TimerNode) {
    let events = [ON_CONNECT, ON_ERROR, ON_CLOSE, ON_RECEIVE]
        .iter()
        .fold(0, |events, kind| events | 1 << kind);
    // SAFETY: the live client.
    let client = unsafe { &*CLEAN_CLIENT.load(Ordering::SeqCst) };
    listen(client, events, std::rc::Rc::new(clean_client_event));
    let host = CString::new(TEST_HOST).unwrap();
    let connected = client_run(Op::Connect {
        host: &host,
        port: CLEAN_PORT.load(Ordering::SeqCst).into(),
        timeout: -1.0,
        flags: 0,
    }) >= 0;
    expect(connected, "ac.connect(TEST_HOST, port->get_port())");
}

unsafe extern "C" fn clean_on_start(_: *mut c_void, serv: *mut c_void) {
    timer_after(100, clean_connect_timer, Server(serv));
}

unsafe extern "C" fn clean_deferred(serv: *mut c_void) {
    let serv = Server(serv).view();
    // SAFETY: this process's worker.
    let worker = unsafe { Worker::new(current_worker()) };
    event_worker_ops::clean_worker_connections(serv, worker);
    client_run(Op::Shutdown(SHUT_RDWR));
    event_worker_ops::stop_async_worker(serv, worker);
}

unsafe extern "C" fn clean_on_connect(_: *mut c_void, serv: *mut c_void, _: *mut DataHead) {
    // SAFETY: a deferred call over the running server.
    unsafe { swoole_event_defer(clean_deferred, serv) };
}

unsafe extern "C" fn clean_worker_start(_: *mut c_void, serv: *mut c_void, _: *mut WorkerRecord) {
    if !check(
        Server(serv).view().connection_num() == 0,
        "serv->get_connection_num() == 0",
    ) {
        return;
    }
    if counter_incr(1, 1) == 3 {
        timer_after(100, shutdown_timer, Server(serv));
    }
}

unsafe extern "C" fn ignore_worker(_: *mut c_void, _: *mut c_void, _: *mut WorkerRecord) {}

/// `test_clean_worker(mode)`.
fn test_clean_worker(mode: c_int) {
    let serv = Server::new(mode);
    serv.config().worker_num = 2;

    counter_init();

    let client = swr_v1_clients_new(SW_SOCK_TCP, true);
    assert!(
        !client.is_null(),
        "AsyncClient ac(SW_SOCK_TCP) has a socket"
    );
    CLEAN_CLIENT.store(client, Ordering::SeqCst);

    let port = serv.add_port(SW_SOCK_TCP, TEST_HOST, 0);
    assert!(port.is_some());
    CLEAN_PORT.store(port.unwrap().port(), Ordering::SeqCst);
    assert_eq!(serv.create(), SW_OK);

    serv.set_callback(CB_CONNECT, clean_on_connect as *const c_void);
    serv.set_callback(CB_RECEIVE, kill_on_receive as *const c_void);
    serv.set_callback(CB_CLOSE, kill_on_close as *const c_void);
    serv.set_callback(CB_WORKER_START, clean_worker_start as *const c_void);
    serv.set_callback(cb::CB_WORKER_STOP as usize, ignore_worker as *const c_void);
    serv.set_callback(CB_START, clean_on_start as *const c_void);

    assert_eq!(serv.start(), SW_OK);
    assert_eq!(counter_get(0), 0); // Server on_receive
    assert_eq!(counter_get(1), 3); // worker start
    assert_eq!(counter_get(2), 0); // Server on_close
    assert_eq!(counter_get(3), 0); // Client on_receive
    assert_no_failure();
    // SAFETY: the client this test made, deleted once.
    unsafe { clean_client().free() };
    serv.delete();
}

/// `TEST(server, clean_worker_1)`.
fn clean_worker_1() {
    test_clean_worker(MODE_BASE);
}

/// `TEST(server, clean_worker_2)`.
fn clean_worker_2() {
    test_clean_worker(MODE_THREAD);
}

pub const CASES: &[(&str, fn())] = &[
    ("base", base),
    ("process", process),
    ("process_single_thread", process_single_thread),
    ("reload_all_workers", reload_all_workers),
    ("reload_all_workers2", reload_all_workers2),
    ("kill_user_workers", kill_user_workers),
    ("force_kill_all_workers", force_kill_all_workers),
    ("kill_user_workers1", kill_user_workers1),
    ("create_task_worker_fail", create_task_worker_fail),
    ("reload_single_process", reload_single_process),
    ("reload_no_task_worker", reload_no_task_worker),
    ("restart_worker_process", restart_worker_process),
    ("reload_thread", reload_thread),
    ("reload_thread_2", reload_thread_2),
    ("reload_thread_3", reload_thread_3),
    ("kill_worker_1", kill_worker_1),
    ("kill_worker_2", kill_worker_2),
    ("kill_worker_3", kill_worker_3),
    ("kill_worker_4", kill_worker_4),
    ("kill_worker_5", kill_worker_5),
    ("kill_worker_6", kill_worker_6),
    ("worker_exit", worker_exit),
    ("shutdown_event", shutdown_event),
    ("kill_self", kill_self),
    ("clean_worker_1", clean_worker_1),
    ("clean_worker_2", clean_worker_2),
    ("reload_all_from_worker_error", reload_all_from_worker_error),
    (
        "reload_tasks_from_worker_error",
        reload_tasks_from_worker_error,
    ),
];
