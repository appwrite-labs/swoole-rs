//! The objects `ProcessPool` runs over (once `src/os/process_pool.cc`): its worker pipes, message bus,
//! timers, event loop and callbacks. `corebase::process` decides every step
//! (`process_pool.rs`, `pool.rs`) and asks for each effect through the
//! [`PoolNative`] side [`process_pool_native`] returns. It lives here because these objects are
//! `corebase::reactor`'s, `corebase::tls`'s and `corebase::memory`'s, which `corebase::process` can't
//! depend on.
//!
//! Each hook that can reach PHP runs inside the bailout trap `corebase::process` sets
//! up; the frames here hold nothing to drop, so the `longjmp` may cross them.
use core::cell::Cell;
use core::ffi::{CStr, c_int, c_long};
use process::worker_record::Worker as Record;
use process::worker_record::WorkerType;
use std::rc::Rc;
use swoole_rs_corebase::CCast;
use swoole_rs_corebase::process;
use swoole_rs_corebase::reactor::ErrorHandler::ConnectionDefault;
use swoole_rs_corebase::reactor::Events;

use crate::objects::{Reactor, worker_pipe};
pub(crate) use crate::reactor_ops::PoolBus;
use core::sync::atomic::AtomicU64;
use process::armed_timer::ArmedTimer;
use process::methods::{
    ExitStatus, is_worker_running, kill_all_workers, push_stop, reload_warn, swoole_set_worker_id,
    swoole_set_worker_pid, worker_report_error,
};
use process::pool::{PoolCall, Warning};
use process::process_pool::{
    Callback, CallbackError, CallbackResult, Hooks, MainLoopFn, Op, PipeError, PoolNative, PoolRef,
    Protocol, RecvData, pool_run_main,
};
use process::task_file::Packed;
use process::worker_record::{WorkerRecord, swr_v1_worker_init, swr_v1_worker_shutdown};
use swoole_rs_bus::MessageBus;
use swoole_rs_corebase::DataHead;
use swoole_rs_corebase::constants::SW_IPC_MAX_SIZE;
use swoole_rs_corebase::log;
use swoole_rs_corebase::memory::members::pool as mem_pool;
use swoole_rs_corebase::reactor::event_api::{
    event_write, set_handlers, swoole_event_is_available,
};
use swoole_rs_corebase::reactor::{Event, Heap, Running, Socket};
use swoole_rs_corebase::timer::{
    swoole_timer_create, swoole_timer_free, swoole_timer_get_next_msec, swoole_timer_is_available,
    swoole_timer_select,
};
use swoole_rs_corebase::tls::socket_pair::{self, SocketPair, unix_socket};
use swoole_rs_corebase::tls::upstream::{self, socket_free};

/// `swoole::ProtocolType`, `SW_FD_PIPE`, `SW_EVENT_READ`,
/// `SW_EVENTLOOP_WAIT_EXIT` and the `Channel` flags.
const FD_PIPE: c_int = 3;
const EVENTLOOP_WAIT_EXIT: c_int = 1;
/// `sizeof(EventData)`.
const EVENT_SIZE: usize = SW_IPC_MAX_SIZE as usize;

/// The pool every hook here runs over. Each entry point makes it once: from the
/// [`PoolCall`] `corebase::process` hands it, or, in a callback C holds, from the pool
/// the callback was registered with.
type Pool<'a> = PoolRef<'a>;

/// A socket's fd, or -1 for none.
fn fd(socket: Option<Heap<'_>>) -> c_int {
    socket.map_or(-1, |socket| socket.fd())
}

use swoole_rs_corebase::sys::errno;

fn exit_status(exited: u64) -> ExitStatus {
    ExitStatus::new((exited & 0xffff_ffff) as c_int, (exited >> 32) as c_int)
}

// ---------------------------------------------------------------- message bus

/// An operation over the pool's bus; the borrow ends before any PHP callback.
fn with_bus<R>(pool: Pool<'_>, f: impl FnOnce(crate::objects::Bus<'_>) -> R) -> Option<R> {
    PoolBus::with_pool(pool, |bus| f(crate::objects::Bus::over(bus)))
}

