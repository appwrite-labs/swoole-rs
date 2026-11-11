//! The task worker's ops on native objects (its pool's callbacks, its pipe in the
//! reactor and the commands sent to it), and the callbacks that run its operations:
//! what workers.cc's `TaskWorkerContext::call` and `TaskWorker_*` did.
use core::ffi::c_int;
use core::marker::PhantomData;
use core::mem::MaybeUninit;

use swoole_rs_buffer::string::CppString;
use swoole_rs_corebase::DataHead;
use swoole_rs_process::methods::swoole_get_worker_id;
use swoole_rs_process::process_pool::PoolRef;
use swoole_rs_process::task_file::Packed;
use swoole_rs_process::worker_record::Worker;
use swoole_rs_process::workers::ops::{
    OP_COMMAND_REPLY, OP_HAS_COMMAND, OP_LISTEN_TASK_PIPE, OP_POOL_INIT, OP_SET_ASYNC_LOOP,
    OP_UNLIMIT_BUS_PIPES, OP_UNPACK_COMMAND, RUN_FINISH, RUN_INIT_TASK_WORKERS,
    RUN_TASK_LOOP_ASYNC, RUN_TASK_ON_PIPE, RUN_TASK_ON_START, RUN_TASK_ON_STOP, RUN_TASK_ON_TASK,
};

use super::{Context, TaskPool};
use crate::objects::{EVENT_READ, Reactor, in_worker_bus, set_nonblock, socket};

/// `SW_FD_PIPE`.
const SW_FD_PIPE: c_int = 3;

/// The ops on native objects.
#[inline(always)]
pub(super) fn native(ctx: &mut Context, op: u32, arg: u64) -> Option<u64> {
    Some(match op {
        OP_POOL_INIT => {
            let serv = ctx.serv();
            ctx.pool().edit(|pool| {
                pool.ptr = serv;
                pool.on_task = Some(on_task);
                pool.on_worker_start = Some(on_start);
                pool.on_worker_stop = Some(on_stop);
            });
            0
        }
        OP_SET_ASYNC_LOOP => {
            ctx.pool().edit(|pool| pool.main_loop = Some(loop_async));
            0
        }
        OP_HAS_COMMAND => {
            let id = i32::from(ctx.task().server_fd);
            let handler = crate::commands::handler(ctx.state(), id);
            let found = handler.is_some();
            if found {
                ctx.handler = handler;
            }
            u64::from(found)
        }
        OP_UNPACK_COMMAND => {
            let buffer = swoole_rs_corebase::thread_buffer().cast::<CppString>();
            // SAFETY: the live command task.
            let task = unsafe { Packed::from_raw(ctx.task.cast()) };
            // SAFETY: this thread's live buffer.
            let Some((data, length, _)) = task.unpack(unsafe { &mut *buffer }) else {
                return Some(0);
            };
            ctx.packet = (length, data);
            1
        }
        OP_COMMAND_REPLY => {
            let (length, message) = ctx.packet;
            let state = ctx.state();
            let sock = crate::commands::reply_socket(state, ctx.server)?;
            let bus = crate::master_ops::server_message_bus(state);
            // `serv->get_task_id(task)`: the task's fd.
            let (fd, id) = (ctx.task().fd, ctx.worker().id());
            let id = u16::try_from(id).unwrap_or(u16::MAX);
            let handler = ctx.handler.as_ref()?;
            let message = if message.is_null() {
                &[][..]
            } else {
                // SAFETY: the message OP_UNPACK_COMMAND unpacked, `length` bytes.
                unsafe { core::slice::from_raw_parts(message.cast(), length) }
            };
            let reply = (bus, sock);
            u64::from(crate::commands::answer(
                handler, ctx.server, message, reply, fd, id,
            ))
        }
        OP_LISTEN_TASK_PIPE => {
            let pipe = in_worker_bus(ctx.state(), ctx.config(), ctx.worker().pipe_worker());
            let pipe = socket(pipe)?;
            set_nonblock(pipe);
            let reactor = Reactor::thread();
            reactor.fields().ptr = ctx.pool().raw.cast();
            reactor.add(pipe, EVENT_READ);
            reactor.set_handler(SW_FD_PIPE, EVENT_READ, on_pipe_receive);
            0
        }
        OP_UNLIMIT_BUS_PIPES => {
            let id = u16::try_from(arg).unwrap_or(u16::MAX);
            let server = ctx.server;
            for pipe in [server.worker_pipe(id, false), server.worker_pipe(id, true)] {
                let pipe = pipe.map_or(core::ptr::null_mut(), |pipe| pipe.as_ptr().cast());
                if let Some(pipe) = socket(in_worker_bus(ctx.state(), ctx.config(), pipe)) {
                    pipe.edit(|pipe| pipe.output.buffer_size = u32::MAX);
                }
            }
            0
        }
        _ => return None,
    })
}

