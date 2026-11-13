//! The objects `ProcessPool` runs over (once `src/os/process_pool.cc`): its worker pipes, message bus,
//! timers, event loop and callbacks. crates/process decides every step
//! (`process_pool.rs`, `pool.rs`) and asks for each effect through the
//! [`PoolNative`] side [`swr_v1_process_pool_native`] returns. It lives here because these objects are
//! crates/reactor's, crates/tls's and crates/memory's, which crates/process can't
//! depend on.
//!
//! Each hook that can reach PHP runs inside the bailout trap crates/process sets
//! up; the frames here hold nothing to drop, so the `longjmp` may cross them.
use core::cell::{Cell, RefCell};
use core::ffi::{CStr, c_int, c_long, c_void};
use core::ptr;
use swoole_rs_corebase::CCast;
use swoole_rs_process::worker_record::Worker as Record;
use swoole_rs_process::worker_record::WorkerType;

use crate::objects::{Reactor, worker_pipe};
use core::sync::atomic::AtomicU64;
use swoole_rs_bus::{BUS_ALLOCATE, BusRead, MessageBus};
use swoole_rs_corebase::CoreAllocator;
use swoole_rs_corebase::DataHead;
use swoole_rs_corebase::constants::SW_IPC_MAX_SIZE;
use swoole_rs_corebase::log;
use swoole_rs_memory::members::pool as mem_pool;
use swoole_rs_process::armed_timer::ArmedTimer;
use swoole_rs_process::methods::{
    ExitStatus, is_worker_running, kill_all_workers, pop_message, push_stop, reload_warn,
    swoole_set_worker_id, swoole_set_worker_pid, worker_report_error,
};
use swoole_rs_process::pool::{
    PoolCall, WARN_ACCEPT, WARN_BAD_PACKET, WARN_MSGRCV, WARN_PIPE, WARN_READ, WARN_SEND,
    WARN_TASK_FAILED, raise_bailout,
};
use swoole_rs_process::process_pool::{
    CB_BEFORE_RELOAD, CB_MAIN_LOOP, CB_SHUTDOWN, CB_START, CB_WORKER_START, CB_WORKER_STOP, Hooks,
    MainLoopFn, OP_ADD_PIPE, OP_ALLOC_WORKERS, OP_BUS_BUFFER, OP_DONT_RESTART, OP_EVENT_AVAILABLE,
    OP_EVENT_INIT, OP_EVENT_WAIT, OP_EXIT_CALLBACK, OP_FREE_MESSAGE_BUS, OP_FREE_PIPES,
    OP_FREE_WORKERS, OP_HAVE_MSGQUEUE, OP_KILL_TIMER, OP_MAIN_LOOP, OP_NEW_MESSAGE_BUS,
    OP_NEW_PIPE, OP_NEW_PIPES, OP_ON_AFTER_RELOAD, OP_ON_WORKER_MESSAGE, OP_ON_WORKER_NOT_FOUND,
    OP_PIPE_HANDLER, OP_POP_MESSAGE, OP_PUSH_STOP, OP_RELOAD_TIMER, OP_REMOVE_PIPE,
    OP_REOPEN_LOGGER, OP_SNDBUF, OP_TIMER_FREE, OP_TIMER_INIT, OP_TIMER_SELECT, OP_UNLIMIT_PIPES,
    OP_WAITPID, OP_WORKER_ERROR, OP_WORKER_INIT, OP_WORKER_SHUTDOWN, PoolNative, PoolRef, RAISE,
    RecvData, pool_run_main,
};
use swoole_rs_process::task_file::Packed;
use swoole_rs_process::worker_record::{WorkerRecord, swr_v1_worker_init, swr_v1_worker_shutdown};
use swoole_rs_reactor::event_api::{
    swoole_event_is_available, swoole_event_set_handler, swoole_event_write,
};
use swoole_rs_reactor::{Event, Running, Socket};
use swoole_rs_timer::{
    swoole_timer_create, swoole_timer_free, swoole_timer_get_next_msec, swoole_timer_is_available,
    swoole_timer_select,
};
use swoole_rs_tls::socket_pair::{self, SocketPair, unix_socket};
use swoole_rs_tls::upstream::{self, socket_free, socket_read_sync, socket_recv_sync};

