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
pub(crate) use crate::reactor_ops::{PoolBus, STD_ALLOCATOR};
use core::sync::atomic::AtomicU64;
use swoole_rs_bus::MessageBus;
use swoole_rs_corebase::DataHead;
use swoole_rs_corebase::constants::SW_IPC_MAX_SIZE;
use swoole_rs_corebase::log;
use swoole_rs_memory::members::pool as mem_pool;
use swoole_rs_process::armed_timer::ArmedTimer;
use swoole_rs_process::methods::{
    ExitStatus, is_worker_running, kill_all_workers, pop_message, push_stop, reload_warn,
    swoole_set_worker_id, swoole_set_worker_pid, worker_report_error,
};
use swoole_rs_process::pool::{PoolCall, Warning, raise_bailout};
use swoole_rs_process::process_pool::{
    Callback, Hooks, MainLoopFn, Op, PoolNative, PoolRef, Protocol, RAISE, RecvData, pool_run_main,
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
use swoole_rs_tls::upstream::{self, read_sync_fill, recv_sync_fill, socket_free};

/// `swoole::ProtocolType`, `SW_FD_PIPE`, `SW_EVENT_READ`, `SW_TIMEOUT_READ`,
/// `SW_EVENTLOOP_WAIT_EXIT` and the `Channel` flags.
const FD_PIPE: c_int = 3;
const EVENT_READ: c_int = swoole_rs_corebase::constants::EventType::SW_EVENT_READ as c_int;
const TIMEOUT_READ: c_int = 4;
const EVENTLOOP_WAIT_EXIT: c_int = 1;
/// `sizeof(EventData)`.
const EVENT_SIZE: usize = SW_IPC_MAX_SIZE as usize;

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
    crate::view::pool_ipc_listener(pool)
}