/// `TaskWorker_onTask`: the pool's task callback.
extern "C-unwind" fn on_task(pool: PoolRef<'_>, worker: Worker<'_>, task: &mut DataHead) -> c_int {
    run_task(task_pool(pool), worker, task)
}

/// The task pool its callbacks are passed.
fn task_pool(pool: PoolRef<'_>) -> TaskPool<'_> {
    TaskPool {
        raw: pool.raw(),
        live: PhantomData,
    }
}

/// Runs `task`, null or live while the call runs, in `pool`'s `worker`.
pub(super) fn run_task(pool: TaskPool<'_>, worker: Worker<'_>, task: *const DataHead) -> c_int {
    let mut ctx = Context::of(pool, worker);
    ctx.task = task;
    run(&mut ctx, RUN_TASK_ON_TASK)
}

/// `TaskWorker_onStart`.
extern "C-unwind" fn on_start(pool: PoolRef<'_>, worker: Worker<'_>) {
    run(&mut Context::of(task_pool(pool), worker), RUN_TASK_ON_START);
}

/// `TaskWorker_onStop`.
extern "C-unwind" fn on_stop(pool: PoolRef<'_>, worker: Worker<'_>) {
    run(&mut Context::of(task_pool(pool), worker), RUN_TASK_ON_STOP);
}

/// `TaskWorker_loop_async`: the coroutine task worker's main loop.
extern "C-unwind" fn loop_async(pool: PoolRef<'_>, worker: Worker<'_>) -> c_int {
    run(
        &mut Context::of(task_pool(pool), worker),
        RUN_TASK_LOOP_ASYNC,
    )
}

/// `swoole::EventData`: `SW_IPC_MAX_SIZE` bytes, aligned for its head. Left
/// uninitialized as upstream's are: a read or `task_pack` writes the head and
/// the payload its `len` counts, and nothing reads past them.
#[repr(C, align(8))]
struct EventData([u8; super::EVENT_DATA_SIZE]);

/// `TaskWorker_onPipeReceive`: a task from a worker, in the coroutine loop.
extern "C" fn on_pipe_receive(
    reactor: swoole_rs_reactor::Running<'_>,
    event: &mut swoole_rs_reactor::Event,
) -> c_int {
    // The reactor running this handler, whose `ptr` is the task pool.
    let pool = TaskPool {
        raw: Reactor::running(reactor).fields().ptr.cast(),
        live: PhantomData,
    };
    let id = u16::try_from(swoole_get_worker_id()).unwrap_or(u16::MAX);
    let Some(worker) = pool.server().worker_record(id) else {
        return -1;
    };
    let mut task = MaybeUninit::<EventData>::uninit();
    let mut ctx = Context::of(pool, worker);
    ctx.socket = event.socket_ptr().cast();
    ctx.buf = task.as_mut_ptr().cast();
    run(&mut ctx, RUN_TASK_ON_PIPE)
}

/// Runs `operation` over `ctx`, whose objects are live.
fn run(ctx: &mut Context, operation: u32) -> c_int {
    c_int::try_from(run_with(ctx, operation, 0)).unwrap_or(-1)
}

/// Runs `operation` with `arg` over `ctx`, whose objects are live.
fn run_with(ctx: &mut Context, operation: u32, arg: u64) -> i64 {
    swoole_rs_process::workers::run_task_worker(ctx, operation, arg)
}

/// `Server::init_task_workers()` over the server's task pool.
#[must_use]
pub(crate) fn init_task_workers(serv: crate::view::Server<'_>) -> bool {
    let mut ctx = Context::over(serv);
    ctx.pool = Some(TaskPool::of(serv));
    run_with(&mut ctx, RUN_INIT_TASK_WORKERS, 0) != 0
}

/// `Server::finish(data, data_len, flags, current_task)`: the task's result to the
/// worker that sent it. With no `current_task`, the task this worker runs; false
/// when there is none.
#[must_use]
pub fn finish(
    serv: crate::view::Server<'_>,
    data: &[u8],
    flags: c_int,
    current_task: Option<&DataHead>,
) -> bool {
    let mut buf = MaybeUninit::<EventData>::uninit();
    let latest = super::LATEST_TASK.get();
    let mut ctx = Context::over(serv);
    ctx.task = current_task
        .or(latest.as_ref())
        .map_or(core::ptr::null(), core::ptr::from_ref);
    ctx.buf = buf.as_mut_ptr().cast();
    (ctx.data, ctx.data_len) = (data.as_ptr(), data.len());
    run_with(&mut ctx, RUN_FINISH, u64::from(flags.cast_unsigned())) != 0
}