/// `swoole::ProtocolType`, `SW_FD_PIPE`, `SW_EVENT_READ`, `SW_TIMEOUT_READ`,
/// `SW_EVENTLOOP_WAIT_EXIT`, `Reactor::PRIORITY_TRY_EXIT`,
/// and the `Channel` flags.
const PROTOCOL_TASK: c_int = 1;
const PROTOCOL_STREAM: c_int = 2;
const PROTOCOL_MESSAGE: c_int = 3;
const FD_PIPE: c_int = 3;
const EVENT_READ: c_int = 512;
const TIMEOUT_READ: c_int = 4;
const EVENTLOOP_WAIT_EXIT: c_int = 1;
const PRIORITY_TRY_EXIT: c_int = 4;
/// `sizeof(EventData)`.
const EVENT_SIZE: usize = SW_IPC_MAX_SIZE as usize;

/// A pool's message bus: the Rust bus, then the allocator its packets and buffer
/// come from. crates/process and crates/phpprocess read both through `message_bus`.
#[repr(C)]
struct PoolBus {
    bus: Box<MessageBus>,
    allocator: *const CoreAllocator,
}

/// `SwooleG.std_allocator`.
pub(crate) static STD_ALLOCATOR: CoreAllocator = CoreAllocator {
    malloc: Some(libc::malloc),
    calloc: Some(libc::calloc),
    realloc: Some(libc::realloc),
    free: Some(libc::free),
};

/// The pool every hook here runs over. Each entry point makes it once: from the
/// [`PoolCall`] crates/process hands it, or, in a callback C holds, from the pool
/// the callback was registered with.
type Pool<'a> = PoolRef<'a>;

/// A socket's fd, or -1 for none.
fn fd(socket: Option<Socket<'_>>) -> c_int {
    socket.map_or(-1, Socket::fd)
}

/// The pool's stream listener, or `None` without one.
fn ipc_listener(pool: Pool<'_>) -> Option<Socket<'_>> {
    // SAFETY: null, or the listening socket the pool's IPC owns until the pool frees it.
    unsafe { Socket::from_ptr(pool.ipc_listener().cast()) }
}

/// The socket a hook works on, or `None`.
fn call_socket(c: &PoolCall) -> Option<Socket<'_>> {
    // SAFETY: null, or the live native socket the call holds (`PoolCall::socket`).
    unsafe { Socket::from_ptr(c.socket().cast()) }
}

/// `socket->send_sync(data, length)` of the call's bytes on its socket, or -1 without one.
fn send_sync(c: &PoolCall) -> isize {
    call_socket(c).map_or(-1, |socket| upstream::send_sync(socket, c.bytes(), 0))
}

fn errno() -> c_int {
    std::io::Error::last_os_error().raw_os_error().unwrap_or(0)
}

/// `pool->onMessage(pool, &msg)`, whose `data` points at `info.len` bytes.
fn on_message(pool: Pool<'_>, msg: &RecvData) {
    if let Some(on_message) = pool.on_message() {
        // SAFETY: the pool that holds the callback, and the message its caller read:
        // `data` points at `info.len` bytes.
        unsafe { on_message(pool, msg) };
    }
}

fn exit_status(exited: u64) -> ExitStatus {
    ExitStatus::new((exited & 0xffff_ffff) as c_int, (exited >> 32) as c_int)
}

// ---------------------------------------------------------------- message bus

fn bus_of(pool: Pool<'_>) -> crate::objects::Bus {
    // SAFETY: the pool's bus, which `OP_NEW_MESSAGE_BUS` made.
    unsafe { crate::objects::Bus::of(pool.message_bus()) }
}

/// `MessageBus::read(sock)`.
fn bus_read(pool: Pool<'_>, sock: Option<Socket<'_>>) -> isize {
    sock.map_or(-1, |sock| bus_of(pool).read(sock))
}

/// `MessageBus::read(sock)` (`read_with_buffer` when `dgram`) over the bus and its
/// default allocator: allocates each packet block the bus asks for.
pub(crate) fn read_bus(
    bus: &MessageBus,
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
            swoole_rs_corebase::fatal("out of memory");
        }
        // SAFETY: the block just allocated, from `allocator`.
        unsafe { bus.adopt(data, state.size, ptr::from_ref(allocator).cast()) };
    }
}

