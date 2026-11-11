//! The objects `ProcessPool` runs over (once `src/os/process_pool.cc`): its worker pipes, message box, message bus,
//! timers, event loop and callbacks. crates/process decides every step
//! (`process_pool.rs`, `pool.rs`) and asks for each effect through the
//! [`PoolNative`] side [`swr_v1_process_pool_native`] returns. It lives here because these objects are
//! crates/reactor's, crates/tls's and crates/memory's, which crates/process can't
//! depend on.
//!
//! Each hook that can reach PHP runs inside the bailout trap crates/process sets
//! up; the frames here hold nothing to drop, so the `longjmp` may cross them.
#![expect(
    clippy::cast_possible_truncation,
    clippy::cast_possible_wrap,
    reason = "upstream's C conversions between ssize_t, size_t, uint32_t and int"
)]
use core::cell::{Cell, UnsafeCell};
use core::ffi::{CStr, c_int, c_long, c_void};
use core::ptr;
use swoole_rs_process::worker_record::Worker as Record;

use crate::objects::socket;
use core::sync::atomic::AtomicU64;
use swoole_rs_bus::{BUS_ALLOCATE, BusRead, MessageBus};
use swoole_rs_corebase::CoreAllocator;
use swoole_rs_corebase::DataHead;
use swoole_rs_corebase::constants::SW_IPC_MAX_SIZE;
use swoole_rs_corebase::log;
use swoole_rs_memory::channel::Record as ChannelRecord;
use swoole_rs_memory::members::{Shm, pool as mem_pool, sw_shm_free};
use swoole_rs_memory::{
    swr_v1_memory_channel_bytes, swr_v1_memory_channel_close, swr_v1_memory_channel_init,
};
use swoole_rs_process::methods::{
    ExitStatus, is_worker_running, kill_all_workers, pop_message, push_message, reload_warn,
    swoole_set_worker_id, swoole_set_worker_pid, swoole_set_worker_type, worker_report_error,
};
use swoole_rs_process::pool::{
    PoolCall, WARN_ACCEPT, WARN_BAD_PACKET, WARN_MSGRCV, WARN_PIPE, WARN_READ, WARN_SEND,
    WARN_TASK_FAILED, pool_ipc_packet, raise_bailout, swr_v1_process_pool_ipc_listener,
};
use swoole_rs_process::process_pool::{
    CB_BEFORE_RELOAD, CB_MAIN_LOOP, CB_SHUTDOWN, CB_START, CB_WORKER_START, CB_WORKER_STOP,
    OP_ADD_PIPE, OP_ALLOC_WORKERS, OP_BOX_PUSH, OP_BUS_BUFFER, OP_DONT_RESTART, OP_EVENT_AVAILABLE,
    OP_EVENT_INIT, OP_EVENT_WAIT, OP_EXIT_CALLBACK, OP_FREE_MESSAGE_BOX, OP_FREE_MESSAGE_BUS,
    OP_FREE_PIPES, OP_FREE_WORKERS, OP_HAVE_MSGQUEUE, OP_KILL_TIMER, OP_MAIN_LOOP,
    OP_NEW_MESSAGE_BOX, OP_NEW_MESSAGE_BUS, OP_NEW_PIPE, OP_NEW_PIPES, OP_ON_AFTER_RELOAD,
    OP_ON_WORKER_MESSAGE, OP_ON_WORKER_NOT_FOUND, OP_PIPE_HANDLER, OP_POP_MESSAGE, OP_PUSH_STOP,
    OP_RELOAD_TIMER, OP_REMOVE_PIPE, OP_REOPEN_LOGGER, OP_SNDBUF, OP_TIMER_DEL, OP_TIMER_FREE,
    OP_TIMER_INIT, OP_TIMER_SELECT, OP_UNLIMIT_PIPES, OP_WAITPID, OP_WORKER_ERROR, OP_WORKER_INIT,
    OP_WORKER_SHUTDOWN, PoolNative, PoolRecord, PoolRef, RAISE, RUN_MAIN, RecvData, pool_run_op,
};
use swoole_rs_process::reload_task::ReloadTask;
use swoole_rs_process::task_file::Packed;
use swoole_rs_process::worker_record::{WorkerRecord, swr_v1_worker_init, swr_v1_worker_shutdown};
use swoole_rs_reactor::event_api::{
    swoole_event_add, swoole_event_del, swoole_event_is_available, swoole_event_set_handler,
    swoole_event_write,
};
use swoole_rs_reactor::{Event, Running, Socket};
use swoole_rs_timer::{
    owner_reinit, swoole_timer_create, swoole_timer_del, swoole_timer_free,
    swoole_timer_get_next_msec, swoole_timer_is_available, swoole_timer_select,
    swr_v1_timer_add_plain,
};
use swoole_rs_tls::socket_pair::{self, SocketPair, unix_socket};
use swoole_rs_tls::upstream::{socket_free, socket_read_sync, socket_recv_sync, socket_send_sync};

