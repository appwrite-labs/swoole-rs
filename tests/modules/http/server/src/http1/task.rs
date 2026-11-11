//! Upstream's `core-tests/src/server/server.cpp` task-worker TESTs, one Rust function per
//! TEST with the same inputs and expected values, over the crates. `Server::task` and
//! `task_sync` are crates/process's `Tasks` over the server's task pool and this worker's
//! notify pipe, lock and result slot ([`tasks`], as crates/phpserver's `task_state`
//! lends them to `Swoole\Server`'s task methods); `finish`, `task_pack`, `task_unpack`,
//! `task_dump`, `send_to_worker_from_worker`, `kill_worker` and the pool's `dispatch`,
//! main loop and `destroy` are the crates' functions under their upstream symbols.
//!
//! `test::counter_*` is shared memory the forked processes inherit, as core-tests' is.
//! An expectation in a callback records its failure there too, so the case fails on one
//! in a forked worker as well as in this process once the server stops.

use super::*;
use std::sync::atomic::AtomicU32;
use swoole_rs_buffer::string::CppString;
use swoole_rs_corebase::{swoole_random_string, swr_v1_core_set_current_task_id};
use swoole_rs_locks::{swr_v1_locks_lock, swr_v1_locks_unlock};
use swoole_rs_process::methods as pool;
use swoole_rs_process::process_pool::{PoolRecord, PoolRef, SW_IPC_MAX_SIZE};
use swoole_rs_process::task_file::{self, EventHead, Packed, task_pack};
use swoole_rs_process::task_sync::{Bailed, MultiTask, TaskHost, Tasks};
use swoole_rs_process::worker_record::{Worker, WorkerRecord};
use swoole_rs_tls::socket_pair::{self, SocketPair};
use swoole_rs_utilities::swoole_set_log_file;

use case::{TimerNode, swoole_timer_after};

const PACKET: &[u8] = b"hello world\n";
const TEST_STR: &[u8] = b"hello world, hello swoole\n";
const TASK_IPC_UNIXSOCK: u8 = 1;
const TASK_IPC_MSGQUEUE: u8 = 2;
const TASK_IPC_PREEMPTIVE: u8 = 3;
const TASK_IPC_STREAM: u8 = 4;
// `SW_TASK_*`.
const TASK_NONBLOCK: u16 = 1 << 2;
const TASK_CALLBACK: u16 = 1 << 3;
const TASK_COROUTINE: u16 = 1 << 5;
const TASK_NOREPLY: u16 = 1 << 7;
// `SW_PIPE_MASTER`, `SW_PIPE_NONBLOCK`.
const PIPE_MASTER: c_int = 1;
const PIPE_NONBLOCK: c_int = 2;
const SW_SERVER_EVENT_SHUTDOWN: u8 = 16;
/// glibc's value (the case binary is Linux's).
const SIGIO: c_int = 29;
const TEST_LOG_FILE: &str = "/tmp/swoole.log";
const TEST_COUNTER_NUM: usize = 32;
const IPC_MAX_SIZE: usize = SW_IPC_MAX_SIZE as usize;
/// `Lock::MUTEX`, `LOCK_EX`.
const MUTEX: i32 = 3;
const LOCK_EX: i32 = 2;

// Callback slots, in callbacks.rs's order.
const CB_TASK: usize = 4;
const CB_BEFORE_SHUTDOWN: usize = 8;
const CB_MANAGER_START: usize = 10;
const CB_PIPE_MESSAGE: usize = 15;
const CB_FINISH: usize = 20;

/// `EventData`: a head and `SW_IPC_BUFFER_SIZE` bytes.
#[repr(C)]
struct EventData {
    info: DataHead,
    data: [u8; IPC_MAX_SIZE - size_of::<DataHead>()],
}

impl EventData {
    /// `EventData buf{}`.
    fn new() -> Box<Self> {
        // SAFETY: plain integers, all-zero valid.
        Box::new(unsafe { std::mem::zeroed() })
    }

    /// `EventData buf{}` then `Server::task_pack(&buf, data, len)`.
    fn packed(data: &[u8]) -> Box<Self> {
        let mut buf = Self::new();
        // SAFETY: a writable EventData and `data`'s bytes.
        let packed = unsafe { task_pack(buf.head(), data.as_ptr().cast(), data.len()) };
        check(packed, "Server::task_pack(&buf, data, len)");
        buf
    }

    /// `EventData buf; memset(&buf.info, 0, ..); buf.info.len = len; memcpy(buf.data, ..)`.
    fn raw(data: &[u8], ext_flags: u16, reactor_id: i16) -> Box<Self> {
        let mut buf = Self::new();
        buf.info.len = u32::try_from(data.len()).unwrap();
        buf.data[..data.len()].copy_from_slice(data);
        buf.info.ext_flags = ext_flags;
        buf.info.reactor_id = reactor_id;
        buf
    }

    fn head(&mut self) -> *mut EventHead {
        ptr::from_mut(self).cast()
    }

    /// `string(task->data, task->info.len)`.
    fn payload(&self) -> &[u8] {
        &self.data[..self.info.len as usize]
    }

    /// `EventData::size()`.
    fn size(&self) -> usize {
        size_of::<DataHead>() + self.info.len as usize
    }
}

/// The server's task pool, and this worker's notify pipe and lock.
struct TaskPool {
    pool: *mut PoolRecord,
    pipe: *mut SocketPair,
    lock: *mut c_void,
}

impl TaskHost for TaskPool {
    fn dispatch(&mut self, task: *mut u8, worker_id: &mut c_int, blocking: bool) -> bool {
        // SAFETY: the live task pool and a packed task.
        unsafe { pool::dispatch(self.pool, task, worker_id, blocking) == SW_OK }
    }

    fn pipe_clean(&mut self) {
        // SAFETY: this worker's live notify pipe.
        socket_pair::clean(unsafe { &*self.pipe });
    }

    fn set_timeout(&mut self, timeout: f64) {
        // SAFETY: as above.
        socket_pair::set_timeout(unsafe { &mut *self.pipe }, timeout);
    }

    fn read_notify(&mut self) -> isize {
        let mut notify = 0u64;
        // SAFETY: this worker's live notify pipe.
        let pipe = unsafe { &*self.pipe };
        // SAFETY: a local of that size.
        unsafe { socket_pair::read(pipe, (&raw mut notify).cast(), size_of::<u64>()) }
    }

    fn lock(&mut self, on: bool) {
        // SAFETY: this worker's live mutex.
        unsafe {
            if on {
                swr_v1_locks_lock(self.lock, MUTEX, LOCK_EX, -1, 0);
            } else {
                swr_v1_locks_unlock(self.lock, MUTEX);
            }
        }
    }
}

