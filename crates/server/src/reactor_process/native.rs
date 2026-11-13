//! The base-mode reactor process's native objects: its event worker pool and the
//! reads from the server's message bus.
use core::ffi::c_int;
use swoole_rs_process::methods::ExitStatus;
use swoole_rs_process::process_pool::{Hooks, IpcMode, PoolRef};
use swoole_rs_process::task_file::Packed;
use swoole_rs_process::worker_record::{Worker, WorkerType};
use swoole_rs_reactor::{Event, Running};

use crate::base_factory::SendData;
use crate::callbacks::TaskSlot;
use crate::objects::Bus;
use crate::reactor_handle::{EVENT_READ, Reactor};
use crate::view;

/// `SW_FD_PIPE`, `Server::CLOSE_ACTIVELY` and the `ServerEventType`s a reactor
/// process reads from its pipe.
const FD_PIPE: c_int = 3;
const CLOSE_ACTIVELY: c_int = 1 << 2;
const EVENT_RECV_DATA: u8 = 0;
const EVENT_SEND_DATA: u8 = 2;
const EVENT_SEND_FILE: u8 = 3;
const EVENT_CLOSE: u8 = 4;
const EVENT_CLOSE_FORWARD: u8 = 7;
const EVENT_FINISH: u8 = 9;
const EVENT_PIPE_MESSAGE: u8 = 10;
const EVENT_SHUTDOWN: u8 = 16;
const EVENT_COMMAND_REQUEST: u8 = 17;
const EVENT_COMMAND_RESPONSE: u8 = 18;

/// The event worker pool over the server's workers.
pub(crate) fn create_pool(serv: view::Server<'_>) -> c_int {
    let config = serv.config();
    let workers = serv.workers();
    let pool = workers.event_pool.raw();
    // SAFETY: the server's event pool, reset to zero as `*pool = {}` did.
    unsafe { pool.write_bytes(0, 1) };
    let pool_ref = serv.event_pool_ref();
    let ipc = IpcMode::UnixSock.code().into();
    if swoole_rs_process::methods::create(pool_ref, config.worker_num.get(), 0, ipc) < 0 {
        return -1;
    }
    swoole_rs_process::methods::set_max_request(
        pool_ref,
        config.max_request.get(),
        config.max_request_grace.get(),
    );
    let hooks = Hooks {
        ptr: Some(serv.raw()),
        main_loop: Some(main_loop),
        on_worker_not_found: Some(worker_not_found),
        ..Hooks::default()
    };
    // SAFETY: both hooks read the pool's `ptr` as the server that owns the pool, which
    // is what this sets it to; the server outlives its event pool.
    unsafe { pool_ref.install(&hooks) };
    // SAFETY: as above.
    let record = unsafe { &mut *pool };
    record.max_wait_time = config.max_wait_time.get();
    record.use_msgqueue = false;
    let count = config.worker_num.get() as usize;
    // SAFETY: the pool's and the server's worker arrays, `count` records each.
    unsafe { core::ptr::copy_nonoverlapping(record.workers(), workers.event_workers, count) };
    // SAFETY: the server's `count` (`worker_num`) event worker records, which live
    // as long as the server and so its event pool.
    unsafe { record.set_workers(workers.event_workers) };
    // `create` set the pool's `worker_num` to `count`, so these are the server's records.
    for (id, index) in (0u32..).zip(0..count) {
        if let Some(worker) = record.worker(index) {
            worker.set_pool(pool.cast());
            worker.set_id(id);
            worker.set_kind(WorkerType::Event.code());
        }
    }
    0
}

/// `pool->main_loop`: `Server::reactor_process_main_loop` over the pool's worker.
extern "C-unwind" fn main_loop(pool: PoolRef<'_>, worker: Worker<'_>) -> c_int {
    // SAFETY: a reactor process pool's `ptr` is its live server.
    let serv = unsafe { view::Server::from_raw(pool.ptr()) };
    super::main_loop(serv, worker)
}