/// `swoole::ProtocolType`, `SW_FD_PIPE`, `SW_EVENT_READ`, `SW_TIMEOUT_READ`,
/// `SW_EVENTLOOP_WAIT_EXIT`, `Reactor::PRIORITY_TRY_EXIT`, `SW_WORKER`,
/// `SW_WORKER_MESSAGE_STOP` and the `Channel` flags.
const PROTOCOL_TASK: c_int = 1;
const PROTOCOL_STREAM: c_int = 2;
const PROTOCOL_MESSAGE: c_int = 3;
const FD_PIPE: c_int = 3;
const EVENT_READ: c_int = 512;
const TIMEOUT_READ: c_int = 4;
const EVENTLOOP_WAIT_EXIT: c_int = 1;
const PRIORITY_TRY_EXIT: c_int = 4;
const SW_WORKER: c_int = 2;
const WORKER_MESSAGE_STOP: u8 = 1;
const CHAN_LOCK: c_int = 2;
const CHAN_SHM: c_int = 8;
/// `sizeof(EventData)`.
const EVENT_SIZE: usize = SW_IPC_MAX_SIZE as usize;

/// A pool's message bus: the Rust bus, then the allocator its packets and buffer
/// come from. crates/process and crates/phpprocess read both through `message_bus`.
#[repr(C)]
struct PoolBus {
    bus: *mut MessageBus,
    allocator: *const CoreAllocator,
}

/// `SwooleG.std_allocator`.
pub(crate) static STD_ALLOCATOR: CoreAllocator = CoreAllocator {
    malloc: Some(libc::malloc),
    calloc: Some(libc::calloc),
    realloc: Some(libc::realloc),
    free: Some(libc::free),
};

/// `swoole::WorkerStopMessage`.
#[repr(C)]
struct WorkerStopMessage {
    pid: c_int,
    worker_id: u16,
}

type Pool = *mut PoolRecord;
type Worker = *mut WorkerRecord;

fn record<'a>(pool: Pool) -> &'a mut PoolRecord {
    // SAFETY: every caller passes the live pool its operation runs over.
    unsafe { &mut *pool }
}

/// [`record`]'s pool as the handle its callbacks take.
pub(crate) fn pool_ref<'a>(pool: Pool) -> PoolRef<'a> {
    PoolRef::from(record(pool))
}

fn worker_of<'a>(worker: Worker) -> &'a mut WorkerRecord {
    // SAFETY: every caller passes a live worker of the pool.
    unsafe { &mut *worker }
}

/// The pool's IPC state, borrowed for one step.
fn ipc<'a>(pool: Pool) -> &'a mut swoole_rs_process::pool::PoolIpc {
    // SAFETY: a live pool's IPC state lives as long as the pool.
    unsafe { &mut *record(pool).ipc }
}

/// The pool's stream listener socket, or null.
fn listener(pool: Pool) -> *mut c_void {
    swr_v1_process_pool_ipc_listener(ipc(pool))
}

/// [`worker_of`]'s worker as a [`Record`] handle.
fn handle<'a>(worker: Worker) -> Record<'a> {
    // SAFETY: as `worker_of`; the pool's protocol orders the writes.
    unsafe { Record::new(worker) }
}

/// A live socket's fd, or -1 for none.
fn fd(socket: *mut c_void) -> c_int {
    crate::objects::socket(socket).map_or(-1, Socket::fd)
}

fn errno() -> c_int {
    std::io::Error::last_os_error().raw_os_error().unwrap_or(0)
}

/// `pool->onMessage(pool, &msg)`, whose `data` points at `info.len` bytes.
fn on_message(pool: Pool, msg: &RecvData) {
    if let Some(on_message) = record(pool).on_message {
        // SAFETY: the pool that holds the callback, and the message its caller read:
        // `data` points at `info.len` bytes.
        unsafe { on_message(pool_ref(pool), msg) };
    }
}

fn exit_status(exited: u64) -> ExitStatus {
    ExitStatus::new((exited & 0xffff_ffff) as c_int, (exited >> 32) as c_int)
}

// ---------------------------------------------------------------- message bus

fn bus_of(pool: Pool) -> crate::objects::Bus {
    // SAFETY: the pool's bus, which `OP_NEW_MESSAGE_BUS` made.
    unsafe { crate::objects::Bus::of(record(pool).message_bus) }
}

/// `MessageBus::read(sock)`.
fn bus_read(pool: Pool, sock: *mut c_void) -> isize {
    socket(sock).map_or(-1, |sock| bus_of(pool).read(sock))
}