/// `serv`'s task state; for a call that waits (`sync`), with this worker's notify pipe,
/// lock and `task_results` slot.
fn tasks(serv: Server, sync: bool) -> Tasks<'static, TaskPool> {
    let view = serv.view();
    let counters = view.counters();
    let mut host = TaskPool {
        pool: view.task_pool(),
        pipe: ptr::null_mut(),
        lock: ptr::null_mut(),
    };
    let mut result = ptr::null_mut();
    if sync {
        let id = u16::try_from(pool::swoole_get_worker_id()).unwrap();
        host.pipe = view.task_notify_pipe(id);
        // SAFETY: this worker's live record.
        host.lock = unsafe { (*view.worker(id)).lock };
        result = serv.get_task_result().cast();
    }
    Tasks {
        host,
        tasking: &counters.tasking_num,
        count: &counters.task_count,
        result,
    }
}

unsafe extern "C" {
    fn usleep(usec: u32) -> c_int;
    fn mmap(
        addr: *mut c_void,
        len: usize,
        prot: c_int,
        flags: c_int,
        fd: c_int,
        off: i64,
    ) -> *mut c_void;
    #[cfg(target_os = "linux")]
    fn __libc_current_sigrtmin() -> c_int;
}

#[cfg(target_os = "linux")]
const MAP_SHARED_ANONYMOUS: c_int = 0x01 | 0x20;
#[cfg(not(target_os = "linux"))]
const MAP_SHARED_ANONYMOUS: c_int = 0x01 | 0x1000;

/// What the forked processes share: core-tests' counters, whether an expectation
/// failed, and the gate `no_idle_task_worker`'s mutex was.
#[repr(C)]
struct Shared {
    counters: [AtomicI32; TEST_COUNTER_NUM],
    failed: AtomicBool,
    unlocked: AtomicBool,
}

/// The shared memory, mapped once before the first fork.
fn shared() -> &'static Shared {
    static SHARED: OnceLock<usize> = OnceLock::new();
    let at = *SHARED.get_or_init(|| {
        // SAFETY: a fresh anonymous shared mapping, zero-filled, never unmapped.
        let at = unsafe {
            mmap(
                ptr::null_mut(),
                size_of::<Shared>(),
                3,
                MAP_SHARED_ANONYMOUS,
                -1,
                0,
            )
        };
        assert!(!at.is_null() && at as isize != -1, "mmap failed");
        at as usize
    });
    // SAFETY: the mapping above: zero-filled atomics, for the process's life.
    unsafe { &*(at as *const Shared) }
}

/// `test::counter_init()`, and no expectation failed yet.
fn counter_init() {
    let shared = shared();
    for counter in &shared.counters {
        counter.store(0, Ordering::SeqCst);
    }
    shared.failed.store(false, Ordering::SeqCst);
    shared.unlocked.store(false, Ordering::SeqCst);
}

/// `test::counter_incr(index)`: the new value.
fn counter_incr(index: usize) -> i32 {
    shared().counters[index].fetch_add(1, Ordering::SeqCst) + 1
}

fn counter_get(index: usize) -> i32 {
    shared().counters[index].load(Ordering::SeqCst)
}

fn counter_set(index: usize, value: i32) {
    shared().counters[index].store(value, Ordering::SeqCst);
}

/// `EXPECT_*` (or an `ASSERT_*` in a callback): report and record, then carry on.
fn check(held: bool, what: &str) {
    if !held {
        eprintln!("expectation failed: {what}");
        shared().failed.store(true, Ordering::SeqCst);
    }
}

/// Fails the case on an expectation that failed in any of its processes.
fn assert_no_failure() {
    assert!(
        !shared().failed.load(Ordering::SeqCst),
        "a server-side expectation failed"
    );
}

type Deferred = Box<dyn FnOnce()>;

unsafe extern "C" fn fire(_: *mut c_void, node: *mut TimerNode) {
    // SAFETY: the node `after` armed, whose data is its boxed closure, run once.
    let deferred = unsafe { Box::from_raw((*node).data.cast::<Deferred>()) };
    deferred();
}

/// `swoole_timer_after(ms, fn)`.
fn after(ms: c_long, deferred: impl FnOnce() + 'static) {
    let data = Box::into_raw(Box::new(Box::new(deferred) as Deferred));
    // SAFETY: a callback of the timer's signature, which takes `data` back once.
    unsafe { swoole_timer_after(ms, fire, data.cast()) };
}

fn sleep_us(usec: u32) {
    // SAFETY: usleep(3).
    unsafe { usleep(usec) };
}

/// A worker's `id`.
fn worker_id(worker: *mut c_void) -> u32 {
    // SAFETY: the live worker the server passed the callback.
    unsafe { &*worker.cast::<WorkerRecord>() }.id()
}

fn data<'a>(task: *mut c_void) -> &'a EventData {
    // SAFETY: the live EventData the server passed the callback.
    unsafe { &*task.cast::<EventData>() }
}

/// The `Server` methods the task cases call.
trait TaskServer: Copy {
    fn task_pool(self) -> *mut PoolRecord;
    /// `serv->task(&buf, &dst_worker_id)`.
    fn task(self, buf: &mut EventData, dst_worker_id: &mut c_int) -> bool;
    /// `serv->finish(data, len, 0, task)`.
    fn finish(self, data: &[u8], task: *mut c_void) -> bool;
    /// `get_task_worker_pool()->dispatch(&buf, &dst_worker_id)`.
    fn dispatch(self, buf: &mut EventData, dst_worker_id: &mut c_int) -> c_int;
    fn get_tasking_num(self) -> c_int;
    fn get_idle_task_worker_num(self) -> c_int;
    fn tasking_num(self) -> &'static AtomicU32;
    fn task_count(self) -> u64;
    fn shutdown(self) -> bool;
    fn get_worker(self, id: u16) -> *mut WorkerRecord;
    fn get_worker_pid(self, id: u16) -> c_int;
    fn get_manager_pid(self) -> c_int;
    /// `serv->kill_worker(worker_id)`.
    fn kill_worker(self, worker_id: c_int) -> bool;
    /// `serv->send_to_worker_from_worker(id, &ev, flags)`.
    fn send_to_worker_from_worker(self, id: u16, ev: &EventData, flags: c_int) -> isize;
    /// `serv->send_pipe_message(worker_id, data, len)`.
    fn send_pipe_message(self, worker_id: u32, data: &[u8]) -> bool;
    /// `serv->get_task_result()`: this worker's slot of `task_results`.
    fn get_task_result(self) -> *mut EventData;
}