/// `MessageBus::read(sock)`; `None` (upstream's -1) without a socket or a bus too.
fn bus_read(pool: Pool<'_>, sock: Option<Heap<'_>>) -> Option<usize> {
    sock.and_then(|sock| with_bus(pool, |bus| bus.read(*sock)).flatten())
}

/// `onMessage` with the bus's packet, taken out of the bus meanwhile (`onMessage`
/// may take it), then `message_bus->pop()`.
fn bus_deliver(pool: Pool<'_>) {
    let taken = with_bus(pool, |bus| (bus.head(), bus.rust().take_packet()));
    let Some((info, mut packet)) = taken else {
        return;
    };
    pool.call_on_message(&mut packet.receive(info));
    PoolBus::with_pool(pool, |bus| {
        let bus = bus.parts().0;
        bus.restore(packet);
        if !process::pool::bailout_pending() {
            bus.pop();
        }
    });
}

/// The pool's new bus, owned, or None when its counter can't be allocated.
fn new_message_bus() -> Option<Box<dyn core::any::Any>> {
    let [msg_id] = mem_pool::words::<AtomicU64>(1)? else {
        return None;
    };
    *msg_id.get_mut() = 1;
    Some(Box::new(new_bus(msg_id)))
}

/// `new MessageBus()` numbering its messages from `counter`.
pub(crate) fn new_bus(counter: &'static AtomicU64) -> PoolBus {
    let bus = MessageBus::create(swoole_rs_corebase::constants::SW_BUFFER_SIZE_STD as usize);
    bus.count_ids(counter);
    PoolBus::std(bus)
}

// ---------------------------------------------------------------- pipes and box

/// A worker's pipe, boxed in the pool's owner list so its address stays put.
struct Pipe(SocketPair);

impl Drop for Pipe {
    fn drop(&mut self) {
        socket_pair::drop(&mut self.0);
    }
}

/// Create a datagram pair and retain it before publishing the worker's pipe views.
fn new_pipe(pool: Pool<'_>, worker: Record<'_>) -> Result<(), PipeError> {
    let mut built = Box::new(Pipe(unix_socket(true, libc::SOCK_DGRAM)));
    let (master, end) = built.0.raw_ends();
    if master.is_null() || end.is_null() {
        return Err(PipeError::Open);
    }
    // The box keeps the pair at this address while the list owns it.
    let pair = &raw mut built.0;
    if !pool.keep_pipe(built) {
        return Err(PipeError::Closed);
    }
    // SAFETY: the two ends of the pair the pool's list now owns until it drops,
    // in the pool's `destroy`, after which this process sends to its workers no more.
    unsafe { worker.set_pipes(pair.cast(), master.cast(), end.cast()) };
    Ok(())
}

// ---------------------------------------------------------------- timers and loop

/// The pool's `max_wait_time`, in the milliseconds a timer takes.
fn max_wait_ms(pool: Pool<'_>) -> c_long {
    c_long::from(pool.max_wait_time()) * 1000
}

/// The pool's timers that kill its workers once they outstay `max_wait_time`.
#[derive(Clone, Copy)]
enum PoolTimer {
    /// The reload task's: its workers that haven't exited yet.
    Reload,
    /// `kill_timer`: all of them.
    Kill,
}

/// Arms the one-shot timer `which` of the pool's `max_wait_time` and gives it to the
/// pool, whose reload task's drop (`Reload`), or `wait`, `destroy` or free (`Kill`),
/// deletes it unless it has fired.
fn arm(pool: Pool<'_>, which: PoolTimer) {
    let raw = pool.raw();
    let fire = move || {
        // SAFETY: the pool holds the timer and deletes it before it is freed, so the
        // pool is live whenever the timer fires.
        let pool = unsafe { PoolRef::new(raw) };
        match which {
            PoolTimer::Reload => {
                pool.with_reload_task(|task| task.kill_all(libc::SIGKILL, reload_warn));
            }
            PoolTimer::Kill => kill_all_workers(crate::process_pool_native(), pool, libc::SIGKILL),
        }
    };
    let armed = || {
        let held = swoole_rs_corebase::timer::add_fn(max_wait_ms(pool), false, fire)?;
        Some(ArmedTimer::new(
            u64::from(held.generation()),
            held.id(),
            swoole_rs_corebase::timer::cancel_issued,
        ))
    };
    match which {
        PoolTimer::Reload => pool.arm_reload_timer(armed),
        PoolTimer::Kill => pool.set_kill_timer(armed()),
    }
}