/// `MessageBus::read(sock)` (`read_with_buffer` when `dgram`) over the bus and its
/// default allocator: allocates each packet block the bus asks for.
pub(crate) fn read_bus(
    bus: &mut MessageBus,
    allocator: *const CoreAllocator,
    sock: Socket<'_>,
    dgram: bool,
) -> isize {
    let mut state = BusRead {
        received: 0,
        size: 0,
        data: ptr::null_mut(),
        allocator: ptr::null(),
        chunks: 0,
        resume: false,
        event: 0,
    };
    loop {
        // The receive only ever sets `kernel_nobufs`, so it edits the record only then:
        // in thread mode, other threads read this worker's pipe meanwhile.
        let mut nobufs = false;
        let n = bus.receive(sock.fd(), dgram, &mut nobufs, &mut state);
        if nobufs {
            sock.edit(|sock| sock.kernel_nobufs = true);
        }
        if state.event != BUS_ALLOCATE {
            return n;
        }
        let allocator = if state.data.is_null() {
            allocator
        } else {
            state.allocator.cast()
        };
        // SAFETY: a live `swoole::Allocator`.
        let allocator = unsafe { &*allocator };
        let data = match (state.data.is_null(), allocator.malloc, allocator.realloc) {
            // SAFETY: a fresh block of the size the bus asked for.
            (true, Some(malloc), _) => unsafe { malloc(state.size) },
            // SAFETY: the block the bus asked to grow, from this allocator.
            (false, _, Some(realloc)) => unsafe { realloc(state.data, state.size) },
            _ => ptr::null_mut(),
        };
        if data.is_null() {
            // `throw std::bad_alloc()`, which nothing catches.
            std::process::abort();
        }
        // SAFETY: the block just allocated, from `allocator`.
        unsafe { bus.adopt(data, state.size, ptr::from_ref(allocator).cast()) };
    }
}

/// `onMessage` with the bus's packet, then `message_bus->pop()`.
fn bus_deliver(pool: Pool) {
    let bus = bus_of(pool);
    let mut info = bus.head();
    let packet = bus.packet();
    info.len = packet.length as u32;
    on_message(
        pool,
        &RecvData {
            info,
            data: packet.data,
        },
    );
    bus.pop();
}

fn new_message_bus(pool: Pool) -> i64 {
    let msg_id = mem_pool::alloc(8).cast::<i64>();
    if msg_id.is_null() {
        return 0;
    }
    // SAFETY: a fresh, aligned counter.
    unsafe { msg_id.write(1) };
    record(pool).message_bus = new_bus(msg_id.cast());
    1
}

/// `new MessageBus()` numbering its messages from `counter`, which outlives it;
/// [`free_bus`] frees it.
pub(crate) fn new_bus(counter: *mut c_void) -> *mut c_void {
    let mut bus = MessageBus::create(swoole_rs_corebase::constants::SW_BUFFER_SIZE_STD as usize);
    // SAFETY: the caller's live, aligned counter, which outlives the bus.
    bus.count_ids(unsafe { &*counter.cast::<AtomicU64>() });
    let bus = PoolBus {
        bus: Box::into_raw(bus),
        allocator: &raw const STD_ALLOCATOR,
    };
    Box::into_raw(Box::new(bus)).cast()
}

fn bus_buffer(pool: Pool, size: u64) -> i64 {
    i64::from(bus_of(pool).alloc_buffer(size as usize))
}

fn free_message_bus(pool: Pool) {
    // SAFETY: `new_message_bus`'s bus, freed once.
    unsafe { free_bus(record(pool).message_bus) };
}

/// `delete bus`.
///
/// # Safety
/// `bus` is [`new_bus`]'s, not used again.
pub(crate) unsafe fn free_bus(bus: *mut c_void) {
    // SAFETY: as the caller guarantees.
    let bus = unsafe { Box::from_raw(bus.cast::<PoolBus>()) };
    // SAFETY: the bus `new_bus` boxed, not used again.
    drop(unsafe { Box::from_raw(bus.bus) });
}

// ---------------------------------------------------------------- pipes and box

/// `pool->pipes`: each worker's `UnixSocket`.
type Pipes = Vec<*mut SocketPair>;

fn free_pipes(pool: Pool) {
    let pipes = record(pool).pipes.cast::<Pipes>();
    if pipes.is_null() {
        return;
    }
    // SAFETY: `OP_NEW_PIPES`'s box, freed once.
    for pair in *unsafe { Box::from_raw(pipes) } {
        // SAFETY: a pair `new_pipe` boxed, not used again.
        socket_pair::drop(&mut *unsafe { Box::from_raw(pair) });
    }
}

/// `new UnixSocket(true, SOCK_DGRAM)` for `worker`: 0 (and no pipes) when it fails.
fn new_pipe(pool: Pool, worker: Worker) -> i64 {
    let mut built = Box::new(unix_socket(true, libc::SOCK_DGRAM));
    let (master, end) = (built.master_socket, built.worker_socket);
    if master.is_null() || end.is_null() {
        socket_pair::drop(&mut built);
        free_pipes(pool);
        record(pool).pipes = ptr::null_mut();
        return 0;
    }
    let pair = Box::into_raw(built);
    // SAFETY: `OP_NEW_PIPES` made the list.
    unsafe { &mut *record(pool).pipes.cast::<Pipes>() }.push(pair);
    let worker = worker_of(worker);
    worker.pipe_master = master.cast();
    worker.pipe_worker = end.cast();
    worker.pipe_object = pair.cast();
    1
}