impl TaskServer for Server {
    fn task_pool(self) -> *mut PoolRecord {
        self.view().task_pool()
    }

    fn task(self, buf: &mut EventData, dst_worker_id: &mut c_int) -> bool {
        tasks(self, false)
            .task(buf.head().cast(), dst_worker_id, false)
            .unwrap_or(false)
    }

    fn finish(self, data: &[u8], task: *mut c_void) -> bool {
        // SAFETY: null, or the task being run, which starts with its head.
        let task = unsafe { task.cast::<swoole_rs_corebase::DataHead>().as_ref() };
        swoole_rs_server::task_worker_ops::finish(self.view(), data, 0, task)
    }

    fn dispatch(self, buf: &mut EventData, dst_worker_id: &mut c_int) -> c_int {
        // SAFETY: the created server's task pool and a writable EventData.
        unsafe { pool::dispatch(self.task_pool(), buf.head().cast(), dst_worker_id, false) }
    }

    fn get_tasking_num(self) -> c_int {
        server_methods::get_tasking_num(self.view())
    }

    fn get_idle_task_worker_num(self) -> c_int {
        server_methods::get_idle_task_worker_num(self.view())
    }

    fn tasking_num(self) -> &'static AtomicU32 {
        &self.view().counters().tasking_num
    }

    fn task_count(self) -> u64 {
        self.view().counters().task_count.load(Ordering::SeqCst)
    }

    fn shutdown(self) -> bool {
        server_methods::shutdown(self.view())
    }

    fn get_worker(self, id: u16) -> *mut WorkerRecord {
        self.view().worker(id)
    }

    fn get_worker_pid(self, id: u16) -> c_int {
        // SAFETY: one of the server's live workers.
        unsafe { &*self.get_worker(id) }.pid()
    }

    fn get_manager_pid(self) -> c_int {
        self.view().counters().manager_pid
    }

    fn kill_worker(self, worker_id: c_int) -> bool {
        swoole_rs_server::event_worker_ops::kill_worker(self.view(), worker_id)
    }

    fn send_to_worker_from_worker(self, id: u16, ev: &EventData, flags: c_int) -> isize {
        // SAFETY: one of the server's workers, and the event's bytes.
        unsafe {
            pool::send_pipe_message(
                self.get_worker(id),
                ptr::from_ref(ev).cast(),
                ev.size(),
                flags,
            )
        }
    }

    fn send_pipe_message(self, worker_id: u32, data: &[u8]) -> bool {
        let mut buf = EventData::new();
        // SAFETY: a writable EventData and `data`'s bytes.
        if !unsafe { task_pack(buf.head(), data.as_ptr().cast(), data.len()) } {
            return false;
        }
        // SAFETY: a live server and the packed message.
        unsafe { server_methods::send_pipe_message(self.0, worker_id, buf.head().cast()) }
    }

    fn get_task_result(self) -> *mut EventData {
        let id = pool::swoole_get_worker_id() as usize;
        self.config()
            .task_results
            .cast::<u8>()
            .wrapping_add(id * IPC_MAX_SIZE)
            .cast()
    }
}

/// `onReceive` that only returns `SW_OK`.
unsafe extern "C" fn receive_ok(_: *mut c_void, _: *mut c_void, _: *mut RecvData) -> c_int {
    SW_OK
}

/// `onTask` that only returns 0.
unsafe extern "C" fn task_zero(_: *mut c_void, _: *mut c_void, _: *mut c_void) -> c_int {
    0
}

/// `ListenPort *port = serv.add_port(SW_SOCK_TCP, TEST_HOST, 0)`, or exit 2.
fn listen(serv: Server) {
    if serv.add_port(SW_SOCK_TCP, TEST_HOST, 0).is_none() {
        eprintln!("listen failed, [error={}]", last_error());
        std::process::exit(2);
    }
}

unsafe extern "C" fn task_worker_on_task(
    _: *mut c_void,
    serv: *mut c_void,
    task: *mut c_void,
) -> c_int {
    let serv = Server(serv);
    check(serv.get_tasking_num() == 1, "serv->get_tasking_num() == 1");
    check(
        data(task).payload() == PACKET,
        "string(task->data, task->info.len) == packet",
    );
    // SAFETY: the created server's task pool.
    unsafe { (*serv.task_pool()).running = false };
    serv.view()
        .counters()
        .task_count
        .fetch_add(1, Ordering::SeqCst);
    serv.tasking_num().fetch_sub(1, Ordering::SeqCst);
    0
}

/// `TEST(server, task_worker)`.
fn task_worker() {
    counter_init();
    let serv = Server::new(MODE_BASE);
    serv.config().worker_num = 1;
    serv.config().task_worker_num = 1;

    listen(serv);

    serv.set_callback(CB_TASK, task_worker_on_task as *const c_void);

    assert_eq!(serv.create(), SW_OK);

    let t1 = std::thread::spawn(move || {
        let pool = serv.task_pool();
        // SAFETY: the created server's task pool, its first worker and its main loop.
        unsafe {
            (*pool).running = true;
            let main_loop = (*pool).main_loop.unwrap();
            main_loop(PoolRef::new(pool), Worker::new((*pool).workers()));
        }
        check(serv.get_tasking_num() == 0, "serv.get_tasking_num() == 0");
        serv.tasking_num().fetch_sub(1, Ordering::SeqCst);
        check(serv.get_tasking_num() == 0, "serv.get_tasking_num() == 0");
        check(
            serv.get_idle_task_worker_num() == serv.config().task_worker_num as c_int,
            "serv.get_idle_task_worker_num() == serv.task_worker_num",
        );
    });

    sleep_us(10000);

    let mut buf = EventData::raw(PACKET, TASK_NOREPLY, 0);
    let mut dst_worker_id = 0;

    assert!(serv.task(&mut buf, &mut dst_worker_id));
    assert_eq!(serv.task_count(), 1);

    t1.join().unwrap();
    // The task pool, whose worker loop has ended.
    pool::destroy(serv.view().task_pool_ref());

    assert_eq!(serv.task_count(), 2);
    assert_no_failure();
    serv.delete();
}

unsafe extern "C" fn task_worker2_on_pipe_message(
    _: *mut c_void,
    _: *mut c_void,
    task: *mut c_void,
) {
    check(
        data(task).data.starts_with(TEST_STR),
        "EXPECT_MEMEQ(task->data, TEST_STR, strlen(TEST_STR))",
    );
    counter_incr(7);
}