thread_local! {
    /// The pool whose worker runs the loop `Op::EventInit` made on this thread, for
    /// that loop's handlers.
    static LOOP_POOL: Cell<Option<Pool<'static>>> = const { Cell::new(None) };
    /// The index of the worker `Op::ExitCallback` names, among its pool's.
    static EXITING: Cell<Option<usize>> = const { Cell::new(None) };
}

/// `onWorkerExit`, then stop the loop once it may exit.
fn worker_exit() {
    if let Some(pool) = LOOP_POOL.get()
        && let Some(worker) = EXITING.get().and_then(|index| pool.worker(index))
    {
        pool.call_on_worker_exit(worker);
        if process::pool::bailout_pending() {
            return;
        }
    }
    // The loop running this end callback.
    let reactor = crate::objects::Reactor::thread();
    if reactor.if_exit() {
        reactor.handle().set_running(false);
    }
}

/// `ProcessPool::recv_packet`: the event loop's pipe handler under the stream protocol.
fn recv_packet(_: Running<'_>, event: &mut Event) -> c_int {
    let Some(pool) = LOOP_POOL.get() else {
        return 0;
    };
    let mut packet = pool.take_ipc_packet();
    if packet.is_empty() {
        return 0;
    }
    let size = packet.len().min(pool.max_packet_size() as usize);
    let n = swoole_rs_corebase::sys::read(
        fd(event.socket()),
        packet.get_mut(..size).unwrap_or_default(),
    );
    if n.is_none() && errno() != libc::EINTR {
        log::sys_warning(
            c"ProcessPool::recv_packet",
            format_args!("failed to read({}) pipe", event.fd()),
        );
    }
    let info = DataHead {
        reactor_id: -1,
        ..DataHead::default()
    };
    // The bytes read: none when the read failed.
    let read = packet.get(..n.unwrap_or(0)).unwrap_or_default();
    pool.call_on_message(&mut RecvData::new(info, read));
    pool.restore_ipc_packet(packet);
    0
}

/// `ProcessPool::recv_message`: the event loop's pipe handler under the message protocol.
fn recv_message(_: Running<'_>, event: &mut Event) -> c_int {
    if let Some(pool) = LOOP_POOL.get()
        && bus_read(pool, event.socket()) > Some(0)
    {
        bus_deliver(pool);
    }
    0
}

/// `ProcessPool::run_with_*_protocol` and `run_async`: the worker loop.
fn run_loop(pool: PoolRef<'_>, worker: Record<'_>, protocol: Option<Protocol>) -> CallbackResult {
    match pool_run_main(&NATIVE, pool, worker, protocol) {
        Ok(false) => Err(CallbackError::Failed),
        Ok(true) => Ok(()),
        Err(_) => {
            process::pool::defer_bailout();
            Ok(())
        }
    }
}

// ---------------------------------------------------------------- the table

/// The effects on the thread's timer and event loop: whether `op` was one.
fn loop_ops(op: Op, arg: u64) -> bool {
    match op {
        Op::PipeHandler => {
            let handler = if arg == 0 { recv_packet } else { recv_message };
            set_handlers(FD_PIPE, Some(handler), None, ConnectionDefault);
        }
        Op::TimerSelect => swoole_timer_select(),
        Op::TimerFree => {
            if swoole_timer_is_available() {
                swoole_timer_free();
            }
        }
        _ => return false,
    }
    true
}

/// `pool_call`'s lifecycle effects on the pool's objects: whether `op` was one.
fn objects(pool: Pool<'_>, op: Op, arg: u64) -> bool {
    match op {
        Op::MainLoop => {
            let main_loop: MainLoopFn = match Protocol::from_code(arg.c_cast::<c_int>()) {
                Some(Protocol::Task) => |pool, worker| run_loop(pool, worker, Some(Protocol::Task)),
                Some(Protocol::Stream) => {
                    |pool, worker| run_loop(pool, worker, Some(Protocol::Stream))
                }
                Some(Protocol::Message) => {
                    |pool, worker| run_loop(pool, worker, Some(Protocol::Message))
                }
                None => |pool, worker| run_loop(pool, worker, None),
            };
            let hooks = Hooks {
                main_loop: Some(main_loop),
                ..Hooks::default()
            };
            pool.install(&hooks);
        }
        // The global pool's `free` is a no-op.
        Op::FreeWorkers => {}
        _ => return false,
    }
    true
}

