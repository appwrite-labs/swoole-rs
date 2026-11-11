//! The task worker's ops on native objects (its pool's callbacks, its pipe in the
//! reactor and the commands sent to it), and the callbacks that run its operations:
//! what workers.cc's `TaskWorkerContext::call` and `TaskWorker_*` did.
use core::ffi::{c_int, c_void};
use core::marker::PhantomData;
use core::mem::MaybeUninit;

use swoole_rs_corebase::DataHead;
use swoole_rs_process::methods::swoole_get_worker_id;
use swoole_rs_process::worker_record::Worker;
use swoole_rs_process::workers::entry::Bytes;
use swoole_rs_process::workers::ops::{
    OP_COMMAND_REPLY, OP_HAS_COMMAND, OP_LISTEN_TASK_PIPE, OP_POOL_INIT, OP_SET_ASYNC_LOOP,
    OP_UNLIMIT_BUS_PIPES, OP_UNPACK_COMMAND, RUN_FINISH, RUN_TASK_LOOP_ASYNC, RUN_TASK_ON_PIPE,
    RUN_TASK_ON_START, RUN_TASK_ON_STOP, RUN_TASK_ON_TASK,
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
                pool.on_task = on_task as *const c_void;
                pool.on_worker_start = on_start as *const c_void;
                pool.on_worker_stop = on_stop as *const c_void;
            });
            0
        }
        OP_SET_ASYNC_LOOP => {
            ctx.pool()
                .edit(|pool| pool.main_loop = loop_async as *const c_void);
            0
        }
        OP_HAS_COMMAND => {
            let id = i32::from(ctx.task().server_fd);
            let handler = crate::commands::handler(ctx.state(), id);
            if let Some(handler) = handler {
                ctx.handler = handler;
            }
            u64::from(handler.is_some())
        }
        OP_UNPACK_COMMAND => {
            let buffer = swoole_rs_corebase::thread_buffer();
            let unpack = swoole_rs_process::task_file::unpack;
            // SAFETY: the live command task and this thread's buffer.
            let Some((data, length, _)) = (unsafe { unpack(ctx.task.cast_mut().cast(), buffer) })
            else {
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
            // SAFETY: the handler OP_HAS_COMMAND found and the message unpacked for it.
            u64::from(unsafe {
                crate::commands::answer(
                    ctx.handler,
                    ctx.serv(),
                    (message, length),
                    (bus, sock),
                    fd,
                    id,
                )
            })
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
extern "C-unwind" fn on_task(pool: TaskPool<'_>, worker: Worker<'_>, task: &mut DataHead) -> c_int {
    run_task(pool, worker, task)
}

/// Runs `task`, null or live while the call runs, in `pool`'s `worker`.
pub(super) fn run_task(pool: TaskPool<'_>, worker: Worker<'_>, task: *const DataHead) -> c_int {
    let mut ctx = Context::of(pool, worker);
    ctx.task = task;
    run(&mut ctx, RUN_TASK_ON_TASK)
}

/// `TaskWorker_onStart`.
extern "C-unwind" fn on_start(pool: TaskPool<'_>, worker: Worker<'_>) {
    run(&mut Context::of(pool, worker), RUN_TASK_ON_START);
}

/// `TaskWorker_onStop`.
extern "C-unwind" fn on_stop(pool: TaskPool<'_>, worker: Worker<'_>) {
    run(&mut Context::of(pool, worker), RUN_TASK_ON_STOP);
}

/// `TaskWorker_loop_async`: the coroutine task worker's main loop.
extern "C-unwind" fn loop_async(pool: TaskPool<'_>, worker: Worker<'_>) -> c_int {
    run(&mut Context::of(pool, worker), RUN_TASK_LOOP_ASYNC)
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

/// `Server::init_task_workers` over the server's task pool, and `Server::finish`
/// (crates/process's entry.rs) of `task`'s result `bytes`.
///
/// # Safety
/// `serv` is a live `Server`; for `RUN_FINISH`, `task` is null or a live
/// `EventData` and `bytes` the method's live result.
pub(crate) unsafe fn enter(
    operation: u32,
    serv: *mut c_void,
    task: *mut c_void,
    bytes: *mut c_void,
    arg: u64,
) -> i64 {
    // SAFETY: per the caller, `serv` is a live `Server` for the call.
    let serv = unsafe { crate::view::Server::from_raw(serv) };
    let mut buf = MaybeUninit::<EventData>::uninit();
    let mut ctx = Context::over(serv);
    if operation == RUN_FINISH {
        // SAFETY: as the caller guarantees.
        let bytes = unsafe { &*bytes.cast::<Bytes>() };
        ctx.task = task.cast();
        ctx.buf = buf.as_mut_ptr().cast();
        (ctx.data, ctx.data_len) = (bytes.data.cast(), bytes.length);
    } else {
        ctx.pool = Some(TaskPool::of(serv));
    }
    run_with(&mut ctx, operation, arg)
}