unsafe extern "C" fn task_worker2_on_worker_start(
    _: *mut c_void,
    serv: *mut c_void,
    worker: *mut c_void,
) {
    let serv = Server(serv);
    if worker_id(worker) == 0 {
        after(50, move || {
            let mut ev = EventData::new();
            ev.info.type_ = SW_SERVER_EVENT_SHUTDOWN;
            ev.info.len = 0;
            check(
                serv.send_to_worker_from_worker(1, &ev, PIPE_MASTER | PIPE_NONBLOCK) > 0,
                "serv->send_to_worker_from_worker(1, &ev, ..) > 0",
            );
        });

        after(60, move || {
            check(
                serv.send_pipe_message(2, TEST_STR),
                "serv->send_pipe_message(2, SW_STRL(TEST_STR))",
            );
        });

        after(70, move || {
            let mut ev = EventData::new();
            ev.info.type_ = SW_SERVER_EVENT_SHUTDOWN + 99;
            ev.info.len = 0;
            check(
                serv.send_to_worker_from_worker(0, &ev, PIPE_MASTER | PIPE_NONBLOCK) > 0,
                "serv->send_to_worker_from_worker(0, &ev, ..) > 0",
            );
        });

        after(100, move || {
            serv.shutdown();
        });
    }
    counter_incr(1);
}

/// `TEST(server, task_worker2)`.
fn task_worker2() {
    let serv = Server::new(MODE_PROCESS);
    serv.config().worker_num = 1;
    serv.config().task_worker_num = 2;
    counter_init();

    assert!(serv.add_port(SW_SOCK_TCP, TEST_HOST, 0).is_some());

    serv.set_callback(CB_TASK, task_zero as *const c_void);
    serv.set_callback(
        CB_PIPE_MESSAGE,
        task_worker2_on_pipe_message as *const c_void,
    );
    serv.set_callback(
        CB_WORKER_START,
        task_worker2_on_worker_start as *const c_void,
    );
    serv.set_callback(CB_RECEIVE, receive_ok as *const c_void);

    assert_eq!(serv.create(), SW_OK);
    assert_eq!(serv.start(), SW_OK);

    assert_eq!(counter_get(1), 4); // onWorkerStart
    assert_eq!(counter_get(7), 1); // onPipeMessage
    assert_no_failure();
    serv.delete();
}

/// `SIGRTMIN`.
#[cfg(target_os = "linux")]
fn sigrtmin() -> c_int {
    // SAFETY: glibc's real-time signal base.
    unsafe { __libc_current_sigrtmin() }
}

#[cfg(not(target_os = "linux"))]
fn sigrtmin() -> c_int {
    unreachable!("the case binary is Linux's")
}

unsafe extern "C" fn task_worker_3_on_manager_start(_: *mut c_void, serv: *mut c_void) {
    let serv = Server(serv);
    after(5000, move || {
        serv.shutdown();
    });
}

unsafe extern "C" fn task_worker_3_on_worker_start(
    _: *mut c_void,
    serv: *mut c_void,
    worker: *mut c_void,
) {
    let serv = Server(serv);
    let id = worker_id(worker);
    counter_incr(2 + id as usize);
    if counter_incr(1) == 5 {
        after(100, move || {
            serv.shutdown();
        });
    }
    if id == 0 {
        after(50, move || {
            let pid = serv.get_worker_pid(2);
            // SAFETY: signals that process.
            if pid <= 0 || unsafe { kill(pid, SIGTERM) } < 0 {
                counter_set(31, 1);
            }
        });
        after(60, move || {
            // SAFETY: signals the manager.
            unsafe { kill(serv.get_manager_pid(), sigrtmin()) };
        });
    }
    if id == 1 && counter_get(30) == 0 {
        counter_set(30, 1);
        after(20, move || {
            serv.kill_worker(-1);
        });
    }
}

/// `TEST(server, task_worker_3)`.
fn task_worker_3() {
    let serv = Server::new(MODE_BASE);
    serv.config().worker_num = 1;
    serv.config().task_worker_num = 2;
    counter_init();

    // A missed worker restart must fail the test instead of leaving the server running
    // until the CI watchdog fires.
    serv.set_callback(
        CB_MANAGER_START,
        task_worker_3_on_manager_start as *const c_void,
    );

    assert!(serv.add_port(SW_SOCK_TCP, TEST_HOST, 0).is_some());

    serv.set_callback(CB_TASK, task_zero as *const c_void);
    serv.set_callback(
        CB_WORKER_START,
        task_worker_3_on_worker_start as *const c_void,
    );
    serv.set_callback(CB_RECEIVE, receive_ok as *const c_void);

    assert_eq!(serv.create(), SW_OK);
    assert_eq!(serv.start(), SW_OK);

    assert_eq!(counter_get(1), 5); // onWorkerStart
    assert_eq!(counter_get(2), 1); // event worker
    assert_eq!(counter_get(3), 2); // task worker 1
    assert_eq!(counter_get(4), 2); // task worker 2
    assert_eq!(counter_get(31), 0); // SIGTERM was sent to task worker 2
    assert_no_failure();
    serv.delete();
}

/// `onTask` of `test_task`, `task_worker3`, `task_worker4` and `test_task_ipc`: the
/// packet, finished with itself.
unsafe extern "C" fn finish_packet(_: *mut c_void, serv: *mut c_void, task: *mut c_void) -> c_int {
    let serv = Server(serv);
    let payload = data(task).payload();
    check(
        payload == PACKET,
        "string(task->data, task->info.len) == packet",
    );
    check(
        serv.finish(payload, task),
        "serv->finish(task->data, task->info.len, 0, task)",
    );
    0
}

/// `onFinish` of `test_task`, `task_worker3` and `task_worker4`: the packet back.
unsafe extern "C" fn finished_packet(_: *mut c_void, _: *mut c_void, task: *mut c_void) -> c_int {
    check(
        data(task).payload() == PACKET,
        "string(task->data, task->info.len) == packet",
    );
    0
}

unsafe extern "C" fn test_task_on_worker_start(
    _: *mut c_void,
    serv: *mut c_void,
    worker: *mut c_void,
) {
    let serv = Server(serv);
    let id = worker_id(worker);
    if id == 1 {
        let mut dst_worker_id = 0;
        let mut buf = EventData::raw(
            PACKET,
            TASK_NONBLOCK | TASK_CALLBACK,
            i16::try_from(id).unwrap(),
        );
        check(
            serv.task(&mut buf, &mut dst_worker_id),
            "serv->task(&buf, &_dst_worker_id)",
        );
        sleep(1.0);
        serv.shutdown();
    }
}