/// `Channel::make(size, sizeof(EventData), SW_CHAN_LOCK | SW_CHAN_SHM)`.
fn new_message_box(size: usize) -> *mut c_void {
    let flags = CHAN_LOCK | CHAN_SHM;
    let total = swr_v1_memory_channel_bytes(size, EVENT_SIZE);
    if total == 0 {
        return ptr::null_mut();
    }
    let mut memory = Shm::malloc(total);
    if memory.is_null() {
        log::warning(c"Channel::make", format_args!("alloc({size}) failed"));
        return ptr::null_mut();
    }
    // SAFETY: fresh shared memory of the size the record needs.
    match unsafe {
        swr_v1_memory_channel_init(
            memory.as_ptr().cast::<ChannelRecord>(),
            size,
            EVENT_SIZE,
            flags,
        )
    } {
        0 => return memory.into_raw(),
        1 => {
            log::sys_warning(c"Channel::make", format_args!("pipe() failed"));
            log::warning(c"Channel::make", format_args!("notify_fd init failed"));
        }
        2 => log::warning(
            c"Channel::make",
            format_args!("pthread_mutex_init() failed"),
        ),
        _ => {}
    }
    memory.free();
    ptr::null_mut()
}

/// `message_box->push(msg, msg->size())`, under the box's lock.
fn box_push(pool: Pool, message: *const u8) -> i64 {
    // SAFETY: an `EventData`, whose `info.len` counts the bytes after its head.
    let length =
        unsafe { message.cast::<DataHead>().read_unaligned() }.len as usize + size_of::<DataHead>();
    // SAFETY: as above, the message's bytes.
    let message = unsafe { std::slice::from_raw_parts(message, length) };
    // SAFETY: the box `new_message_box` made, live while the pool is; its SW_CHAN_LOCK
    // mutex serializes the processes that push to and pop from it.
    let channel = unsafe { &*record(pool).message_box.cast::<ChannelRecord>() };
    i64::from(channel.push(message))
}

pub(crate) fn free_message_box(pool: Pool) {
    let channel = record(pool).message_box.cast::<ChannelRecord>();
    // SAFETY: the box `new_message_box` made, with no other user.
    unsafe { swr_v1_memory_channel_close(channel) };
    // SAFETY: `new_message_box`'s mapping, which the pool's `message_box` only names.
    unsafe { sw_shm_free(channel.cast()) };
}

// ---------------------------------------------------------------- timers and loop

unsafe extern "C-unwind" fn reload_timeout(task: *mut c_void) {
    // SAFETY: the pool's live reload task.
    let task = unsafe { &mut *task.cast::<ReloadTask>() };
    task.kill_all(libc::SIGKILL, reload_warn);
    task.set_timer(ptr::null_mut());
}

unsafe extern "C-unwind" fn kill_timeout(pool: *mut c_void) {
    // SAFETY: the live pool the timer was set for.
    unsafe { kill_all_workers(pool.cast(), libc::SIGKILL) };
}

thread_local! {
    /// The pool and worker `OP_EXIT_CALLBACK` names.
    static EXITING: Cell<(Pool, Worker)> = const { Cell::new((ptr::null_mut(), ptr::null_mut())) };
}

/// `onWorkerExit`, then stop the loop once it may exit.
extern "C-unwind" fn worker_exit() {
    let (pool, worker) = EXITING.get();
    if let Some(on_worker_exit) = record(pool).on_worker_exit {
        // SAFETY: the pool that holds the callback, and the worker exiting.
        unsafe { on_worker_exit(pool_ref(pool), handle(worker)) };
    }
    // The loop running this end callback.
    let reactor = crate::objects::Reactor::thread();
    if reactor.if_exit() {
        reactor.fields().running = false;
    }
}

/// The pool of the loop `OP_EVENT_INIT` made: its `ptr`.
fn reactor_pool(reactor: Running<'_>) -> Pool {
    crate::objects::Reactor::running(reactor)
        .fields()
        .ptr
        .cast()
}

/// `ProcessPool::recv_packet`: the event loop's pipe handler under the stream protocol.
extern "C" fn recv_packet(reactor: Running<'_>, event: &mut Event) -> c_int {
    let pool = reactor_pool(reactor);
    let packet = pool_ipc_packet(ipc(pool));
    let size = record(pool).max_packet_size as usize;
    // SAFETY: the packet buffer holds `max_packet_size` bytes.
    let fd = fd(event.socket_ptr().cast());
    // SAFETY: as above.
    let n = unsafe { libc::read(fd, packet.cast(), size) };
    if n < 0 && errno() != libc::EINTR {
        log::sys_warning(
            c"ProcessPool::recv_packet",
            format_args!("failed to read({}) pipe", event.fd()),
        );
    }
    let info = DataHead {
        reactor_id: -1,
        // The bytes read: none when the read failed.
        len: u32::try_from(n).unwrap_or(0),
        ..DataHead::default()
    };
    on_message(pool, &RecvData { info, data: packet });
    0
}

/// `ProcessPool::recv_message`: the event loop's pipe handler under the message protocol.
extern "C" fn recv_message(reactor: Running<'_>, event: &mut Event) -> c_int {
    let pool = reactor_pool(reactor);
    if bus_read(pool, event.socket_ptr()) > 0 {
        bus_deliver(pool);
    }
    0
}

/// `ProcessPool::run_with_*_protocol` and `run_async`: the worker loop; a
/// callback's bailout is raised once the export's frames are gone.
unsafe fn run_main(pool: Pool, worker: Worker, protocol: u64) -> c_int {
    // SAFETY: the live pool and worker the loop runs over.
    let result = unsafe { pool_run_op(&NATIVE, pool, worker, RUN_MAIN, protocol) };
    if result == RAISE {
        raise_bailout();
    }
    result as c_int
}