/// `onMessage` with the bus's packet, then `message_bus->pop()`.
fn bus_deliver(pool: Pool<'_>) {
    let bus = bus_of(pool);
    let mut info = bus.head();
    let packet = bus.packet();
    info.len = packet.length.c_cast::<u32>();
    on_message(
        pool,
        &RecvData {
            info,
            data: packet.data,
        },
    );
    bus.pop();
}

/// The pool's new bus, which the pool keeps; null when its counter can't be allocated.
fn new_message_bus() -> *mut c_void {
    let Some([msg_id]) = mem_pool::words::<AtomicU64>(1) else {
        return ptr::null_mut();
    };
    *msg_id.get_mut() = 1;
    new_bus(msg_id)
}

/// `new MessageBus()` numbering its messages from `counter`; [`free_bus`] frees it.
pub(crate) fn new_bus(counter: &'static AtomicU64) -> *mut c_void {
    let bus = MessageBus::create(swoole_rs_corebase::constants::SW_BUFFER_SIZE_STD as usize);
    bus.count_ids(counter);
    let bus = PoolBus {
        bus,
        allocator: &raw const STD_ALLOCATOR,
    };
    Box::into_raw(Box::new(bus)).cast()
}

fn bus_buffer(pool: Pool<'_>, size: u64) -> i64 {
    i64::from(bus_of(pool).alloc_buffer(size.c_cast::<usize>()))
}

fn free_message_bus(pool: Pool<'_>) {
    // SAFETY: `new_message_bus`'s bus, freed once.
    unsafe { free_bus(pool.message_bus()) };
}

/// `delete bus`: the record and the Rust bus it owns.
///
/// # Safety
/// `bus` is [`new_bus`]'s, not used again.
pub(crate) unsafe fn free_bus(bus: *mut c_void) {
    // SAFETY: as the caller guarantees.
    drop(unsafe { Box::from_raw(bus.cast::<PoolBus>()) });
}

// ---------------------------------------------------------------- pipes and box

/// `pool->pipes`: each worker's `UnixSocket`.
type Pipes = Vec<*mut SocketPair>;