/// `test_task(mode, task_ipc_mode)`.
fn test_task(mode: c_int, task_ipc_mode: u8) {
    counter_init();
    let serv = Server::new(mode);
    serv.config().worker_num = 2;
    serv.config().task_ipc_mode = task_ipc_mode;
    serv.config().task_worker_num = 3;

    listen(serv);

    serv.set_callback(CB_RECEIVE, receive_ok as *const c_void);
    serv.set_callback(CB_TASK, finish_packet as *const c_void);
    serv.set_callback(CB_FINISH, finished_packet as *const c_void);

    assert_eq!(serv.create(), SW_OK);

    serv.set_callback(CB_WORKER_START, test_task_on_worker_start as *const c_void);

    assert_eq!(serv.start(), 0);
    assert_no_failure();
    serv.delete();
}

/// `TEST(server, task_base)`.
fn task_base() {
    test_task(MODE_BASE, TASK_IPC_UNIXSOCK);
}

/// `TEST(server, task_process)`.
fn task_process() {
    test_task(MODE_PROCESS, TASK_IPC_UNIXSOCK);
}

/// `TEST(server, task_ipc_stream)`.
fn task_ipc_stream() {
    test_task(MODE_PROCESS, TASK_IPC_STREAM);
}

/// `task_worker3`'s and `task_worker4`'s Server: two workers, three task workers with
/// `task_enable_coroutine`, created, its `onWorkerStart` set after.
fn coroutine_task_server(on_worker_start: *const c_void) -> Server {
    counter_init();
    let serv = Server::new(MODE_PROCESS);
    serv.config().worker_num = 2;
    serv.config().task_worker_num = 3;
    serv.config().task_enable_coroutine = true;

    listen(serv);

    serv.set_callback(CB_RECEIVE, receive_ok as *const c_void);
    serv.set_callback(CB_TASK, finish_packet as *const c_void);
    serv.set_callback(CB_FINISH, finished_packet as *const c_void);

    assert_eq!(serv.create(), SW_OK);

    serv.set_callback(CB_WORKER_START, on_worker_start);
    serv
}

unsafe extern "C" fn task_worker3_on_worker_start(
    _: *mut c_void,
    serv: *mut c_void,
    worker: *mut c_void,
) {
    let serv = Server(serv);
    let id = worker_id(worker);
    if id == 1 {
        let mut dst_worker_id = 0;
        let mut buf = EventData::raw(
            PACKET,
            TASK_NONBLOCK | TASK_COROUTINE,
            i16::try_from(id).unwrap(),
        );
        serv.dispatch(&mut buf, &mut dst_worker_id);
        sleep(1.0);
        kill_pid(serv.get_master_pid());
    }
}

/// `TEST(server, task_worker3)`.
fn task_worker3() {
    let serv = coroutine_task_server(task_worker3_on_worker_start as *const c_void);
    assert_eq!(serv.start(), 0);
    assert_no_failure();
    serv.delete();
}

unsafe extern "C" fn task_worker4_on_worker_start(
    _: *mut c_void,
    serv: *mut c_void,
    worker: *mut c_void,
) {
    let serv = Server(serv);
    let id = worker_id(worker);
    if id == 1 {
        let mut dst_worker_id = 0;
        let reactor_id = i16::try_from(id).unwrap();
        let mut buf = EventData::raw(PACKET, TASK_NONBLOCK | TASK_COROUTINE, reactor_id);
        serv.dispatch(&mut buf, &mut dst_worker_id);
        sleep(1.0);

        // SAFETY: this worker's result slot, an EventData.
        unsafe { serv.get_task_result().write_bytes(0, 1) };
        let mut buf = EventData::raw(PACKET, 0, reactor_id);
        serv.tasking_num().fetch_add(1, Ordering::SeqCst);
        serv.dispatch(&mut buf, &mut dst_worker_id);
        serv.tasking_num().fetch_add(0, Ordering::SeqCst);
        kill_pid(serv.get_master_pid());
    }
}

/// `TEST(server, task_worker4)`.
fn task_worker4() {
    let serv = coroutine_task_server(task_worker4_on_worker_start as *const c_void);
    assert_eq!(serv.start(), 0);
    assert_no_failure();
    serv.delete();
}

/// `Server::task_unpack(task, &buffer, &packet)` into a 32 KiB `String`: the bytes.
fn unpack(task: Packed<'_>) -> Option<Vec<u8>> {
    let mut buffer = CppString::EMPTY;
    buffer.alloc(32 * 1024, None);
    let unpacked = task.unpack(&mut buffer);
    let bytes = unpacked.map(|(data, length, _)| {
        // SAFETY: the unpacked payload, live while `buffer` or the task is.
        unsafe { std::slice::from_raw_parts(data.cast::<u8>(), length) }.to_vec()
    });
    buffer.dispose();
    bytes
}

unsafe extern "C" fn task_sync_multi_on_task(
    _: *mut c_void,
    serv: *mut c_void,
    task: *mut c_void,
) -> c_int {
    let Some(packet) = unpack(unsafe { Packed::from_raw(task) }) else {
        return -1;
    };
    // SAFETY: the live task.
    unsafe { &*task.cast::<task_file::EventData>() }.dump();
    check(
        Server(serv).finish(&packet, task),
        "serv->finish(packet.data, packet.length, 0, task)",
    );
    0
}

/// `task_sync_multi_task`'s tasks and results, which its worker inherits.
static MULTI: Mutex<MultiTasks> = Mutex::new(MultiTasks {
    tasks: Vec::new(),
    results: Vec::new(),
});

/// `task_sync_multi_task`'s `tasks` and `results`.
struct MultiTasks {
    tasks: Vec<Vec<u8>>,
    results: Vec<Vec<u8>>,
}

/// `Server::MultiTask mt(tasks.size())` and its `pack`, `unpack` and `fail`.
struct Multi<'a> {
    tasks: &'a [Vec<u8>],
    results: &'a mut [Vec<u8>],
}

impl MultiTask for Multi<'_> {
    fn count(&self) -> u16 {
        u16::try_from(self.tasks.len()).unwrap()
    }

    fn pack(&mut self, index: u16, event: *mut u8) -> Result<i64, Bailed> {
        let task = &self.tasks[usize::from(index)];
        // SAFETY: a writable EventData and the task's bytes.
        if unsafe { task_pack(event.cast(), task.as_ptr().cast(), task.len()) } {
            // SAFETY: as above, now packed: `info.fd` heads it.
            Ok(unsafe { event.cast::<DataHead>().read_unaligned() }.fd)
        } else {
            Ok(-1)
        }
    }

    fn unpack(&mut self, index: u16, result: Packed<'_>) -> Result<(), Bailed> {
        if let Some(packet) = unpack(result) {
            self.results[usize::from(index)] = packet;
        }
        Ok(())
    }

    fn fail(&mut self, index: u16) {
        eprintln!("task failed: {index}");
    }
}