macro_rules! main_loops {
    ($($name:ident = $protocol:expr;)*) => {$(
        extern "C-unwind" fn $name(pool: PoolRef<'_>, worker: Record<'_>) -> c_int {
            // SAFETY: the pool calls its main loop over itself and a live worker.
            unsafe { run_main(pool.raw(), worker.raw(), $protocol) }
        }
    )*};
}

main_loops! {
    run_task = PROTOCOL_TASK as u64;
    run_stream = PROTOCOL_STREAM as u64;
    run_message = PROTOCOL_MESSAGE as u64;
    run_async = 4;
}

// ---------------------------------------------------------------- the table

/// The effects on the thread's timer and event loop, and the blocking wait.
fn loop_ops(op: u32, arg: u64) -> Option<i64> {
    Some(match op {
        OP_TIMER_SELECT => {
            swoole_timer_select();
            0
        }
        OP_TIMER_FREE => {
            if swoole_timer_is_available() {
                swoole_timer_free();
            }
            0
        }
        OP_TIMER_DEL => {
            let node = ptr::with_exposed_provenance_mut(usize::try_from(arg).unwrap_or(0));
            // SAFETY: a node of the thread's timer the pool added, deleted once.
            unsafe { swoole_timer_del(node) };
            0
        }
        OP_EVENT_WAIT => i64::from(swoole_rs_reactor::event_api::swoole_event_wait()),
        OP_EVENT_AVAILABLE => i64::from(swoole_event_is_available()),
        OP_WAITPID => i64::from(swoole_rs_reactor::wait_upstream::swoole_waitpid(
            arg as c_int,
            Some(&mut 0),
            0,
        )),
        _ => return None,
    })
}

/// `pool_call`'s lifecycle effects: the pool's objects.
fn objects(pool: Pool, worker: Worker, op: u32, arg: u64) -> Option<i64> {
    Some(match op {
        // The message queue is Rust's on every platform (crates/ossupport).
        OP_HAVE_MSGQUEUE => 1,
        OP_ALLOC_WORKERS => {
            let size = record(pool).worker_num as usize * size_of::<WorkerRecord>();
            record(pool).workers = mem_pool::alloc(size as u32).cast();
            i64::from(!record(pool).workers.is_null())
        }
        OP_NEW_MESSAGE_BOX => {
            record(pool).message_box = new_message_box(arg as usize);
            i64::from(!record(pool).message_box.is_null())
        }
        OP_NEW_PIPES => {
            record(pool).pipes = Box::into_raw(Box::new(Pipes::new())).cast();
            0
        }
        OP_NEW_PIPE => new_pipe(pool, worker),
        OP_MAIN_LOOP => {
            record(pool).main_loop = Some(match arg {
                1 => run_task,
                2 => run_stream,
                3 => run_message,
                _ => run_async,
            });
            0
        }
        OP_NEW_MESSAGE_BUS => new_message_bus(pool),
        OP_SNDBUF => {
            let mut size: c_int = 0;
            let fd = fd(worker_of(worker).pipe_master);
            let failed = swoole_rs_corebase::sys::getsockopt_int(
                fd,
                libc::SOL_SOCKET,
                libc::SO_SNDBUF,
                &mut size,
            );
            if failed != 0 { -1 } else { i64::from(size) }
        }
        OP_BUS_BUFFER => bus_buffer(pool, arg),
        OP_BOX_PUSH => box_push(pool, ptr::with_exposed_provenance(arg as usize)),
        OP_FREE_PIPES => {
            free_pipes(pool);
            0
        }
        OP_FREE_MESSAGE_BOX => {
            free_message_box(pool);
            0
        }
        OP_FREE_MESSAGE_BUS => {
            free_message_bus(pool);
            0
        }
        // The global pool's `free` is a no-op.
        OP_FREE_WORKERS => 0,
        _ => return None,
    })
}