/// The socket a hook works on, or `None`.
fn call_socket<'a>(c: &'a PoolCall<'_>) -> Option<Socket<'a>> {
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

/// The pool's bus, which `Op::NewMessageBus` made, or `None` without one.
fn bus_of(pool: Pool<'_>) -> Option<crate::objects::Bus<'_>> {
    PoolBus::of_pool(pool).map(crate::objects::Bus::over)
}

/// `MessageBus::read(sock)`; -1 without a socket or a bus.
fn bus_read(pool: Pool<'_>, sock: Option<Socket<'_>>) -> isize {
    sock.zip(bus_of(pool))
        .map_or(-1, |(sock, bus)| bus.read(sock))
}

/// `onMessage` with the bus's packet, then `message_bus->pop()`.
fn bus_deliver(pool: Pool<'_>) {
    let Some(bus) = bus_of(pool) else {
        return;
    };
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
    Box::into_raw(Box::new(new_bus(msg_id))).cast()
}

/// `new MessageBus()` numbering its messages from `counter`. Its owner boxes and
/// leaks it into the `MessageBus *` it keeps, or owns it.
pub(crate) fn new_bus(counter: &'static AtomicU64) -> PoolBus {
    let bus = MessageBus::create(swoole_rs_corebase::constants::SW_BUFFER_SIZE_STD as usize);
    bus.count_ids(counter);
    PoolBus::std(bus)
}

fn bus_buffer(pool: Pool<'_>, size: u64) -> i64 {
    i64::from(bus_of(pool).is_some_and(|bus| bus.alloc_buffer(size.c_cast::<usize>())))
}

/// `delete bus`: the record and the Rust bus it owns.
fn free_message_bus(pool: Pool<'_>) {
    // SAFETY: `new_message_bus`'s box, freed once: the pool forgets it after this op.
    drop(unsafe { Box::from_raw(pool.message_bus().cast::<PoolBus>()) });
}

// ---------------------------------------------------------------- pipes and box

/// `pool->pipes`: each worker's `UnixSocket`, boxed so a worker's record keeps its
/// pair's address while the list grows.
type Pipes = Vec<Box<SocketPair>>;

fn free_pipes(pool: Pool<'_>) {
    let pipes = pool.pipes().cast::<Pipes>();
    if pipes.is_null() {
        return;
    }
    // SAFETY: `Op::NewPipes`'s box, freed once.
    for mut pair in *unsafe { Box::from_raw(pipes) } {
        socket_pair::drop(&mut pair);
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
    // SAFETY: `Op::NewPipes` made the list.
    let pipes = unsafe { &mut *pool.pipes().cast::<Pipes>() };
    pipes.push(built);
    // The box keeps the pair at this address while the list owns it.
    let pair = pipes
        .last_mut()
        .map_or(ptr::null_mut(), |pair| &raw mut **pair);
    // SAFETY: the two ends of the pair the pool's list now owns until `free_pipes`,
    // in the pool's `destroy`, after which this process sends to its workers no more.
    unsafe { worker.set_pipes(pair.cast(), master.cast(), end.cast()) };
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
    let (generation, id) = swoole_rs_timer::add_fn_held(max_wait_ms(pool), false, f)?.parts();
    Some(ArmedTimer::new(
        generation,
        id,
        swoole_rs_timer::cancel_issued,
    ))
}

thread_local! {
    /// The pool whose worker runs the loop `Op::EventInit` made on this thread, for
    /// that loop's handlers.
    static LOOP_POOL: Cell<Option<Pool<'static>>> = const { Cell::new(None) };
    /// The worker `Op::ExitCallback` names.
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
    run_task = Some(Protocol::Task);
    run_stream = Some(Protocol::Stream);
    run_message = Some(Protocol::Message);
    run_async = None;
}

// ---------------------------------------------------------------- the table

/// The effects on the thread's timer and event loop, and the blocking wait.
fn loop_ops(pool: Pool<'_>, op: Op, arg: u64) -> Option<i64> {
    Some(match op {
        Op::EventInit => {
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
        Op::PipeHandler => {
            let handler = if arg == 0 { recv_packet } else { recv_message };
            swoole_event_set_handler(FD_PIPE, EVENT_READ, Some(handler));
            0
        }
        Op::TimerSelect => {
            swoole_timer_select();
            0
        }
        Op::TimerFree => {
            if swoole_timer_is_available() {
                swoole_timer_free();
            }
            0
        }
        Op::EventWait => i64::from(swoole_rs_reactor::event_api::swoole_event_wait()),
        Op::EventAvailable => i64::from(swoole_event_is_available()),
        Op::Waitpid => i64::from(swoole_rs_reactor::wait_upstream::swoole_waitpid(
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
fn objects(pool: Pool<'_>, op: Op, arg: u64) -> Option<i64> {
    Some(match op {
        // The message queue is Rust's on every platform (crates/ossupport).
        Op::HaveMsgqueue => 1,
        Op::AllocWorkers => {
            // `arg` workers, which the pool counts once they're allocated.
            let size = arg.c_cast::<usize>() * size_of::<WorkerRecord>();
            address(mem_pool::alloc(size.c_cast::<u32>()))
        }
        Op::NewPipes => address(Box::into_raw(Box::new(Pipes::new()))),
        Op::MainLoop => {
            let main_loop: MainLoopFn = match Protocol::from_code(arg.c_cast::<c_int>()) {
                Some(Protocol::Task) => run_task,
                Some(Protocol::Stream) => run_stream,
                Some(Protocol::Message) => run_message,
                None => run_async,
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
        Op::NewMessageBus => address(new_message_bus()),
        Op::BusBuffer => bus_buffer(pool, arg),
        Op::FreePipes => {
            free_pipes(pool);
            0
        }
        Op::FreeMessageBus => {
            free_message_bus(pool);
            0
        }
        // The global pool's `free` is a no-op.
        Op::FreeWorkers => 0,
        _ => return None,
    })
}

/// `pool_call`'s effects on its worker: its pipes, globals and callbacks.
fn worker_ops(pool: Pool<'_>, worker: Record<'_>, op: Op, arg: u64) -> Option<i64> {
    Some(match op {
        Op::NewPipe => new_pipe(pool, worker),
        Op::Sndbuf => {
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
        Op::UnlimitPipes => {
            for master in [false, true] {
                if let Some(end) = worker_pipe(worker, master) {
                    end.edit(|end| end.output.buffer_size = u32::MAX);
                }
            }
            0
        }
        Op::WorkerInit => {
            swr_v1_worker_init(worker);
            worker.set_pid(std::process::id().c_cast::<c_int>());
            swoole_rs_process::methods::set_worker_type(WorkerType::Event);
            swoole_set_worker_id(worker.id());
            swoole_set_worker_pid(worker.pid());
            swoole_rs_process::worker_globals::set_current_worker(worker.raw());
            0
        }
        Op::WorkerShutdown => {
            swr_v1_worker_shutdown(worker);
            0
        }
        Op::RemovePipe => {
            // arg: only when the pipe is still in the reactor.
            if let Some(pipe) = worker_pipe(worker, false)
                && (arg == 0 || pipe.read(|pipe| !pipe.removed))
            {
                Reactor::thread().del(pipe);
            }
            0
        }
        Op::ExitCallback => {
            // SAFETY: the worker of the loop this thread runs, which outlives the
            // loop's end callback, the one reader.
            EXITING.set(Some(unsafe { Record::new(worker.raw()) }));
            swoole_rs_reactor::swr_v1_event_set_end_callback(
                swoole_rs_reactor::Priority::TryExit,
                worker_exit,
            );
            0
        }
        Op::DontRestart => {
            if let Some(pipe) = worker_pipe(worker, false) {
                pipe.edit(|pipe| pipe.dont_restart = true);
            }
            0
        }
        Op::AddPipe => {
            let add = |pipe| Reactor::thread().add(pipe, EVENT_READ);
            i64::from(worker_pipe(worker, false).map_or(-1, add))
        }
        Op::PushStop => {
            if push_stop(pool, worker.pid(), worker.id().c_cast::<u16>()) {
                0
            } else {
                -1
            }
        }
        Op::WorkerError => {
            let status = exit_status(arg);
            worker_report_error(worker, &status);
            pool.call_on_worker_error(worker, &status);
            0
        }
        _ => return None,
    })
}

/// `pool_call`'s timers, messages and callbacks.
fn master_ops(pool: Pool<'_>, op: Op, arg: u64) -> i64 {
    match op {
        Op::ReopenLogger => {
            swoole_rs_utilities::process_logger()
                .into_iter()
                .for_each(swoole_rs_utilities::Logger::reopen);
        }
        Op::ReloadTimer => {
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
        Op::TimerInit => {
            if swoole_timer_is_available() {
                crate::objects::timer_reinit(true);
            } else {
                // This thread has no timer.
                swoole_timer_create(true);
            }
        }
        Op::PopMessage => {
            return POOL_MESSAGE.with_borrow_mut(|message| {
                if pop_message(pool, &mut message.0).is_some_and(|length| length > 0) {
                    message.0.as_mut_ptr().expose_provenance().c_cast::<i64>()
                } else {
                    0
                }
            });
        }
        Op::OnWorkerMessage => {
            if let Some(on_worker_message) = pool.on_worker_message() {
                let message = POOL_MESSAGE.with_borrow_mut(|message| message.0.as_mut_ptr());
                // SAFETY: the message `Op::PopMessage` just popped.
                unsafe { on_worker_message(pool, message.cast()) };
            }
        }
        Op::OnWorkerNotFound => {
            pool.call_on_worker_not_found(&exit_status(arg));
        }
        Op::OnAfterReload => {
            pool.call_on_after_reload();
        }
        Op::KillTimer => {
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
    match Protocol::from_code(pool.protocol_type()) {
        Some(Protocol::Stream) => c"ProcessPool::run_with_stream_protocol",
        Some(Protocol::Message) => c"ProcessPool::run_with_message_protocol",
        _ => c"ProcessPool::run_with_task_protocol",
    }
}

/// The objects and callbacks every `ProcessPool` method crates/process defines runs over.
struct Native;

static NATIVE: Native = Native;

impl PoolNative for Native {
    fn call(&self, c: &PoolCall, op: Op, arg: u64) -> i64 {
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
    fn callback(&self, c: &mut PoolCall, callback: Callback) {
        let (pool, worker) = (c.pool(), c.worker());
        match (callback, worker) {
            (Callback::Start, _) => pool.call_on_start(),
            (Callback::BeforeReload, _) => pool.call_on_before_reload(),
            (Callback::Shutdown, _) => pool.call_on_shutdown(),
            (Callback::WorkerStart, Some(worker)) => pool.call_on_worker_start(worker),
            (Callback::WorkerStop, Some(worker)) => pool.call_on_worker_stop(worker),
            (Callback::MainLoop, Some(worker)) => {
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
            // SAFETY: the call's buffer.
            let buffer = unsafe { swoole_rs_reactor::Fill::from_raw(data, length) };
            call_socket(c).map_or(-1, |socket| recv_sync_fill(socket, buffer, flags))
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
            // SAFETY: the call's buffer.
            let buffer = unsafe { swoole_rs_reactor::Fill::from_raw(c.data(), c.length()) };
            read_sync_fill(pipe, buffer)
        });
    }

    fn pipe_wait(&self, c: &mut PoolCall) {
        io(c, |c| {
            let next = swoole_timer_get_next_msec();
            let ready = c
                .worker()
                .and_then(|worker| worker_pipe(worker, false))
                .is_some_and(|pipe| pipe.wait_event(next.c_cast::<c_int>(), EVENT_READ));
            if ready { 0 } else { -1 }
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

    fn bus_write(
        &self,
        pool: PoolRef<'_>,
        worker: Record<'_>,
        head: &mut DataHead,
        message: &[u8],
    ) -> bool {
        worker_pipe(worker, true)
            .zip(bus_of(pool))
            .is_some_and(|(pipe, bus)| bus.write(pipe, head, message))
    }

    fn task(&self, c: &mut PoolCall) {
        let (pool, worker) = (c.pool(), c.worker());
        // The task the worker read into the call's buffer: its head and the `len`
        // bytes after it, which the loop checked fit.
        if let Some((worker, task)) = worker.zip(Packed::from_bytes(c.bytes()))
            && let Some(code) = pool.call_on_task(worker, task)
        {
            c.set_result(code as isize);
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

    /// A worker loop's warning. Upstream's trace logs exist only in trace builds,
    /// which this one isn't, so nothing reports them.
    fn warn(&self, c: &PoolCall, warning: Warning) {
        let (pool, worker) = (c.pool(), c.worker());
        let id = worker.map_or(0, Record::id);
        let origin = loop_origin(pool);
        match warning {
            Warning::Msgrcv => {
                let queue = pool.queue_id();
                log::sys_warning(origin, format_args!("[Worker#{id}] msgrcv({queue}) failed"));
            }
            Warning::Accept => {
                log::sys_warning(
                    origin,
                    format_args!("[Worker#{id}] accept({}) failed", fd(ipc_listener(pool))),
                );
            }
            Warning::Read => {
                let fd = fd(worker.and_then(|worker| worker_pipe(worker, false)));
                log::sys_warning(origin, format_args!("[Worker#{id}] read({fd}) failed"));
            }
            Warning::BadPacket { received, length } => log::error_log(
                4, // SW_LOG_WARNING
                swoole_rs_error::ErrorCode::SW_ERROR_PROTOCOL_ERROR as c_int,
                origin,
                format_args!(
                    "[Worker#{id}] bad task packet, received data length[{received}], packet length[{length}]"
                ),
            ),
            Warning::TaskFailed(task) => log::warning(
                origin,
                format_args!("[Worker#{id}] the execution of task#{task} has failed"),
            ),
            Warning::Pipe => log::sys_warning(
                origin,
                format_args!("[Worker #{id}]failed to read data from pipe"),
            ),
            Warning::Send { size, worker } => log::warning(
                c"ProcessPool::dispatch",
                format_args!(
                    "send {} bytes to worker#{} failed",
                    size.c_cast::<c_int>(),
                    worker.c_cast::<c_int>()
                ),
            ),
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