fn free_pipes(pool: Pool<'_>) {
    let pipes = pool.pipes().cast::<Pipes>();
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
fn new_pipe(pool: Pool<'_>, worker: Record<'_>) -> i64 {
    let mut built = Box::new(unix_socket(true, libc::SOCK_DGRAM));
    let (master, end) = (built.master_socket, built.worker_socket);
    if master.is_null() || end.is_null() {
        socket_pair::drop(&mut built);
        // The pool forgets the list when this fails.
        free_pipes(pool);
        return 0;
    }
    let pair = Box::into_raw(built);
    // SAFETY: `OP_NEW_PIPES` made the list.
    unsafe { &mut *pool.pipes().cast::<Pipes>() }.push(pair);
    worker.set_pipe_master(master.cast());
    worker.set_pipe_worker(end.cast());
    worker.set_pipe_object(pair.cast());
    1
}

// ---------------------------------------------------------------- timers and loop

/// The pool's `max_wait_time`, in the milliseconds a timer takes.
fn max_wait_ms(pool: Pool<'_>) -> c_long {
    c_long::from(pool.max_wait_time()) * 1000
}

/// A one-shot timer of the pool's `max_wait_time` that runs `f`, owned: dropping it
/// deletes the timer unless it has fired.
fn armed(pool: Pool<'_>, f: impl Fn() + Copy + 'static) -> Option<ArmedTimer> {
    let id = swoole_rs_timer::add_fn_id(max_wait_ms(pool), false, f)?;
    Some(ArmedTimer::new(id, swoole_rs_timer::timer_cancel))
}

thread_local! {
    /// The pool whose worker runs the loop `OP_EVENT_INIT` made on this thread, for
    /// that loop's handlers.
    static LOOP_POOL: Cell<Option<Pool<'static>>> = const { Cell::new(None) };
    /// The worker `OP_EXIT_CALLBACK` names.
    static EXITING: Cell<Option<Record<'static>>> = const { Cell::new(None) };
}

/// `onWorkerExit`, then stop the loop once it may exit.
extern "C-unwind" fn worker_exit() {
    if let (Some(pool), Some(worker)) = (LOOP_POOL.get(), EXITING.get()) {
        pool.call_on_worker_exit(worker);
    }
    // The loop running this end callback.
    let reactor = crate::objects::Reactor::thread();
    if reactor.if_exit() {
        reactor.handle().set_running(false);
    }
}

/// `ProcessPool::recv_packet`: the event loop's pipe handler under the stream protocol.
extern "C" fn recv_packet(_: Running<'_>, event: &mut Event) -> c_int {
    let Some(pool) = LOOP_POOL.get() else {
        return 0;
    };
    let packet = pool.ipc_packet();
    let size = pool.max_packet_size() as usize;
    let fd = fd(event.socket());
    // SAFETY: the packet buffer holds `max_packet_size` bytes.
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
extern "C" fn recv_message(_: Running<'_>, event: &mut Event) -> c_int {
    if let Some(pool) = LOOP_POOL.get()
        && bus_read(pool, event.socket()) > 0
    {
        bus_deliver(pool);
    }
    0
}

/// A worker loop's result; a callback's bailout is raised once the export's frames
/// are gone.
fn main_result(result: i64) -> c_int {
    if result == RAISE {
        raise_bailout();
    }
    result.c_cast::<c_int>()
}

/// `ProcessPool::run_with_*_protocol` and `run_async`: the worker loop.
macro_rules! main_loops {
    ($($name:ident = $protocol:expr;)*) => {$(
        extern "C-unwind" fn $name(pool: PoolRef<'_>, worker: Record<'_>) -> c_int {
            main_result(pool_run_main(&NATIVE, pool, worker, $protocol))
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
fn loop_ops(pool: Pool<'_>, op: u32, arg: u64) -> Option<i64> {
    Some(match op {
        OP_EVENT_INIT => {
            // This thread has no event loop yet.
            if swoole_rs_reactor::swoole_event_init(EVENTLOOP_WAIT_EXIT) < 0 {
                return Some(-1);
            }
            // The loop just made.
            crate::objects::Reactor::thread()
                .handle()
                .set_object(pool.raw().cast());
            // SAFETY: the pool outlives the loop its worker runs on this thread, and
            // only that loop's handlers read the handle.
            LOOP_POOL.set(Some(unsafe { PoolRef::new(pool.raw()) }));
            0
        }
        OP_PIPE_HANDLER => {
            let handler = if arg == 0 { recv_packet } else { recv_message };
            swoole_event_set_handler(FD_PIPE, EVENT_READ, Some(handler));
            0
        }
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
        OP_EVENT_WAIT => i64::from(swoole_rs_reactor::event_api::swoole_event_wait()),
        OP_EVENT_AVAILABLE => i64::from(swoole_event_is_available()),
        OP_WAITPID => i64::from(swoole_rs_reactor::wait_upstream::swoole_waitpid(
            arg.c_cast::<c_int>(),
            Some(&mut 0),
            0,
        )),
        _ => return None,
    })
}

/// An object's address as an op's result, which the pool stores (null fails).
fn address<T>(object: *mut T) -> i64 {
    object.expose_provenance().c_cast::<i64>()
}

/// `pool_call`'s lifecycle effects: the pool's objects.
fn objects(pool: Pool<'_>, op: u32, arg: u64) -> Option<i64> {
    Some(match op {
        // The message queue is Rust's on every platform (crates/ossupport).
        OP_HAVE_MSGQUEUE => 1,
        OP_ALLOC_WORKERS => {
            // `arg` workers, which the pool counts once they're allocated.
            let size = arg.c_cast::<usize>() * size_of::<WorkerRecord>();
            address(mem_pool::alloc(size.c_cast::<u32>()))
        }
        OP_NEW_PIPES => address(Box::into_raw(Box::new(Pipes::new()))),
        OP_MAIN_LOOP => {
            let main_loop: MainLoopFn = match arg {
                1 => run_task,
                2 => run_stream,
                3 => run_message,
                _ => run_async,
            };
            let hooks = Hooks {
                main_loop: Some(main_loop),
                ..Hooks::default()
            };
            // SAFETY: these loops run crates/process' own worker loop over the pool
            // and worker they are given, and read nothing through the pool's `ptr`.
            unsafe { pool.install(&hooks) };
            0
        }
        OP_NEW_MESSAGE_BUS => address(new_message_bus()),
        OP_BUS_BUFFER => bus_buffer(pool, arg),
        OP_FREE_PIPES => {
            free_pipes(pool);
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

/// `pool_call`'s effects on its worker: its pipes, globals and callbacks.
fn worker_ops(pool: Pool<'_>, worker: Record<'_>, op: u32, arg: u64) -> Option<i64> {
    Some(match op {
        OP_NEW_PIPE => new_pipe(pool, worker),
        OP_SNDBUF => {
            let mut size: c_int = 0;
            let fd = fd(worker_pipe(worker, true));
            let failed = swoole_rs_corebase::sys::getsockopt_int(
                fd,
                libc::SOL_SOCKET,
                libc::SO_SNDBUF,
                &mut size,
            );
            if failed != 0 { -1 } else { i64::from(size) }
        }
        OP_UNLIMIT_PIPES => {
            for master in [false, true] {
                if let Some(end) = worker_pipe(worker, master) {
                    end.edit(|end| end.output.buffer_size = u32::MAX);
                }
            }
            0
        }
        OP_WORKER_INIT => {
            swr_v1_worker_init(worker);
            worker.set_pid(std::process::id().c_cast::<c_int>());
            swoole_rs_process::methods::set_worker_type(WorkerType::Event);
            swoole_set_worker_id(worker.id());
            swoole_set_worker_pid(worker.pid());
            swoole_rs_process::worker_globals::set_current_worker(worker.raw());
            0
        }
        OP_WORKER_SHUTDOWN => {
            swr_v1_worker_shutdown(worker);
            0
        }
        OP_REMOVE_PIPE => {
            // arg: only when the pipe is still in the reactor.
            if let Some(pipe) = worker_pipe(worker, false)
                && (arg == 0 || pipe.read(|pipe| !pipe.removed))
            {
                Reactor::thread().del(pipe);
            }
            0
        }
        OP_EXIT_CALLBACK => {
            // SAFETY: the worker of the loop this thread runs, which outlives the
            // loop's end callback, the one reader.
            EXITING.set(Some(unsafe { Record::new(worker.raw()) }));
            swoole_rs_reactor::swr_v1_event_set_end_callback(PRIORITY_TRY_EXIT, worker_exit);
            0
        }
        OP_DONT_RESTART => {
            if let Some(pipe) = worker_pipe(worker, false) {
                pipe.edit(|pipe| pipe.dont_restart = true);
            }
            0
        }
        OP_ADD_PIPE => {
            let add = |pipe| Reactor::thread().add(pipe, EVENT_READ);
            i64::from(worker_pipe(worker, false).map_or(-1, add))
        }
        OP_PUSH_STOP => i64::from(push_stop(pool, worker.pid(), worker.id().c_cast::<u16>())),
        OP_WORKER_ERROR => {
            let status = exit_status(arg);
            worker_report_error(worker, &status);
            pool.call_on_worker_error(worker, &status);
            0
        }
        _ => return None,
    })
}

/// `pool_call`'s timers, messages and callbacks.
fn master_ops(pool: Pool<'_>, op: u32, arg: u64) -> i64 {
    match op {
        OP_REOPEN_LOGGER => {
            swoole_rs_utilities::process_logger()
                .into_iter()
                .for_each(swoole_rs_utilities::Logger::reopen);
        }
        OP_RELOAD_TIMER => {
            let task = pool.reload_task();
            let timer = armed(pool, move || {
                // SAFETY: the pool's live reload task, which owns the timer
                // (`set_timer`), so dropping the task deletes it.
                let task = unsafe { &mut *task };
                task.kill_all(libc::SIGKILL, reload_warn);
            });
            // SAFETY: the pool's live reload task.
            unsafe { &mut *task }.set_timer(timer);
        }
        OP_TIMER_INIT => {
            if swoole_timer_is_available() {
                crate::objects::timer_reinit(true);
            } else {
                // This thread has no timer.
                swoole_timer_create(true);
            }
        }
        OP_POP_MESSAGE => {
            return POOL_MESSAGE.with_borrow_mut(|message| {
                if pop_message(pool, &mut message.0) > 0 {
                    message.0.as_mut_ptr().expose_provenance().c_cast::<i64>()
                } else {
                    0
                }
            });
        }
        OP_ON_WORKER_MESSAGE => {
            if let Some(on_worker_message) = pool.on_worker_message() {
                let message = POOL_MESSAGE.with_borrow_mut(|message| message.0.as_mut_ptr());
                // SAFETY: the message `OP_POP_MESSAGE` just popped.
                unsafe { on_worker_message(pool, message.cast()) };
            }
        }
        OP_ON_WORKER_NOT_FOUND => {
            let _ = pool.call_on_worker_not_found(&exit_status(arg));
        }
        OP_ON_AFTER_RELOAD => {
            pool.call_on_after_reload();
        }
        OP_KILL_TIMER => {
            // The pool owns the timer in `kill_timer`, so `wait`, `destroy` or
            // freeing the pool deletes it.
            let raw = pool.raw();
            pool.set_kill_timer(armed(pool, move || {
                // SAFETY: the pool that owns the timer, so it is live while it can fire.
                kill_all_workers(unsafe { PoolRef::new(raw) }, libc::SIGKILL);
            }));
        }
        _ => {}
    }
    0
}

/// An `EventData`'s bytes, aligned as one.
#[repr(C, align(8))]
struct Message([u8; EVENT_SIZE]);

thread_local! {
    /// The worker's stop messages and the others the master pops while it waits.
    static POOL_MESSAGE: RefCell<Box<Message>> = RefCell::new(Box::new(Message([0; EVENT_SIZE])));
}

/// Runs `io` and records its result and errno in the call.
fn io(c: &mut PoolCall, io: impl FnOnce(&PoolCall) -> isize) {
    let result = io(c);
    c.set_result(result);
    c.set_error(errno());
}

/// The upstream method each worker loop's warnings came from.
fn loop_origin(pool: Pool<'_>) -> &'static CStr {
    match pool.protocol_type() {
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
        let pool = c.pool();
        objects(pool, op, arg)
            .or_else(|| loop_ops(pool, op, arg))
            .or_else(|| {
                c.worker()
                    .and_then(|worker| worker_ops(pool, worker, op, arg))
            })
            .unwrap_or_else(|| master_ops(pool, op, arg))
    }

    /// The pool's callbacks, inside the bailout trap.
    fn callback(&self, c: &mut PoolCall) {
        let (pool, worker) = (c.pool(), c.worker());
        match (c.flags(), worker) {
            (CB_START, _) => pool.call_on_start(),
            (CB_BEFORE_RELOAD, _) => pool.call_on_before_reload(),
            (CB_SHUTDOWN, _) => pool.call_on_shutdown(),
            (CB_WORKER_START, Some(worker)) => pool.call_on_worker_start(worker),
            (CB_WORKER_STOP, Some(worker)) => pool.call_on_worker_stop(worker),
            (CB_MAIN_LOOP, Some(worker)) => {
                if let Some(code) = pool.call_main_loop(worker) {
                    c.set_result(code as isize);
                }
            }
            _ => {}
        }
    }

    fn event_size(&self) -> usize {
        EVENT_SIZE
    }

    fn running(&self, c: &PoolCall) -> bool {
        c.worker()
            .is_some_and(|worker| is_worker_running(c.pool(), worker))
    }

    fn end(&self) {
        swoole_timer_select();
        swoole_rs_process::signal::swoole_signal_dispatch();
    }

    fn queue_pop(&self, c: &mut PoolCall) {
        io(c, |c| {
            let queue = c.pool().queue();
            if queue.is_null() {
                return -1;
            }
            // SAFETY: the pool's queue and the call's buffer.
            unsafe {
                swoole_rs_ossupport::swr_v1_os_msg_queue_pop(
                    queue.cast(),
                    c.data().cast(),
                    c.length(),
                )
            }
        });
    }

    fn queue_push(&self, c: &mut PoolCall) {
        io(c, |c| {
            let queue = c.pool().queue();
            if queue.is_null() {
                return 0;
            }
            // SAFETY: the pool's queue and the call's message.
            let pushed = unsafe {
                swoole_rs_ossupport::swr_v1_os_msg_queue_push(
                    queue.cast(),
                    c.data().cast(),
                    c.length(),
                )
            };
            isize::from(pushed)
        });
    }

    fn accept(&self, c: &mut PoolCall) {
        let socket = ipc_listener(c.pool()).map_or(ptr::null_mut(), |listener| {
            listener.edit(swoole_rs_reactor::SocketFields::accept)
        });
        // SAFETY: null, or the connection the listener just accepted, which the pool frees.
        unsafe { c.set_socket(socket.cast()) };
        c.set_error(errno());
    }

    fn recv(&self, c: &mut PoolCall) {
        io(c, |c| {
            let (data, length, flags) = (c.data(), c.length(), c.flags());
            // SAFETY: the call's connection and buffer.
            let recv = |socket| unsafe { socket_recv_sync(socket, data, length, flags) };
            call_socket(c).map_or(-1, recv)
        });
    }

    fn send(&self, c: &mut PoolCall) {
        io(c, send_sync);
    }

    fn close(&self, c: &PoolCall) {
        // SAFETY: a connection the loop is done with.
        unsafe { socket_free(c.socket().cast()) };
    }

    fn pipe_read(&self, c: &mut PoolCall) {
        io(c, |c| {
            let Some(pipe) = c.worker().and_then(|worker| worker_pipe(worker, false)) else {
                return -1;
            };
            let next = swoole_timer_get_next_msec();
            let timeout = next.c_cast::<f64>() / 1000.0;
            pipe.edit(|pipe| pipe.timeouts.set(timeout, TIMEOUT_READ));
            // SAFETY: the worker's pipe and the call's buffer.
            unsafe { socket_read_sync(pipe, c.data(), c.length()) }
        });
    }

    fn pipe_wait(&self, c: &mut PoolCall) {
        io(c, |c| {
            let next = swoole_timer_get_next_msec();
            c.worker()
                .and_then(|worker| worker_pipe(worker, false))
                .map_or(-1, |pipe| {
                    pipe.wait_event(next.c_cast::<c_int>(), EVENT_READ) as isize
                })
        });
    }

    fn bus_read(&self, c: &mut PoolCall) {
        io(c, |c| {
            c.worker()
                .map_or(-1, |worker| bus_read(c.pool(), worker_pipe(worker, false)))
        });
    }

    fn bus_deliver(&self, c: &mut PoolCall) {
        bus_deliver(c.pool());
    }

    fn task(&self, c: &mut PoolCall) {
        let (pool, worker) = (c.pool(), c.worker());
        if let Some(worker) = worker {
            // SAFETY: the task the worker read into the call's buffer, its head and
            // the `len` bytes after it.
            let task = unsafe { Packed::from_raw(c.data().cast()) };
            if let Some(code) = pool.call_on_task(worker, task) {
                c.set_result(code as isize);
            }
        }
    }

    fn message(&self, c: &mut PoolCall) {
        let info = DataHead {
            reactor_id: -1,
            len: c.length().c_cast::<u32>(),
            ..DataHead::default()
        };
        on_message(
            c.pool(),
            &RecvData {
                info,
                data: c.data().cast(),
            },
        );
    }

    fn shutdown(&self, c: &PoolCall) {
        if let Some(worker) = c.worker() {
            swr_v1_worker_shutdown(worker);
        }
    }

    fn pipe_write(&self, c: &mut PoolCall) {
        io(c, |c| {
            if c.flags() != 0 && swoole_event_is_available() {
                // SAFETY: the worker's pipe, in this thread's loop.
                unsafe { swoole_event_write(c.socket().cast(), c.data(), c.length()) }
            } else {
                send_sync(c)
            }
        });
    }

    /// A `WARN_*`; the `TRACE_*` logs exist only in trace builds, which this one isn't.
    fn warn(&self, c: &PoolCall, kind: c_int, a: i64, b: i64) {
        let (pool, worker) = (c.pool(), c.worker());
        let id = worker.map_or(0, Record::id);
        let origin = loop_origin(pool);
        match kind {
            WARN_MSGRCV => {
                let queue = pool.queue_id();
                log::sys_warning(origin, format_args!("[Worker#{id}] msgrcv({queue}) failed"));
            }
            WARN_ACCEPT => {
                log::sys_warning(
                    origin,
                    format_args!("[Worker#{id}] accept({}) failed", fd(ipc_listener(pool))),
                );
            }
            WARN_READ => {
                let fd = fd(worker.and_then(|worker| worker_pipe(worker, false)));
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
                format_args!(
                    "send {} bytes to worker#{} failed",
                    a.c_cast::<c_int>(),
                    b.c_cast::<c_int>()
                ),
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