/// `pool_call`'s worker effects: its pipes, globals and event loop.
fn worker_ops(pool: Pool, worker: Worker, op: u32, arg: u64) -> Option<i64> {
    Some(match op {
        OP_UNLIMIT_PIPES => {
            for end in [worker_of(worker).pipe_worker, worker_of(worker).pipe_master] {
                if let Some(end) = socket(end) {
                    end.edit(|end| end.output.buffer_size = u32::MAX);
                }
            }
            0
        }
        OP_WORKER_INIT => {
            let record = handle(worker);
            swr_v1_worker_init(record);
            record.set_pid(std::process::id() as c_int);
            swoole_set_worker_type(SW_WORKER);
            swoole_set_worker_id(record.id());
            swoole_set_worker_pid(record.pid());
            swoole_rs_process::worker_globals::set_current_worker(worker);
            0
        }
        OP_EVENT_INIT => {
            // This thread has no event loop yet.
            if swoole_rs_reactor::swoole_event_init(EVENTLOOP_WAIT_EXIT) < 0 {
                return Some(-1);
            }
            // The loop just made.
            crate::objects::Reactor::thread().fields().ptr = pool.cast();
            0
        }
        OP_WORKER_SHUTDOWN => {
            swr_v1_worker_shutdown(handle(worker));
            0
        }
        OP_REMOVE_PIPE => {
            // arg: only when the pipe is still in the reactor.
            let pipe = worker_of(worker).pipe_worker;
            if arg == 0 || socket(pipe).is_some_and(|pipe| pipe.read(|pipe| pipe.removed == 0)) {
                // SAFETY: the worker's pipe, in this thread's loop.
                unsafe { swoole_event_del(pipe.cast()) };
            }
            0
        }
        OP_EXIT_CALLBACK => {
            EXITING.set((pool, worker));
            swoole_rs_reactor::swr_v1_event_set_end_callback(PRIORITY_TRY_EXIT, worker_exit);
            0
        }
        OP_DONT_RESTART => {
            let pipe = worker_of(worker).pipe_worker;
            if let Some(pipe) = socket(pipe) {
                pipe.edit(|pipe| pipe.dont_restart = 1);
            }
            0
        }
        OP_ADD_PIPE => {
            let pipe = worker_of(worker).pipe_worker.cast();
            // SAFETY: the worker's pipe, in this thread's loop.
            i64::from(unsafe { swoole_event_add(pipe, EVENT_READ) })
        }
        OP_PIPE_HANDLER => {
            let handler = if arg == 0 { recv_packet } else { recv_message };
            // SAFETY: the receivers need only the live event of a worker pipe.
            unsafe { swoole_event_set_handler(FD_PIPE, EVENT_READ, Some(handler)) };
            0
        }
        _ => return None,
    })
}

/// `pool_call`'s timers, messages and callbacks.
fn master_ops(pool: Pool, worker: Worker, op: u32, arg: u64) -> i64 {
    match op {
        OP_REOPEN_LOGGER => {
            swoole_rs_utilities::process_logger()
                .into_iter()
                .for_each(swoole_rs_utilities::Logger::reopen);
        }
        OP_RELOAD_TIMER => {
            let task = record(pool).reload_task;
            // SAFETY: `reload_timeout` takes the pool's reload task, which
            // deletes the timer before it goes.
            let timer = unsafe {
                swr_v1_timer_add_plain(
                    c_long::from(record(pool).max_wait_time) * 1000,
                    false,
                    reload_timeout,
                    task.cast(),
                )
            };
            // SAFETY: the pool's live reload task.
            unsafe { &mut *task }.set_timer(timer);
        }
        OP_TIMER_INIT => {
            if swoole_timer_is_available() {
                let timer = swoole_rs_corebase::thread_timer();
                // SAFETY: a timer is available, so the slot holds this thread's live timer.
                owner_reinit(unsafe { &*timer.cast::<swoole_rs_timer::Owner>() }, true);
            } else {
                // This thread has no timer.
                swoole_timer_create(true);
            }
        }
        OP_PUSH_STOP => {
            let record = handle(worker);
            let message = WorkerStopMessage {
                pid: record.pid(),
                worker_id: record.id() as u16,
            };
            // SAFETY: the live pool and the message's bytes.
            return i64::from(unsafe {
                push_message(
                    pool,
                    WORKER_MESSAGE_STOP,
                    (&raw const message).cast(),
                    size_of::<WorkerStopMessage>(),
                )
            });
        }
        OP_POP_MESSAGE => {
            let message = POOL_MESSAGE.0.get().cast::<c_void>();
            // SAFETY: the live pool and the master's message buffer.
            let popped = unsafe { pop_message(pool, message, EVENT_SIZE) };
            return if popped > 0 {
                message.expose_provenance() as i64
            } else {
                0
            };
        }
        OP_ON_WORKER_MESSAGE => {
            if let Some(on_worker_message) = record(pool).on_worker_message {
                // SAFETY: the message `OP_POP_MESSAGE` just popped.
                unsafe { on_worker_message(pool_ref(pool), POOL_MESSAGE.0.get().cast()) };
            }
        }
        OP_ON_WORKER_NOT_FOUND => {
            if let Some(not_found) = record(pool).on_worker_not_found {
                // SAFETY: the pool that holds the callback, and the reaped status.
                unsafe { not_found(pool_ref(pool), &exit_status(arg)) };
            }
        }
        OP_WORKER_ERROR => {
            let status = exit_status(arg);
            worker_report_error(handle(worker), &status);
            if let Some(on_worker_error) = record(pool).on_worker_error {
                // SAFETY: the pool that holds the callback, its worker and its status.
                unsafe { on_worker_error(pool_ref(pool), handle(worker), &status) };
            }
        }
        OP_ON_AFTER_RELOAD => {
            if let Some(on_after_reload) = record(pool).on_after_reload {
                // SAFETY: the pool that holds the callback.
                unsafe { on_after_reload(pool_ref(pool)) };
            }
        }
        OP_KILL_TIMER => {
            // SAFETY: `kill_timeout` takes the pool, which outlives its timers.
            unsafe {
                swr_v1_timer_add_plain(
                    c_long::from(record(pool).max_wait_time) * 1000,
                    false,
                    kill_timeout,
                    pool.cast(),
                )
            };
        }
        _ => {}
    }
    0
}