unsafe extern "C" fn task_sync_multi_on_worker_start(
    _: *mut c_void,
    serv: *mut c_void,
    worker: *mut c_void,
) {
    let serv = Server(serv);
    if worker_id(worker) == 1 {
        let mut guard = MULTI.lock().unwrap();
        let MultiTasks {
            tasks: all,
            results,
        } = &mut *guard;
        let mut mt = Multi {
            tasks: all,
            results,
        };

        check(
            tasks(serv, true)
                .task_sync_many(&mut mt, 10.0)
                .unwrap_or(false),
            "serv->task_sync(mt, 10)",
        );

        for (i, (task, result)) in all.iter().zip(results.iter()).enumerate() {
            check(task == result, &format!("tasks[{i}] == results[{i}]"));
        }

        kill_pid(serv.get_master_pid());
    }
}

/// `TEST(server, task_sync_multi_task)`.
fn task_sync_multi_task() {
    counter_init();
    let serv = Server::new(MODE_PROCESS);
    serv.config().worker_num = 2;
    serv.config().task_worker_num = 3;

    let n_task = 16;
    let len_task = IPC_MAX_SIZE * 2;
    {
        let mut multi = MULTI.lock().unwrap();
        let MultiTasks { tasks, results } = &mut *multi;
        for _ in 0..n_task {
            let mut data = vec![0u8; len_task];
            // SAFETY: `len_task - 1` characters and their NUL.
            unsafe { swoole_random_string(data.as_mut_ptr().cast(), len_task - 1) };
            data.truncate(len_task - 1);
            tasks.push(data);
        }
        results.resize(n_task, Vec::new());
    }

    listen(serv);

    serv.set_callback(CB_RECEIVE, receive_ok as *const c_void);
    serv.set_callback(CB_TASK, task_sync_multi_on_task as *const c_void);

    assert_eq!(serv.create(), SW_OK);
    // `SwooleG.current_task_id = 100`.
    swr_v1_core_set_current_task_id(100);

    serv.set_callback(
        CB_WORKER_START,
        task_sync_multi_on_worker_start as *const c_void,
    );

    assert_eq!(serv.start(), 0);
    assert_no_failure();
    serv.delete();
}

unsafe extern "C" fn task_sync_on_task(
    _: *mut c_void,
    serv: *mut c_void,
    task: *mut c_void,
) -> c_int {
    let payload = data(task).payload();
    check(
        payload == PACKET,
        "string(task->data, task->info.len) == packet",
    );
    // SAFETY: the live task.
    unsafe { &*task.cast::<task_file::EventData>() }.dump();
    check(
        Server(serv).finish(payload, task),
        "serv->finish(task->data, task->info.len, 0, task)",
    );
    0
}

unsafe extern "C" fn task_sync_on_worker_start(
    _: *mut c_void,
    serv: *mut c_void,
    worker: *mut c_void,
) {
    let serv = Server(serv);
    if worker_id(worker) == 1 {
        let mut dst_worker_id = -1;
        let mut buf = EventData::packed(PACKET);
        check(
            tasks(serv, true)
                .task_sync(buf.head().cast(), &mut dst_worker_id, 0.5)
                .unwrap_or(false),
            "serv->task_sync(&buf, &_dst_worker_id, 0.5)",
        );
        // SAFETY: this worker's result slot, an EventData.
        let task_result = unsafe { &*serv.get_task_result() };
        check(
            task_result.payload() == PACKET,
            "string(task_result->data, task_result->info.len) == packet",
        );
        kill_pid(serv.get_master_pid());
    }
}

/// `TEST(server, task_sync)`.
fn task_sync() {
    counter_init();
    let serv = Server::new(MODE_PROCESS);
    serv.config().worker_num = 2;
    serv.config().task_worker_num = 2;

    listen(serv);

    serv.set_callback(CB_RECEIVE, receive_ok as *const c_void);
    serv.set_callback(CB_TASK, task_sync_on_task as *const c_void);

    assert_eq!(serv.create(), SW_OK);

    serv.set_callback(CB_WORKER_START, task_sync_on_worker_start as *const c_void);

    assert_eq!(serv.start(), 0);
    assert_no_failure();
    serv.delete();
}

/// `test_task_ipc`'s `onFinish`: the packet back, then a shutdown.
unsafe extern "C" fn task_ipc_on_finish(
    _: *mut c_void,
    serv: *mut c_void,
    task: *mut c_void,
) -> c_int {
    check(
        data(task).payload() == PACKET,
        "string(task->data, task->info.len) == packet",
    );
    sleep_us(100_000);
    Server(serv).shutdown();
    0
}

unsafe extern "C" fn task_ipc_on_worker_start(
    _: *mut c_void,
    serv: *mut c_void,
    worker: *mut c_void,
) {
    let serv = Server(serv);
    if worker_id(worker) == 1 {
        let mut dst_worker_id = -1;
        let mut buf = EventData::packed(PACKET);
        buf.info.ext_flags |= TASK_NONBLOCK | TASK_CALLBACK;
        check(
            serv.task(&mut buf, &mut dst_worker_id),
            "serv->task(&buf, &_dst_worker_id)",
        );
    }
}

/// `test_task_ipc(serv)`.
fn test_task_ipc(serv: Server) {
    listen(serv);

    serv.set_callback(CB_RECEIVE, receive_ok as *const c_void);
    serv.set_callback(CB_TASK, finish_packet as *const c_void);
    serv.set_callback(CB_FINISH, task_ipc_on_finish as *const c_void);

    assert_eq!(serv.create(), SW_OK);

    serv.set_callback(CB_WORKER_START, task_ipc_on_worker_start as *const c_void);
    serv.set_callback(CB_BEFORE_SHUTDOWN, ignore_server as *const c_void);

    assert_eq!(serv.start(), 0);
    assert_no_failure();
    serv.delete();
}

/// The `task_ipc_queue_*` Servers: two workers and two task workers.
fn task_ipc_server(mode: c_int, task_ipc_mode: u8) -> Server {
    counter_init();
    let serv = Server::new(mode);
    serv.config().worker_num = 2;
    serv.config().task_worker_num = 2;
    serv.config().task_ipc_mode = task_ipc_mode;
    serv
}

/// `TEST(server, task_ipc_queue_1)`.
fn task_ipc_queue_1() {
    test_task_ipc(task_ipc_server(MODE_PROCESS, TASK_IPC_MSGQUEUE));
}

/// `TEST(server, task_ipc_queue_2)`.
fn task_ipc_queue_2() {
    test_task_ipc(task_ipc_server(MODE_PROCESS, TASK_IPC_PREEMPTIVE));
}