/// `pool_call`'s effects on its worker (its pipes, globals and callbacks): whether
/// `op` was one.
fn worker_ops(pool: Pool<'_>, worker: Record<'_>, op: Op, arg: u64) -> bool {
    match op {
        Op::UnlimitPipes => {
            for master in [false, true] {
                if let Some(end) = worker_pipe(worker, master) {
                    end.edit(|end| end.output.buffer_size = u32::MAX);
                }
            }
        }
        Op::WorkerInit => {
            swr_v1_worker_init(worker);
            worker.set_pid(std::process::id().c_cast::<c_int>());
            process::methods::set_worker_type(WorkerType::Event);
            swoole_set_worker_id(worker.id());
            swoole_set_worker_pid(worker.pid());
            // SAFETY: the pool's worker this process now runs, which the pool keeps
            // until the worker loop ends.
            unsafe {
                process::worker_globals::set_current_worker(worker.raw());
            };
        }
        Op::WorkerShutdown => {
            swr_v1_worker_shutdown(worker);
        }
        Op::RemovePipe => {
            // arg: only when the pipe is still in the reactor.
            if let Some(pipe) = worker_pipe(worker, false)
                && (arg == 0 || pipe.read(|pipe| !pipe.removed))
            {
                Reactor::thread().del(*pipe);
            }
        }
        Op::ExitCallback => {
            // The worker of the loop this thread runs, as one of its pool's.
            EXITING.set(pool.index_of(worker));
            swoole_rs_corebase::reactor::swr_v1_event_set_end_callback(
                swoole_rs_corebase::reactor::Priority::TryExit,
                worker_exit,
            );
        }
        Op::DontRestart => {
            if let Some(pipe) = worker_pipe(worker, false) {
                pipe.edit(|pipe| pipe.dont_restart = true);
            }
        }
        Op::WorkerError => {
            let status = exit_status(arg);
            worker_report_error(worker, &status);
            pool.call_on_worker_error(worker, &status);
        }
        _ => return false,
    }
    true
}

/// `pool_call`'s timers, messages and callbacks.
fn master_ops(pool: Pool<'_>, op: Op, arg: u64) {
    match op {
        Op::ReopenLogger => {
            swoole_rs_corebase::utilities::process_logger()
                .into_iter()
                .for_each(swoole_rs_corebase::utilities::Logger::reopen);
        }
        Op::ReloadTimer => arm(pool, PoolTimer::Reload),
        Op::TimerInit => {
            if swoole_timer_is_available() {
                swoole_rs_corebase::timer::thread_timer_reinit(true);
            } else {
                // This thread has no timer.
                swoole_timer_create(true);
            }
        }
        Op::OnWorkerNotFound => {
            pool.call_on_worker_not_found(&exit_status(arg));
        }
        Op::OnAfterReload => {
            pool.call_on_after_reload();
        }
        Op::KillTimer => arm(pool, PoolTimer::Kill),
        _ => {}
    }
}

/// The socket of a stream connection `accept` gave the pool.
fn connection_socket(connection: &dyn core::any::Any) -> Option<Socket<'_>> {
    connection
        .downcast_ref::<swoole_rs_corebase::reactor::OwnedSocket>()
        .map(swoole_rs_corebase::reactor::OwnedSocket::socket)
}

/// `socket->send_sync(data, length)` on the socket `pick` finds in `c`.
fn send_on(c: &mut PoolCall, pick: for<'c> fn(&'c PoolCall<'_>) -> Option<Socket<'c>>) {
    io(c, |c| {
        pick(c)
            .and_then(|socket| upstream::send_sync(socket, c.bytes(), 0))
            .map_or(-1, usize::cast_signed)
    });
}