/// The worker's stop messages and the others the master pops while it waits.
struct Message(UnsafeCell<[u64; EVENT_SIZE / 8]>);
// SAFETY: only the pool's master process, on its one thread, uses it.
unsafe impl Sync for Message {}
static POOL_MESSAGE: Message = Message(UnsafeCell::new([0; EVENT_SIZE / 8]));

/// The pool and worker of `c`, which crates/process fills over live objects.
fn pool_worker(c: &PoolCall) -> (Pool, Worker) {
    (c.pool().cast(), c.worker().cast())
}

/// Runs `io` and records its result and errno in the call.
fn io(c: &mut PoolCall, io: impl FnOnce(&PoolCall) -> isize) {
    let result = io(c);
    c.set_result(result);
    c.set_error(errno());
}

/// The upstream method each worker loop's warnings came from.
fn loop_origin(pool: Pool) -> &'static CStr {
    match record(pool).protocol_type {
        PROTOCOL_STREAM => c"ProcessPool::run_with_stream_protocol",
        PROTOCOL_MESSAGE => c"ProcessPool::run_with_message_protocol",
        _ => c"ProcessPool::run_with_task_protocol",
    }
}

/// The objects and callbacks every `ProcessPool` method crates/process defines runs over.
struct Native;

static NATIVE: Native = Native;

impl PoolNative for Native {
    fn call(&self, c: &PoolCall, op: u32, arg: u64) -> i64 {
        let (pool, worker) = pool_worker(c);
        objects(pool, worker, op, arg)
            .or_else(|| loop_ops(op, arg))
            .or_else(|| worker_ops(pool, worker, op, arg))
            .unwrap_or_else(|| master_ops(pool, worker, op, arg))
    }

    /// The pool's callbacks, inside the bailout trap.
    fn callback(&self, c: &mut PoolCall) {
        let (pool, worker) = pool_worker(c);
        let r = record(pool);
        let (pool_fn, worker_fn) = match c.flags() {
            CB_START => (r.on_start, None),
            CB_BEFORE_RELOAD => (r.on_before_reload, None),
            CB_SHUTDOWN => (r.on_shutdown, None),
            CB_WORKER_START => (None, r.on_worker_start),
            CB_WORKER_STOP => (None, r.on_worker_stop),
            CB_MAIN_LOOP => {
                if let Some(main_loop) = r.main_loop {
                    // SAFETY: the pool that holds the callback, and its worker.
                    let code = unsafe { main_loop(pool_ref(pool), handle(worker)) };
                    c.set_result(code as isize);
                }
                return;
            }
            _ => return,
        };
        if let Some(f) = pool_fn {
            // SAFETY: the pool that holds the callback.
            unsafe { f(pool_ref(pool)) };
        } else if let Some(f) = worker_fn {
            // SAFETY: the pool that holds the callback, and its worker.
            unsafe { f(pool_ref(pool), handle(worker)) };
        }
    }

    fn event_size(&self) -> usize {
        EVENT_SIZE
    }

    fn running(&self, c: &PoolCall) -> bool {
        let (pool, worker) = pool_worker(c);
        is_worker_running(record(pool), handle(worker))
    }

    fn end(&self) {
        swoole_timer_select();
        swoole_rs_process::signal::swoole_signal_dispatch();
    }

    fn queue_pop(&self, c: &mut PoolCall) {
        io(c, |c| {
            // SAFETY: the pool's queue and the call's buffer.
            unsafe {
                swoole_rs_ossupport::swr_v1_os_msg_queue_pop(
                    record(c.pool().cast()).queue.cast(),
                    c.data().cast(),
                    c.length(),
                )
            }
        });
    }

    fn queue_push(&self, c: &mut PoolCall) {
        io(c, |c| {
            // SAFETY: the pool's queue and the call's message.
            let pushed = unsafe {
                swoole_rs_ossupport::swr_v1_os_msg_queue_push(
                    record(c.pool().cast()).queue.cast(),
                    c.data().cast(),
                    c.length(),
                )
            };
            isize::from(pushed)
        });
    }

    fn accept(&self, c: &mut PoolCall) {
        let listener = listener(c.pool().cast());
        let socket = socket(listener.cast()).map_or(ptr::null_mut(), |listener| {
            listener.edit(swoole_rs_reactor::SocketFields::accept)
        });
        // SAFETY: null, or the connection the listener just accepted, which the pool frees.
        unsafe { c.set_socket(socket.cast()) };
        c.set_error(errno());
    }

    fn recv(&self, c: &mut PoolCall) {
        // SAFETY: the call's connection and buffer.
        io(c, |c| unsafe {
            socket_recv_sync(c.socket().cast(), c.data(), c.length(), c.flags())
        });
    }

    fn send(&self, c: &mut PoolCall) {
        // SAFETY: the call's connection and bytes.
        io(c, |c| unsafe {
            socket_send_sync(c.socket().cast(), c.data(), c.length(), 0)
        });
    }

    fn close(&self, c: &PoolCall) {
        // SAFETY: a connection the loop is done with.
        unsafe { socket_free(c.socket().cast()) };
    }

