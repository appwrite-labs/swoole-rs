//! Upstream's `core-tests/src/server/server.cpp` TESTs that reload, kill and restart a
//! started Server's workers, one Rust function per TEST with the same inputs and expected
//! values. The callbacks are upstream's lambdas; what they captured is a static here, and
//! `test::counter_*` is shared memory every forked process sees, as core-tests' is.

use super::*;
use case::TimerNode;
use std::sync::atomic::{AtomicPtr, AtomicU64};
use swoole_rs_process::methods::{ExitStatus, swoole_get_worker_type};
use swoole_rs_process::worker_record::{Worker, WorkerRecord};
use swoole_rs_process::workers::entry;
use swoole_rs_server::callbacks as cb;
use swoole_rs_server::worker_methods;

const SW_ERROR_OPERATION_NOT_SUPPORT: c_int = 507;
const SW_ERROR_WRONG_OPERATION: c_int = 509;
const TASK_IPC_MSGQUEUE: u8 = 2;
const SW_TASK_WORKER: c_int = 4;
// glibc's values (the core-tests build is Linux).
const SIGUSR1: c_int = 10;
const SIGUSR2: c_int = 12;
const PROT_READ_WRITE: c_int = 3;
const MAP_SHARED_ANONYMOUS: c_int = 0x21;
const TEST_COUNTER_NUM: usize = 32;

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
                size_of::<AtomicI32>() * TEST_COUNTER_NUM,
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
    // SAFETY: the mapping holds TEST_COUNTER_NUM zeroed counters, never unmapped.
    unsafe { std::slice::from_raw_parts(counters, TEST_COUNTER_NUM) }
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

/// `ASSERT_*` in a callback: as [`expect`], and the callback returns when it failed.
fn check(held: bool, what: &str) -> bool {
    expect(held, what);
    held
}

/// Fails the case on an expectation a callback in this process recorded.
fn assert_no_failure() {
    assert!(
        !FAILED.load(Ordering::SeqCst),
        "a server-side expectation failed"
    );
}

fn worker_id(worker: *mut WorkerRecord) -> u32 {
    // SAFETY: the worker the server passed the callback.
    unsafe { Worker::new(worker) }.id()
}

fn manager_pid(serv: Server) -> c_int {
    serv.view().counters().manager_pid
}

fn master_pid(serv: Server) -> c_int {
    serv.view().counters().master_pid
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
    // SAFETY: the running server.
    unsafe { entry::reload(serv.0, reload_all_workers) }
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
fn timer_tick(ms: c_long, callback: TimerFn, serv: Server) {
    // SAFETY: as above.
    unsafe { case::swoole_timer_add(ms, true, callback, serv.0) };
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
    let id = unsafe { worker_methods::add_worker(serv.0, worker) };
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
    serv.delete();
}

/// `TEST(server, reload_all_workers2)`: the master is signalled.
fn reload_all_workers2() {
    let serv = reload_server(false);
    serv.set_callback(CB_WORKER_START, reload_all_workers2_start as *const c_void);

    assert_eq!(serv.start(), 0);
    assert_eq!(counter_get(10), 2); // onBeforeReload called
    assert_eq!(counter_get(11), 2); // onAfterReload called
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
    serv.delete();
}

unsafe extern "C" fn count_worker_start(_: *mut c_void, _: *mut c_void, _: *mut WorkerRecord) {
    counter_incr(1, 1);
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
        (*pool.add(1)).workers = serv.config().workers.cast();
        Worker::new(worker).set_pid(pid());
        swoole_rs_process::methods::add_worker(pool, worker);
        entry::restart_worker_process(serv.0, (&raw const fake_exit).cast());
    }

    wait_all_child_processes();

    assert_eq!(counter_get(1), 1);
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

pub const CASES: &[(&str, fn())] = &[
    ("reload_all_workers", reload_all_workers),
    ("reload_all_workers2", reload_all_workers2),
    ("kill_user_workers", kill_user_workers),
    ("force_kill_all_workers", force_kill_all_workers),
    ("kill_user_workers1", kill_user_workers1),
    ("create_task_worker_fail", create_task_worker_fail),
    ("reload_single_process", reload_single_process),
    ("reload_no_task_worker", reload_no_task_worker),
    ("restart_worker_process", restart_worker_process),
    ("reload_all_from_worker_error", reload_all_from_worker_error),
    (
        "reload_tasks_from_worker_error",
        reload_tasks_from_worker_error,
    ),
];