/// Runs `io` and records its result and errno in the call.
fn io(c: &mut PoolCall, io: impl FnOnce(&mut PoolCall) -> isize) {
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

/// The objects and callbacks every `ProcessPool` method `corebase::process` defines runs over.
struct Native;

static NATIVE: Native = Native;

impl PoolNative for Native {
    fn alloc_workers(&self, count: u32) -> *mut WorkerRecord {
        // `count` workers, which the pool counts once they're allocated.
        let size = count as usize * size_of::<WorkerRecord>();
        mem_pool::alloc(size.c_cast::<u32>()).cast()
    }

    fn new_message_bus(&self) -> Option<Box<dyn core::any::Any>> {
        new_message_bus()
    }

    fn call(&self, c: &PoolCall, op: Op, arg: u64) {
        let pool = c.pool();
        let done = objects(pool, op, arg)
            || loop_ops(op, arg)
            || c.worker()
                .is_some_and(|worker| worker_ops(pool, worker, op, arg));
        if !done {
            master_ops(pool, op, arg);
        }
    }

    fn event_init(&self, c: &PoolCall) -> bool {
        // This thread has no event loop yet.
        if swoole_rs_corebase::reactor::swoole_event_init(EVENTLOOP_WAIT_EXIT).is_err() {
            return false;
        }
        let pool = c.pool();
        // The loop just made.
        crate::objects::Reactor::thread()
            .handle()
            .set_object(pool.raw().cast());
        // SAFETY: the pool outlives the loop its worker runs on this thread, and
        // only that loop's handlers read the handle.
        LOOP_POOL.set(Some(unsafe { PoolRef::new(pool.raw()) }));
        true
    }

    fn event_available(&self) -> bool {
        swoole_event_is_available()
    }

    fn event_wait(&self) -> bool {
        swoole_rs_corebase::reactor::event_api::swoole_event_wait().is_ok()
    }

    fn waitpid(&self, pid: c_int) -> bool {
        swoole_rs_corebase::reactor::wait_upstream::swoole_waitpid(pid, Some(&mut 0), 0) >= 0
    }

    fn new_pipe(&self, call: &PoolCall) -> Result<(), PipeError> {
        new_pipe(call.pool(), call.worker().ok_or(PipeError::MissingWorker)?)
    }

    fn send_buffer_size(&self, call: &PoolCall) -> std::io::Result<c_int> {
        let mut size = 0;
        let Some(worker) = call.worker() else {
            return Ok(0);
        };
        let socket = worker_pipe(worker, true);
        swoole_rs_corebase::sys::getsockopt(
            fd(socket),
            libc::SOL_SOCKET,
            libc::SO_SNDBUF,
            &mut size,
        )?;
        Ok(size)
    }

    fn bus_buffer(&self, call: &PoolCall, size: usize) -> Result<(), PipeError> {
        with_bus(call.pool(), |bus| bus.alloc_buffer(size))
            .unwrap_or(false)
            .then_some(())
            .ok_or(PipeError::Buffer)
    }

    fn add_pipe(&self, call: &PoolCall) -> Result<(), PipeError> {
        let Some(worker) = call.worker() else {
            return Ok(());
        };
        worker_pipe(worker, false)
            .is_some_and(|pipe| Reactor::thread().add(pipe, Events::READ))
            .then_some(())
            .ok_or(PipeError::Register)
    }

    fn push_stop(&self, call: &PoolCall) -> Result<(), PipeError> {
        let Some(worker) = call.worker() else {
            return Ok(());
        };
        push_stop(self, call.pool(), worker.pid(), worker.id().c_cast::<u16>())
            .then_some(())
            .ok_or(PipeError::StopMessage)
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
                    c.set_result(code.map_or(-1, |()| 0));
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
        process::signal::swoole_signal_dispatch();
    }

    fn accept(&self, c: &mut PoolCall) {
        let accepted = crate::view::pool_ipc_listener(c.pool()).and_then(|listener| {
            listener
                .socket()
                .edit(swoole_rs_corebase::reactor::SocketFields::accepted)
        });
        c.set_error(errno());
        if let Some(record) = accepted {
            c.set_connection(Rc::new(swoole_rs_corebase::reactor::OwnedSocket::new(
                record,
            )));
        }
    }

    fn recv(&self, c: &mut PoolCall) {
        io(c, |c| {
            let flags = c.flags();
            let (connection, buffer) = c.connection_buffer();
            connection
                .and_then(connection_socket)
                .and_then(|socket| upstream::recv_sync(socket, buffer, flags))
                .map_or(-1, usize::cast_signed)
        });
    }

    fn send(&self, c: &mut PoolCall) {
        send_on(c, |c| c.connection().and_then(connection_socket));
    }

    fn close(&self, c: &mut PoolCall) {
        // The pool's only hold on the connection or listener.
        let socket = c
            .take_connection()
            .and_then(|socket| {
                socket
                    .downcast::<swoole_rs_corebase::reactor::OwnedSocket>()
                    .ok()
            })
            .and_then(Rc::into_inner);
        socket_free(socket);
    }

    fn pipe_read(&self, c: &mut PoolCall) {
        io(c, |c| {
            let Some(pipe) = c.worker().and_then(|worker| worker_pipe(worker, false)) else {
                return -1;
            };
            let next = swoole_timer_get_next_msec();
            let timeout = next.c_cast::<f64>() / 1000.0;
            pipe.edit(|pipe| {
                pipe.timeouts
                    .set(timeout, swoole_rs_corebase::reactor::TimeoutKind::READ);
            });
            upstream::read_sync(*pipe, c.connection_buffer().1).map_or(-1, usize::cast_signed)
        });
    }

    fn pipe_wait(&self, c: &mut PoolCall) {
        io(c, |c| {
            let next = swoole_timer_get_next_msec();
            let ready = c
                .worker()
                .and_then(|worker| worker_pipe(worker, false))
                .is_some_and(|pipe| pipe.wait_event(next.c_cast::<c_int>(), Events::READ));
            if ready { 0 } else { -1 }
        });
    }

    fn bus_read(&self, c: &mut PoolCall) {
        io(c, |c| {
            c.worker()
                .and_then(|worker| bus_read(c.pool(), worker_pipe(worker, false)))
                .map_or(-1, usize::cast_signed)
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
            .and_then(|pipe| with_bus(pool, |bus| bus.write(pipe, head, message)))
            .unwrap_or(false)
    }

    fn task(&self, c: &mut PoolCall) {
        let (pool, worker) = (c.pool(), c.worker());
        // The task the worker read into the call's buffer: its head and the `len`
        // bytes after it, which the loop checked fit.
        if let Some((worker, task)) = worker.zip(Packed::from_bytes(c.bytes()))
            && let Some(code) = pool.call_on_task(worker, task)
        {
            c.set_result(code.map_or(-1, |()| 0));
        }
    }

    fn message(&self, c: &mut PoolCall) {
        let info = DataHead {
            reactor_id: -1,
            ..DataHead::default()
        };
        c.pool()
            .call_on_message(&mut RecvData::new(info, c.bytes()));
    }

    fn shutdown(&self, c: &PoolCall) {
        if let Some(worker) = c.worker() {
            swr_v1_worker_shutdown(worker);
        }
    }

    fn pipe_write(&self, c: &mut PoolCall) {
        if c.flags() != 0 && swoole_event_is_available() {
            io(c, |c| {
                // The worker's pipe, in this thread's loop.
                c.lent_socket()
                    .map(Heap::lent)
                    .and_then(|pipe| event_write(pipe, c.bytes()))
                    .map_or(-1, usize::cast_signed)
            });
        } else {
            // The worker's pipe the call lends.
            send_on(c, |c| c.lent_socket().map(|pipe| *Heap::lent(pipe)));
        }
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
                let listener = crate::view::pool_ipc_listener(pool);
                let fd = fd(listener
                    .as_deref()
                    .map(swoole_rs_corebase::reactor::OwnedSocket::heap));
                log::sys_warning(origin, format_args!("[Worker#{id}] accept({fd}) failed"));
            }
            Warning::Read => {
                let fd = fd(worker.and_then(|worker| worker_pipe(worker, false)));
                log::sys_warning(origin, format_args!("[Worker#{id}] read({fd}) failed"));
            }
            Warning::BadPacket { received, length } => log::error_log(
                swoole_rs_corebase::log::LogLevel::SW_LOG_WARNING,
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

/// The server's process-pool effects, passed to the methods in `corebase::process`.
#[must_use]
pub fn process_pool_native() -> &'static dyn PoolNative {
    &NATIVE
}
