//! The objects `ProcessPool` runs over (once `src/os/process_pool.cc`): its worker pipes, message box, message bus,
//! timers, event loop and callbacks. crates/process decides every step
//! (`process_pool.rs`, `pool.rs`) and asks for each effect through the table
//! [`swr_v1_process_pool_native`] returns. It lives here because these objects are
//! crates/reactor's, crates/tls's and crates/memory's, which crates/process can't
//! depend on.
//!
//! Each hook that can reach PHP runs inside the bailout trap crates/process sets
//! up; the frames here hold nothing to drop, so the `longjmp` may cross them.
#![expect(
    clippy::cast_possible_truncation,
    clippy::cast_possible_wrap,
    clippy::cast_sign_loss,
    reason = "upstream's C conversions between ssize_t, size_t, uint32_t and int"
)]
use core::cell::{Cell, UnsafeCell};
use core::ffi::{CStr, c_char, c_int, c_long, c_void};
use core::mem::MaybeUninit;
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
use swoole_rs_memory::members::{pool as mem_pool, sw_shm_free, sw_shm_malloc};
use swoole_rs_memory::{
    swr_v1_memory_channel_bytes, swr_v1_memory_channel_close, swr_v1_memory_channel_in,
    swr_v1_memory_channel_init, swr_v1_memory_channel_lock,
};
use swoole_rs_process::methods::{
    ExitStatus, is_worker_running, kill_all_workers, pop_message, push_message, reload_warn,
    swoole_set_worker_id, swoole_set_worker_pid, swoole_set_worker_type, worker_report_error,
};
use swoole_rs_process::pool::{
    PoolCall, PoolHooks, WARN_ACCEPT, WARN_BAD_PACKET, WARN_MSGRCV, WARN_PIPE, WARN_READ,
    WARN_SEND, WARN_TASK_FAILED, pool_ipc_packet, raise_bailout, swr_v1_process_pool_ipc_listener,
};
use swoole_rs_process::process_pool::{
    CB_BEFORE_RELOAD, CB_MAIN_LOOP, CB_SHUTDOWN, CB_START, CB_WORKER_START, CB_WORKER_STOP,
    OP_ADD_PIPE, OP_ALLOC_WORKERS, OP_BOX_PUSH, OP_BUS_BUFFER, OP_DONT_RESTART, OP_EVENT_INIT,
    OP_EXIT_CALLBACK, OP_FREE_MESSAGE_BOX, OP_FREE_MESSAGE_BUS, OP_FREE_PIPES, OP_FREE_WORKERS,
    OP_HAVE_MSGQUEUE, OP_KILL_TIMER, OP_MAIN_LOOP, OP_NEW_MESSAGE_BOX, OP_NEW_MESSAGE_BUS,
    OP_NEW_PIPE, OP_NEW_PIPES, OP_ON_AFTER_RELOAD, OP_ON_WORKER_MESSAGE, OP_ON_WORKER_NOT_FOUND,
    OP_PIPE_HANDLER, OP_POP_MESSAGE, OP_PUSH_STOP, OP_RELOAD_TIMER, OP_REMOVE_PIPE,
    OP_REOPEN_LOGGER, OP_SNDBUF, OP_TIMER_INIT, OP_UNLIMIT_PIPES, OP_WORKER_ERROR, OP_WORKER_INIT,
    OP_WORKER_SHUTDOWN, PoolNative, PoolRecord, RAISE, RUN_MAIN, pool_run_op,
};
use swoole_rs_process::reload_task::ReloadTask;
use swoole_rs_process::worker_globals::thread_globals;
use swoole_rs_process::worker_record::{WorkerRecord, swr_v1_worker_init, swr_v1_worker_shutdown};
use swoole_rs_reactor::event_api::{
    swoole_event_add, swoole_event_del, swoole_event_is_available, swoole_event_set_handler,
    swoole_event_write, swr_v1_event_current,
};
use swoole_rs_reactor::{Event, Socket};
use swoole_rs_timer::{
    owner_reinit, swoole_timer_create, swoole_timer_get_next_msec, swoole_timer_is_available,
    swoole_timer_select, swr_v1_timer_add_plain,
};
use swoole_rs_tls::socket_pair::{self, SocketPair, unix_socket_new};
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