/// `TEST(server, task_ipc_queue_3)`.
fn task_ipc_queue_3() {
    test_task_ipc(task_ipc_server(MODE_PROCESS, TASK_IPC_STREAM));
}

/// `TEST(server, task_ipc_queue_4)`.
fn task_ipc_queue_4() {
    test_task_ipc(task_ipc_server(MODE_BASE, TASK_IPC_MSGQUEUE));
}

/// `TEST(server, task_ipc_queue_5)`: its threads start as `worker_thread_start`'s default (`fn()`).
fn task_ipc_queue_5() {
    let serv = task_ipc_server(MODE_THREAD, TASK_IPC_MSGQUEUE);

    wait_all_child_processes();

    test_task_ipc(serv);
}

/// `ListenPort::get_connection_num()` of the primary port.
fn primary_port_connection_num(serv: Server) -> usize {
    // SAFETY: the running server's primary port.
    serv.view().port_connection_num(
        unsafe { swoole_rs_ffi::view::Port::from_raw(serv.get_primary_port().0) }.unwrap(),
    )
}

/// `serv.get_event_worker_pool()->read_message`: the event pool follows the task pool,
/// as crates/server's `master_ops` lays them out. The master's SIGIO handler sets it,
/// so it is read anew each time.
fn event_pool_read_message(serv: Server) -> bool {
    let pool = serv.task_pool().wrapping_add(1);
    // SAFETY: the running server's event pool, in its shared counters.
    unsafe { ptr::read_volatile(&raw const (*pool).read_message) }
}

/// `onStart` of `thread` and `task_thread`: unlocks the client thread's `lock`.
unsafe extern "C" fn unlock_on_start(_: *mut c_void, _: *mut c_void) {
    shared().unlocked.store(true, Ordering::SeqCst);
}

/// The client thread's `lock.lock()`, which `onStart` unlocks.
fn lock_wait() {
    while !shared().unlocked.load(Ordering::SeqCst) {
        sleep_us(1000);
    }
}

/// `string("Server: ") + string(packet)`.
fn server_response() -> Vec<u8> {
    [b"Server: ".as_slice(), PACKET].concat()
}

unsafe extern "C" fn thread_on_worker_start(
    _: *mut c_void,
    serv: *mut c_void,
    worker: *mut c_void,
) {
    Server(serv).send_pipe_message(1 - worker_id(worker), TEST_STR);
}

unsafe extern "C" fn thread_on_receive(
    _: *mut c_void,
    serv: *mut c_void,
    req: *mut RecvData,
) -> c_int {
    let serv = Server(serv);
    let (session, data) = request(req);
    check(data == PACKET, "string(req->data, req->info.len) == packet");

    serv.send(session, &server_response());

    check(
        serv.view().connection_num() == 1,
        "serv->get_connection_num() == 1",
    );
    check(
        primary_port_connection_num(serv) == 1,
        "serv->get_primary_port()->get_connection_num() == 1",
    );
    SW_OK
}

unsafe extern "C" fn thread_on_pipe_message(_: *mut c_void, _: *mut c_void, _: *mut c_void) {
    counter_incr(4);
}

/// `TEST(server, thread)`: each worker thread sends the other a pipe message, a client
/// gets its packet back, and SIGIO to the master sets the event pool's `read_message`.
/// Its trace logging is left out.
fn thread() {
    let serv = Server::new(MODE_THREAD);
    serv.config().worker_num = 2;

    counter_init();

    let port = serv.add_port(SW_SOCK_TCP, TEST_HOST, 0);
    assert!(port.is_some());
    let port = port.unwrap().port();

    assert_eq!(serv.create(), SW_OK);

    client_thread(
        move || serv.get_master_pid(),
        move || {
            block_all_signals();

            lock_wait();

            sleep_us(1000);

            let mut c = std::net::TcpStream::connect((TEST_HOST, port)).unwrap();
            assert_eq!(std::io::Write::write(&mut c, PACKET).unwrap(), PACKET.len());
            let mut buf = [0u8; 1024];
            assert_eq!(
                std::io::Read::read(&mut c, &mut buf).unwrap(),
                PACKET.len() + 8
            );
            let resp = server_response();
            assert_eq!(&buf[..resp.len()], resp.as_slice());
            drop(c);

            sleep_us(1000);

            assert!(!event_pool_read_message(serv));
            // SAFETY: signals the master, this process.
            unsafe { kill(serv.get_master_pid(), SIGIO) };
            sleep_us(1000);
            assert!(event_pool_read_message(serv));

            serv.shutdown();
        },
    );

    serv.set_callback(CB_START, unlock_on_start as *const c_void);
    serv.set_callback(CB_WORKER_START, thread_on_worker_start as *const c_void);
    serv.set_callback(CB_RECEIVE, thread_on_receive as *const c_void);
    serv.set_callback(CB_PIPE_MESSAGE, thread_on_pipe_message as *const c_void);

    assert_eq!(serv.start(), SW_OK);
    join_client();

    wait_all_child_processes();
    assert_eq!(counter_get(4), 2); // onPipeMessage called
    assert_no_failure();
    serv.delete();
}

/// `task_thread`'s `onWorkerStart`: its `std::atomic<int> count` is counter 1.
unsafe extern "C" fn task_thread_on_worker_start(_: *mut c_void, _: *mut c_void, _: *mut c_void) {
    counter_incr(1);
}

unsafe extern "C" fn task_thread_on_finish(
    _: *mut c_void,
    serv: *mut c_void,
    task: *mut c_void,
) -> c_int {
    let payload = data(task).payload();
    let client_fd = c_long::from_ne_bytes(payload[..size_of::<c_long>()].try_into().unwrap());
    check(
        Server(serv).send(client_fd, &server_response()),
        "serv->send(client_fd, resp)",
    );
    0
}

unsafe extern "C" fn task_thread_on_task(
    _: *mut c_void,
    serv: *mut c_void,
    task: *mut c_void,
) -> c_int {
    check(
        Server(serv).finish(data(task).payload(), task),
        "serv->finish(task->data, task->info.len, 0, task)",
    );
    0
}

unsafe extern "C" fn task_thread_on_receive(
    _: *mut c_void,
    serv: *mut c_void,
    req: *mut RecvData,
) -> c_int {
    let (client_fd, data) = request(req);
    check(data == PACKET, "string(req->data, req->info.len) == packet");

    let mut msg = EventData::packed(&client_fd.to_ne_bytes());
    msg.info.ext_flags |= TASK_NONBLOCK;

    let mut dst_worker_id = -1;
    check(
        Server(serv).task(&mut msg, &mut dst_worker_id),
        "serv->task(&msg, &dst_worker_id)",
    );
    SW_OK
}