    fn pipe_read(&self, c: &mut PoolCall) {
        io(c, |c| {
            let Some(pipe) = socket(worker_of(c.worker().cast()).pipe_worker) else {
                return -1;
            };
            let next = swoole_timer_get_next_msec();
            #[expect(clippy::cast_precision_loss, reason = "msec2sec's (double) conversion")]
            let timeout = next as f64 / 1000.0;
            pipe.edit(|pipe| pipe.timeouts.set(timeout, TIMEOUT_READ));
            // SAFETY: the worker's pipe and the call's buffer.
            unsafe { socket_read_sync(pipe.as_ptr(), c.data(), c.length()) }
        });
    }

    fn pipe_wait(&self, c: &mut PoolCall) {
        io(c, |c| {
            let next = swoole_timer_get_next_msec();
            socket(worker_of(c.worker().cast()).pipe_worker).map_or(-1, |pipe| {
                pipe.wait_event(next as c_int, EVENT_READ) as isize
            })
        });
    }

    fn bus_read(&self, c: &mut PoolCall) {
        io(c, |c| {
            bus_read(c.pool().cast(), worker_of(c.worker().cast()).pipe_worker)
        });
    }

    fn bus_deliver(&self, c: &mut PoolCall) {
        bus_deliver(c.pool().cast());
    }

    fn task(&self, c: &mut PoolCall) {
        let (pool, worker) = pool_worker(c);
        if let Some(on_task) = record(pool).on_task {
            // SAFETY: the task the worker read into the call's buffer, its head and
            // the `len` bytes after it.
            let task = unsafe { Packed::from_raw(c.data().cast()) };
            // SAFETY: the pool that holds the callback, its worker and that task.
            let code = unsafe { on_task(pool_ref(pool), handle(worker), task) };
            c.set_result(code as isize);
        }
    }

    fn message(&self, c: &mut PoolCall) {
        let info = DataHead {
            reactor_id: -1,
            len: c.length() as u32,
            ..DataHead::default()
        };
        on_message(
            c.pool().cast(),
            &RecvData {
                info,
                data: c.data().cast(),
            },
        );
    }

    fn shutdown(&self, c: &PoolCall) {
        swr_v1_worker_shutdown(handle(c.worker().cast()));
    }

    fn pipe_write(&self, c: &mut PoolCall) {
        io(c, |c| {
            if c.flags() != 0 && swoole_event_is_available() {
                // SAFETY: the worker's pipe, in this thread's loop.
                unsafe { swoole_event_write(c.socket().cast(), c.data(), c.length()) }
            } else {
                // SAFETY: the worker's pipe and the call's bytes.
                unsafe { socket_send_sync(c.socket().cast(), c.data(), c.length(), 0) }
            }
        });
    }

    /// A `WARN_*`; the `TRACE_*` logs exist only in trace builds, which this one isn't.
    fn warn(&self, c: &PoolCall, kind: c_int, a: i64, b: i64) {
        let (pool, worker) = pool_worker(c);
        let id = if worker.is_null() {
            0
        } else {
            handle(worker).id()
        };
        let origin = loop_origin(pool);
        match kind {
            WARN_MSGRCV => {
                let queue = record(pool)
                    .queue
                    .cast::<swoole_rs_ossupport::QueueRecord>();
                // SAFETY: the pool's live queue.
                let queue = unsafe { &*queue }.id();
                log::sys_warning(origin, format_args!("[Worker#{id}] msgrcv({queue}) failed"));
            }
            WARN_ACCEPT => {
                let listener = listener(pool);
                log::sys_warning(
                    origin,
                    format_args!("[Worker#{id}] accept({}) failed", fd(listener.cast())),
                );
            }
            WARN_READ => {
                let fd = fd(worker_of(worker).pipe_worker);
                log::sys_warning(origin, format_args!("[Worker#{id}] read({fd}) failed"));
            }
            WARN_BAD_PACKET => log::error_log(
                4, // SW_LOG_WARNING
                swoole_rs_error::ErrorCode::SW_ERROR_PROTOCOL_ERROR as c_int,
                origin,
                format_args!(
                    "[Worker#{id}] bad task packet, received data length[{a}], packet length[{b}]"
                ),
            ),
            WARN_TASK_FAILED => log::warning(
                origin,
                format_args!("[Worker#{id}] the execution of task#{a} has failed"),
            ),
            WARN_PIPE => log::sys_warning(
                origin,
                format_args!("[Worker #{id}]failed to read data from pipe"),
            ),
            WARN_SEND => log::warning(
                c"ProcessPool::dispatch",
                format_args!("send {} bytes to worker#{} failed", a as c_int, b as c_int),
            ),
            _ => {}
        }
    }
}

/// What every `ProcessPool` method crates/process defines runs over, which it
/// declares and links by name since this crate depends on it.
#[unsafe(no_mangle)]
#[expect(
    clippy::no_mangle_with_rust_abi,
    reason = "crates/process declares it `extern \"Rust\"`: both sides are Rust, and the result is a trait object"
)]
pub(crate) fn swr_v1_process_pool_native() -> &'static dyn PoolNative {
    &NATIVE
}