// SAFETY: crates/reactor's, under its upstream symbol.
unsafe extern "C-unwind" {
    #[link_name = "_Z17swoole_event_initi"]
    fn swoole_event_init(flags: c_int) -> c_int;
}

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

/// `swoole::RecvData`.
#[repr(C)]
struct RecvData {
    info: DataHead,
    data: *const c_char,
}

/// `swoole::WorkerStopMessage`.
#[repr(C)]
struct WorkerStopMessage {
    pid: c_int,
    worker_id: u16,
}

type Pool = *mut PoolRecord;
type Worker = *mut WorkerRecord;

/// The pool's callbacks, from the record's untyped slots.
macro_rules! callback {
    ($slot:expr, $ty:ty) => {{
        let slot: *const c_void = $slot;
        // SAFETY: the slot holds null or a function of this type.
        unsafe { core::mem::transmute::<*const c_void, Option<$ty>>(slot) }
    }};
}

type PoolFn = unsafe extern "C" fn(Pool);
type WorkerFn = unsafe extern "C" fn(Pool, Worker);
type TaskFn = unsafe extern "C" fn(Pool, Worker, *mut c_void) -> c_int;
type MessageFn = unsafe extern "C" fn(Pool, *mut RecvData);
type EventFn = unsafe extern "C" fn(Pool, *mut c_void);
type ErrorFn = unsafe extern "C" fn(Pool, Worker, *const ExitStatus);
type NotFoundFn = unsafe extern "C" fn(Pool, *const ExitStatus) -> c_int;

fn record<'a>(pool: Pool) -> &'a mut PoolRecord {
    // SAFETY: every caller passes the live pool its operation runs over.
    unsafe { &mut *pool }
}

fn worker_of<'a>(worker: Worker) -> &'a mut WorkerRecord {
    // SAFETY: every caller passes a live worker of the pool.
    unsafe { &mut *worker }
}

/// A live socket's fd, or -1 for none.
fn fd(socket: *mut c_void) -> c_int {
    crate::objects::socket(socket).map_or(-1, Socket::fd)
}

fn errno() -> c_int {
    std::io::Error::last_os_error().raw_os_error().unwrap_or(0)
}