/// `TEST(server, task_thread)`: a worker thread hands the client's session to a task
/// thread, and the worker's `onFinish` answers the client.
fn task_thread() {
    let serv = Server::new(MODE_THREAD);
    serv.config().worker_num = 2;
    serv.config().task_worker_num = 2;

    counter_init();
    case::log_level(case::SW_LOG_INFO);

    let port = serv.add_port(SW_SOCK_TCP, TEST_HOST, 0);
    assert!(port.is_some());
    let port = port.unwrap().port();

    assert_eq!(serv.create(), SW_OK);

    client_thread(
        move || serv.get_master_pid(),
        move || {
            block_all_signals();

            lock_wait();

            let mut c = std::net::TcpStream::connect((TEST_HOST, port)).unwrap();
            let _ = std::io::Write::write(&mut c, PACKET);
            let mut buf = [0u8; 1024];
            let _ = std::io::Read::read(&mut c, &mut buf);
            drop(c);

            sleep_us(100_000);
            serv.shutdown();
        },
    );

    serv.set_callback(CB_START, unlock_on_start as *const c_void);
    serv.set_callback(
        CB_WORKER_START,
        task_thread_on_worker_start as *const c_void,
    );
    serv.set_callback(CB_FINISH, task_thread_on_finish as *const c_void);
    serv.set_callback(CB_TASK, task_thread_on_task as *const c_void);
    serv.set_callback(CB_RECEIVE, task_thread_on_receive as *const c_void);

    assert_eq!(serv.start(), SW_OK);
    join_client();

    // `serv.get_core_worker_num()`: the workers and the task workers.
    let core_worker_num = serv.config().worker_num + serv.config().task_worker_num;
    assert_eq!(counter_get(1), i32::try_from(core_worker_num).unwrap());
    wait_all_child_processes();
    assert_no_failure();
    serv.delete();
}

static NO_IDLE_CLIENT: Mutex<Option<JoinHandle<()>>> = Mutex::new(None);

/// `no_idle_task_worker`'s `onStart`: the client thread, once a worker has unlocked
/// the gate (upstream's process-shared mutex).
unsafe extern "C" fn no_idle_on_start(_: *mut c_void, serv: *mut c_void) {
    let serv = Server(serv);
    let t1 = std::thread::spawn(move || {
        block_all_signals();

        while !shared().unlocked.load(Ordering::SeqCst) {
            sleep_us(1000);
        }

        let port = serv.get_primary_port().port();

        let mut c = std::net::TcpStream::connect((TEST_HOST, port)).unwrap();
        std::io::Write::write_all(&mut c, PACKET).unwrap();

        // The event worker can queue all 1024 tasks before a task worker has marked itself
        // busy, so where upstream sleeps 3 s this sends another packet each second until the
        // master's once-a-second timer has logged the warning, for up to 10 s.
        for _ in 0..10 {
            sleep(1.0);
            if read_text(TEST_LOG_FILE)
                .is_some_and(|log| log.contains("No idle task worker is available"))
            {
                break;
            }
            std::io::Write::write_all(&mut c, PACKET).unwrap();
        }
        drop(c);

        kill_pid(serv.get_master_pid());
    });
    *NO_IDLE_CLIENT.lock().unwrap() = Some(t1);
}

unsafe extern "C" fn no_idle_on_worker_start(_: *mut c_void, _: *mut c_void, _: *mut c_void) {
    shared().unlocked.store(true, Ordering::SeqCst);
}

unsafe extern "C" fn no_idle_on_receive(
    _: *mut c_void,
    serv: *mut c_void,
    _: *mut RecvData,
) -> c_int {
    let serv = Server(serv);
    for _ in 0..1024 {
        let mut dst_worker_id = -1;
        let mut buf = EventData::packed(PACKET);
        buf.info.ext_flags |= TASK_NONBLOCK | TASK_CALLBACK;
        check(
            serv.task(&mut buf, &mut dst_worker_id),
            "serv->task(&buf, &_dst_worker_id)",
        );
    }
    SW_OK
}

unsafe extern "C" fn no_idle_on_task(_: *mut c_void, _: *mut c_void, task: *mut c_void) -> c_int {
    check(
        data(task).payload() == PACKET,
        "string(task->data, task->info.len) == packet",
    );
    sleep_us(10000);
    0
}

/// `TEST(server, no_idle_task_worker)`.
fn no_idle_task_worker() {
    counter_init();
    let serv = Server::new(MODE_PROCESS);
    serv.config().worker_num = 1;
    serv.config().task_worker_num = 4;
    serv.config().dispatch_mode = DISPATCH_IDLE_WORKER;

    let log_file = CString::new(TEST_LOG_FILE).unwrap();
    // SAFETY: a NUL-terminated path.
    unsafe { swoole_set_log_file(log_file.as_ptr()) };
    set_log_warning();

    listen(serv);

    assert_eq!(serv.create(), SW_OK);

    serv.set_callback(CB_START, no_idle_on_start as *const c_void);
    serv.set_callback(CB_WORKER_START, no_idle_on_worker_start as *const c_void);
    serv.set_callback(CB_RECEIVE, no_idle_on_receive as *const c_void);
    serv.set_callback(CB_TASK, no_idle_on_task as *const c_void);

    assert_eq!(serv.start(), 0);

    let t1 = NO_IDLE_CLIENT.lock().unwrap().take().expect("onStart ran");
    t1.join().unwrap();

    let log = read_text(TEST_LOG_FILE).unwrap_or_default();
    assert!(log.contains("No idle task worker is available"));

    let _ = std::fs::remove_file(TEST_LOG_FILE);
    assert_no_failure();
    serv.delete();
}

pub const CASES: &[(&str, fn())] = &[
    ("task_worker", task_worker),
    ("task_worker2", task_worker2),
    ("task_worker_3", task_worker_3),
    ("task_base", task_base),
    ("task_process", task_process),
    ("task_ipc_stream", task_ipc_stream),
    ("task_worker3", task_worker3),
    ("task_worker4", task_worker4),
    ("task_sync_multi_task", task_sync_multi_task),
    ("task_sync", task_sync),
    ("task_ipc_queue_1", task_ipc_queue_1),
    ("task_ipc_queue_2", task_ipc_queue_2),
    ("task_ipc_queue_3", task_ipc_queue_3),
    ("task_ipc_queue_4", task_ipc_queue_4),
    ("task_ipc_queue_5", task_ipc_queue_5),
    ("no_idle_task_worker", no_idle_task_worker),
    ("thread", thread),
    ("task_thread", task_thread),
];