/// `pool->onWorkerNotFound`: restart the event worker that exited. The pool lends
/// itself and the exit status for the call.
extern "C-unwind" fn worker_not_found(pool: PoolRef<'_>, status: &ExitStatus) -> c_int {
    // SAFETY: a reactor process pool's `ptr` is its live server, as `main_loop`'s.
    let serv = unsafe { view::Server::from_raw(pool.ptr()) };
    crate::manager::native::restart_worker_process(serv, status)
}

/// This thread's reactor reads its pipes with [`on_pipe_read`].
pub(crate) fn set_pipe_handler() {
    Reactor::thread().set_handler(FD_PIPE, EVENT_READ, on_pipe_read);
}

/// `ReactorProcess_onPipeRead`: a message from another worker or a task worker.
extern "C" fn on_pipe_read(reactor: Running<'_>, event: &mut Event) -> c_int {
    let reactor = Reactor::running(reactor);
    super::with_server(reactor, |serv| {
        let bus = serv.state().message_bus();
        if event.socket().is_some_and(|socket| bus.read(socket) > 0) && dispatch(serv, reactor, bus)
        {
            bus.pop();
        }
        0
    })
}

/// The message the bus just read, by its kind; false, leaving it on the bus, when
/// data comes from no known worker.
fn dispatch(serv: view::Server<'_>, reactor: Reactor, bus: Bus) -> bool {
    let state = serv.state();
    let info = bus.head();
    match info.kind {
        EVENT_PIPE_MESSAGE => state.callbacks.pipe_message(serv, event_data(serv)),
        EVENT_FINISH => {
            _ = state
                .callbacks
                .task(TaskSlot::Finish, serv, event_data(serv));
        }
        EVENT_SHUTDOWN => {
            if let Some(worker) = serv.worker_record(reactor.handle().id()) {
                crate::event_worker::native::stop_async_worker(serv, worker);
            }
        }
        EVENT_SEND_FILE => {
            // The bus's `PipeBuffer`: the message's head, then its data.
            let data = bus.buffer().wrapping_add(1).cast();
            finish(state, SendData { info, data });
        }
        EVENT_SEND_DATA => {
            let workers = serv.workers();
            let all = u64::from(workers.event_count)
                + u64::from(workers.task_count)
                + workers.user_count as u64;
            let worker = info.reactor_id;
            if u64::try_from(worker).is_ok_and(|id| id < all) {
                let packet = bus.packet();
                let mut info = info;
                info.kind = EVENT_RECV_DATA;
                info.len = u32::try_from(packet.length).unwrap_or(u32::MAX);
                finish(
                    state,
                    SendData {
                        info,
                        data: packet.data,
                    },
                );
            } else {
                let m = format_args!("invalid worker_id={worker}");
                swoole_rs_corebase::log::warning(c"ReactorProcess_onPipeRead", m);
                return false;
            }
        }
        EVENT_CLOSE | EVENT_CLOSE_FORWARD => {
            _ = crate::factory::end(state, info.fd, CLOSE_ACTIVELY);
        }
        EVENT_COMMAND_REQUEST => {
            // The worker this process runs.
            let me = swoole_rs_process::worker_globals::with_current(|me| me.id());
            let pipe = serv.worker_pipe(0, true);
            if let Some(me) = me {
                let id = u16::try_from(me).unwrap_or(0);
                crate::commands::call_command_handler(serv, bus, id, pipe);
            }
        }
        EVENT_COMMAND_RESPONSE => {
            bus.with_packet(|reply| crate::commands::command_result(state, info.fd, reply));
        }
        _ => {}
    }
    true
}

/// The factory finishes `send`.
fn finish(state: &crate::ServerState, mut send: SendData) {
    crate::factory::finish(state, (&raw mut send).cast());
}

/// The event `serv`'s bus just read, as its sender packed it: an `EventData`, its
/// head and the payload the head counts.
fn event_data(serv: view::Server<'_>) -> Packed<'_> {
    let buffer = serv.state().message_bus().buffer();
    // SAFETY: called once the bus has read a whole event into its buffer, which only
    // this thread's next read replaces, after the callback it is lent to returns.
    unsafe { Packed::from_raw(buffer.cast()) }
}