/// `pool->onMessage(pool, &msg)`.
fn on_message(pool: Pool, mut msg: RecvData) {
    if let Some(on_message) = callback!(record(pool).on_message, MessageFn) {
        // SAFETY: the pool's callback over a message it reads during the call.
        unsafe { on_message(pool, &raw mut msg) };
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
        let n = sock.edit(|sock| {
            // SAFETY: a bus whose buffer is set, and its pipe's descriptor and flag.
            unsafe { bus.receive(sock.fd, dgram, &mut sock.kernel_nobufs, &mut state) }
        });
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
        RecvData {
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
        // SAFETY: a pair `new_pipe` built, not used again.
        unsafe { socket_pair::drop(pair) };
        // SAFETY: its box.
        drop(unsafe { Box::from_raw(pair.cast::<MaybeUninit<SocketPair>>()) });
    }
}

/// `new UnixSocket(true, SOCK_DGRAM)` for `worker`: 0 (and no pipes) when it fails.
fn new_pipe(pool: Pool, worker: Worker) -> i64 {
    let pair = Box::into_raw(Box::new(MaybeUninit::<SocketPair>::uninit())).cast::<SocketPair>();
    // SAFETY: fresh storage for a pair.
    unsafe { unix_socket_new(pair, true, libc::SOCK_DGRAM) };
    // SAFETY: constructed above.
    let pair_ref = unsafe { &*pair };
    let (master, end) = (pair_ref.master_socket, pair_ref.worker_socket);
    if master.is_null() || end.is_null() {
        // SAFETY: the pair just built.
        unsafe { socket_pair::drop(pair) };
        // SAFETY: its box.
        drop(unsafe { Box::from_raw(pair.cast::<MaybeUninit<SocketPair>>()) });
        free_pipes(pool);
        record(pool).pipes = ptr::null_mut();
        return 0;
    }
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
    let memory = sw_shm_malloc(total);
    if memory.is_null() {
        log::warning(c"Channel::make", format_args!("alloc({size}) failed"));
        return ptr::null_mut();
    }
    // SAFETY: fresh shared memory of the size the record needs.
    match unsafe {
        swr_v1_memory_channel_init(memory.cast::<ChannelRecord>(), size, EVENT_SIZE, flags)
    } {
        0 => return memory,
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
    sw_shm_free(memory);
    ptr::null_mut()
}

/// `message_box->push(msg, msg->size())`, under the box's lock.
fn box_push(pool: Pool, message: *const u8) -> i64 {
    let channel = record(pool).message_box.cast::<ChannelRecord>();
    // SAFETY: an `EventData`, whose `info.len` counts the bytes after its head.
    let length =
        unsafe { message.cast::<DataHead>().read_unaligned() }.len as usize + size_of::<DataHead>();
    // SAFETY: the box was made with SW_CHAN_LOCK.
    unsafe { swr_v1_memory_channel_lock(channel, true) };
    // SAFETY: the box's queue and the message's bytes.
    let pushed =
        unsafe { swr_v1_memory_channel_in(channel.cast(), message.cast(), length as c_int) };
    // SAFETY: this thread holds the lock.
    unsafe { swr_v1_memory_channel_lock(channel, false) };
    i64::from(pushed)
}

pub(crate) fn free_message_box(pool: Pool) {
    let channel = record(pool).message_box.cast::<ChannelRecord>();
    // SAFETY: the box `new_message_box` made, with no other user.
    unsafe { swr_v1_memory_channel_close(channel) };
    sw_shm_free(channel.cast());
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
    if let Some(on_worker_exit) = callback!(record(pool).on_worker_exit, WorkerFn) {
        // SAFETY: the pool's callback over its worker.
        unsafe { on_worker_exit(pool, worker) };
    }
    // SAFETY: the loop running this end callback.
    let reactor = unsafe { crate::objects::Reactor::of(swr_v1_event_current()) };
    if reactor.if_exit() {
        reactor.fields().running = false;
    }
}

/// The pool of the loop `OP_EVENT_INIT` made: its `ptr`.
fn reactor_pool(reactor: *mut c_void) -> Pool {
    // SAFETY: the thread's live loop.
    unsafe { crate::objects::Reactor::of(reactor) }
        .fields()
        .ptr
        .cast()
}

/// `ProcessPool::recv_packet`: the event loop's pipe handler under the stream protocol.
unsafe extern "C" fn recv_packet(reactor: *mut c_void, event: *mut c_void) -> c_int {
    let pool = reactor_pool(reactor);
    // SAFETY: the event the loop passes.
    let event = unsafe { &*event.cast::<Event>() };
    // SAFETY: the pool's IPC state.
    let packet = unsafe { pool_ipc_packet(record(pool).ipc.cast()) };
    let size = record(pool).max_packet_size as usize;
    // SAFETY: the packet buffer holds `max_packet_size` bytes.
    let fd = fd(event.socket.cast());
    // SAFETY: as above.
    let n = unsafe { libc::read(fd, packet.cast(), size) };
    if n < 0 && errno() != libc::EINTR {
        log::sys_warning(
            c"ProcessPool::recv_packet",
            format_args!("failed to read({}) pipe", event.fd),
        );
    }
    let info = DataHead {
        reactor_id: -1,
        len: n as u32,
        ..DataHead::default()
    };
    on_message(pool, RecvData { info, data: packet });
    0
}

/// `ProcessPool::recv_message`: the event loop's pipe handler under the message protocol.
unsafe extern "C" fn recv_message(reactor: *mut c_void, event: *mut c_void) -> c_int {
    let pool = reactor_pool(reactor);
    // SAFETY: the event the loop passes.
    let sock = unsafe { (*event.cast::<Event>()).socket };
    if bus_read(pool, sock) > 0 {
        bus_deliver(pool);
    }
    0
}

/// `ProcessPool::run_with_*_protocol` and `run_async`: the worker loop; a
/// callback's bailout is raised once the export's frames are gone.
unsafe fn run_main(pool: Pool, worker: Worker, protocol: u64) -> c_int {
    // SAFETY: the live pool and worker the loop runs over.
    let result = unsafe { pool_run_op(&raw const NATIVE.0, pool, worker, RUN_MAIN, protocol) };
    if result == RAISE {
        raise_bailout();
    }
    result as c_int
}

macro_rules! main_loops {
    ($($name:ident = $protocol:expr;)*) => {$(
        unsafe extern "C" fn $name(pool: Pool, worker: Worker) -> c_int {
            // SAFETY: the pool calls its main loop over a live worker.
            unsafe { run_main(pool, worker, $protocol) }
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
            let main: unsafe extern "C" fn(Pool, Worker) -> c_int = match arg {
                1 => run_task,
                2 => run_stream,
                3 => run_message,
                _ => run_async,
            };
            record(pool).main_loop = main as *const c_void;
            0
        }
        OP_NEW_MESSAGE_BUS => new_message_bus(pool),
        OP_SNDBUF => {
            let mut size: c_int = 0;
            let mut length = size_of::<c_int>() as libc::socklen_t;
            let fd = fd(worker_of(worker).pipe_master);
            // SAFETY: an int option into a local.
            let failed = unsafe {
                libc::getsockopt(
                    fd,
                    libc::SOL_SOCKET,
                    libc::SO_SNDBUF,
                    (&raw mut size).cast(),
                    &raw mut length,
                )
            };
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
            // SAFETY: the worker this process now runs.
            unsafe { swr_v1_worker_init(worker) };
            // SAFETY: as above.
            let record = unsafe { Record::new(worker) };
            record.set_pid(std::process::id() as c_int);
            swoole_set_worker_type(SW_WORKER);
            swoole_set_worker_id(record.id());
            swoole_set_worker_pid(record.pid());
            // SAFETY: this thread's worker globals.
            unsafe { (*thread_globals()).worker = worker };
            0
        }
        OP_EVENT_INIT => {
            // SAFETY: this thread has no event loop yet.
            if unsafe { swoole_event_init(EVENTLOOP_WAIT_EXIT) } < 0 {
                return Some(-1);
            }
            // SAFETY: the loop just made.
            unsafe { crate::objects::Reactor::of(swr_v1_event_current()) }
                .fields()
                .ptr = pool.cast();
            0
        }
        OP_WORKER_SHUTDOWN => {
            // SAFETY: the worker this process runs.
            unsafe { swr_v1_worker_shutdown(worker) };
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
                // SAFETY: this thread's timer slot, holding its live timer.
                let timer = unsafe { *swoole_rs_corebase::swr_v1_core_timer_slot() };
                // SAFETY: as above.
                unsafe { owner_reinit(timer.cast(), true) };
            } else {
                // This thread has no timer.
                swoole_timer_create(true);
            }
        }
        OP_PUSH_STOP => {
            // SAFETY: the worker stopping.
            let record = unsafe { Record::new(worker) };
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
            if let Some(on_worker_message) = callback!(record(pool).on_worker_message, EventFn) {
                // SAFETY: the message `OP_POP_MESSAGE` just popped.
                unsafe { on_worker_message(pool, POOL_MESSAGE.0.get().cast()) };
            }
        }
        OP_ON_WORKER_NOT_FOUND => {
            if let Some(not_found) = callback!(record(pool).on_worker_not_found, NotFoundFn) {
                // SAFETY: the pool's callback over a status it reads during the call.
                let status = exit_status(arg);
                // SAFETY: as above.
                unsafe { not_found(pool, &raw const status) };
            }
        }
        OP_WORKER_ERROR => {
            let status = exit_status(arg);
            // SAFETY: the worker that exited.
            unsafe { worker_report_error(worker, &raw const status) };
            if let Some(on_worker_error) = callback!(record(pool).on_worker_error, ErrorFn) {
                // SAFETY: the pool's callback over its worker and the status.
                unsafe { on_worker_error(pool, worker, &raw const status) };
            }
        }
        OP_ON_AFTER_RELOAD => {
            if let Some(on_after_reload) = callback!(record(pool).on_after_reload, PoolFn) {
                // SAFETY: the pool's callback.
                unsafe { on_after_reload(pool) };
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

unsafe extern "C" fn call(pool: *mut c_void, worker: *mut c_void, op: u32, arg: u64) -> i64 {
    let (pool, worker) = (pool.cast::<PoolRecord>(), worker.cast::<WorkerRecord>());
    objects(pool, worker, op, arg)
        .or_else(|| worker_ops(pool, worker, op, arg))
        .unwrap_or_else(|| master_ops(pool, worker, op, arg))
}

fn call_of<'a>(arg: *mut c_void) -> &'a mut PoolCall {
    // SAFETY: every hook gets its live call.
    unsafe { &mut *arg.cast::<PoolCall>() }
}

fn pool_worker(arg: *mut c_void) -> (Pool, Worker) {
    let c = call_of(arg);
    (c.pool.cast(), c.worker.cast())
}

/// The pool's callbacks, inside the bailout trap.
unsafe extern "C" fn pool_callback(arg: *mut c_void) {
    let (pool, worker) = pool_worker(arg);
    let r = record(pool);
    let (pool_fn, worker_fn) = match call_of(arg).flags {
        CB_START => (r.on_start, ptr::null()),
        CB_BEFORE_RELOAD => (r.on_before_reload, ptr::null()),
        CB_SHUTDOWN => (r.on_shutdown, ptr::null()),
        CB_WORKER_START => (ptr::null(), r.on_worker_start),
        CB_WORKER_STOP => (ptr::null(), r.on_worker_stop),
        CB_MAIN_LOOP => {
            if let Some(main_loop) =
                callback!(r.main_loop, unsafe extern "C" fn(Pool, Worker) -> c_int)
            {
                // SAFETY: the pool's main loop over its worker.
                call_of(arg).result = unsafe { main_loop(pool, worker) } as isize;
            }
            return;
        }
        _ => return,
    };
    if let Some(f) = callback!(pool_fn, PoolFn) {
        // SAFETY: the pool's callback.
        unsafe { f(pool) };
    } else if let Some(f) = callback!(worker_fn, WorkerFn) {
        // SAFETY: the pool's callback over its worker.
        unsafe { f(pool, worker) };
    }
}

/// Runs `io` and records its result and errno in the call.
fn io(arg: *mut c_void, io: impl FnOnce(&mut PoolCall) -> isize) {
    let c = call_of(arg);
    c.result = io(c);
    c.error = errno();
}

unsafe extern "C" fn running(pool: *mut c_void, worker: *mut c_void) -> bool {
    // SAFETY: the live pool and worker.
    unsafe { is_worker_running(pool.cast(), worker.cast()) }
}

unsafe extern "C" fn end(_: *mut c_void) {
    swoole_timer_select();
    swoole_rs_process::signal::swoole_signal_dispatch();
}

unsafe extern "C" fn queue_pop(arg: *mut c_void) {
    io(arg, |c| {
        // SAFETY: the pool's queue and the call's buffer.
        unsafe {
            swoole_rs_ossupport::swr_v1_os_msg_queue_pop(
                record(c.pool.cast()).queue.cast(),
                c.data.cast(),
                c.length,
            )
        }
    });
}

unsafe extern "C" fn queue_push(arg: *mut c_void) {
    io(arg, |c| {
        // SAFETY: the pool's queue and the call's message.
        let pushed = unsafe {
            swoole_rs_ossupport::swr_v1_os_msg_queue_push(
                record(c.pool.cast()).queue.cast(),
                c.data.cast(),
                c.length,
            )
        };
        isize::from(pushed)
    });
}

unsafe extern "C" fn accept(arg: *mut c_void) {
    let c = call_of(arg);
    // SAFETY: the pool's stream listener.
    let listener = unsafe { swr_v1_process_pool_ipc_listener(record(c.pool.cast()).ipc.cast()) };
    c.socket = socket(listener.cast())
        .map_or(ptr::null_mut(), |listener| {
            listener.edit(swoole_rs_reactor::SocketRecord::accept)
        })
        .cast();
    c.error = errno();
}

unsafe extern "C" fn recv(arg: *mut c_void) {
    // SAFETY: the call's connection and buffer.
    io(arg, |c| unsafe {
        socket_recv_sync(c.socket.cast(), c.data, c.length, c.flags)
    });
}

unsafe extern "C" fn send(arg: *mut c_void) {
    // SAFETY: the call's connection and bytes.
    io(arg, |c| unsafe {
        socket_send_sync(c.socket.cast(), c.data, c.length, 0)
    });
}

unsafe extern "C" fn close(sock: *mut c_void) {
    // SAFETY: a connection the loop is done with.
    unsafe { socket_free(sock.cast()) };
}

unsafe extern "C" fn pipe_read(arg: *mut c_void) {
    io(arg, |c| {
        let Some(pipe) = socket(worker_of(c.worker.cast()).pipe_worker) else {
            return -1;
        };
        let next = swoole_timer_get_next_msec();
        #[expect(clippy::cast_precision_loss, reason = "msec2sec's (double) conversion")]
        let timeout = next as f64 / 1000.0;
        pipe.edit(|pipe| pipe.timeouts.set(timeout, TIMEOUT_READ));
        // SAFETY: the worker's pipe and the call's buffer.
        unsafe { socket_read_sync(pipe.as_ptr(), c.data, c.length) }
    });
}

unsafe extern "C" fn pipe_wait(arg: *mut c_void) {
    io(arg, |c| {
        let next = swoole_timer_get_next_msec();
        socket(worker_of(c.worker.cast()).pipe_worker).map_or(-1, |pipe| {
            pipe.wait_event(next as c_int, EVENT_READ) as isize
        })
    });
}

unsafe extern "C" fn hook_bus_read(arg: *mut c_void) {
    io(arg, |c| {
        bus_read(c.pool.cast(), worker_of(c.worker.cast()).pipe_worker)
    });
}

unsafe extern "C" fn hook_bus_deliver(arg: *mut c_void) {
    bus_deliver(call_of(arg).pool.cast());
}

unsafe extern "C" fn task(arg: *mut c_void) {
    let (pool, worker) = pool_worker(arg);
    if let Some(on_task) = callback!(record(pool).on_task, TaskFn) {
        let c = call_of(arg);
        // SAFETY: the pool's callback over its worker and the task read.
        c.result = unsafe { on_task(pool, worker, c.data) } as isize;
    }
}

unsafe extern "C" fn message(arg: *mut c_void) {
    let c = call_of(arg);
    let info = DataHead {
        reactor_id: -1,
        len: c.length as u32,
        ..DataHead::default()
    };
    on_message(
        c.pool.cast(),
        RecvData {
            info,
            data: c.data.cast(),
        },
    );
}

unsafe extern "C" fn shutdown(worker: *mut c_void) {
    // SAFETY: the worker this process runs.
    unsafe { swr_v1_worker_shutdown(worker.cast()) };
}

unsafe extern "C" fn pipe_write(arg: *mut c_void) {
    io(arg, |c| {
        if c.flags != 0 && swoole_event_is_available() {
            // SAFETY: the worker's pipe, in this thread's loop.
            unsafe { swoole_event_write(c.socket.cast(), c.data, c.length) }
        } else {
            // SAFETY: the worker's pipe and the call's bytes.
            unsafe { socket_send_sync(c.socket.cast(), c.data, c.length, 0) }
        }
    });
}

/// The upstream method each worker loop's warnings came from.
fn loop_origin(pool: Pool) -> &'static CStr {
    match record(pool).protocol_type {
        PROTOCOL_STREAM => c"ProcessPool::run_with_stream_protocol",
        PROTOCOL_MESSAGE => c"ProcessPool::run_with_message_protocol",
        _ => c"ProcessPool::run_with_task_protocol",
    }
}

/// A `WARN_*`; the `TRACE_*` logs exist only in trace builds, which this one isn't.
unsafe extern "C" fn warn(pool: *mut c_void, worker: *mut c_void, kind: c_int, a: i64, b: i64) {
    let pool = pool.cast::<PoolRecord>();
    let worker = worker.cast::<WorkerRecord>();
    let id = if worker.is_null() {
        0
    } else {
        // SAFETY: the loop's worker.
        unsafe { Record::new(worker).id() }
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
            // SAFETY: the pool's stream listener.
            let listener = unsafe { swr_v1_process_pool_ipc_listener(record(pool).ipc.cast()) };
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

static HOOKS: PoolHooks = PoolHooks {
    running,
    end,
    queue_pop,
    queue_push,
    accept,
    recv,
    send,
    close,
    pipe_read,
    pipe_wait,
    bus_read: hook_bus_read,
    bus_deliver: hook_bus_deliver,
    task,
    message,
    shutdown,
    pipe_write,
    warn,
};

struct Native(PoolNative);
// SAFETY: the table is immutable and its hooks are static.
unsafe impl Sync for Native {}

static NATIVE: Native = Native(PoolNative {
    call,
    callback: pool_callback,
    hooks: &raw const HOOKS,
    event_size: EVENT_SIZE,
});

/// The table every `ProcessPool` method crates/process defines runs over.
#[unsafe(no_mangle)]
pub(crate) extern "C" fn swr_v1_process_pool_native() -> *const PoolNative {
    &raw const NATIVE.0
}
