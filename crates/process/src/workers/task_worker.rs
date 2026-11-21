//! Task workers (upstream's `src/server/task_worker.cc`; workers.cc's
//! `TaskWorkerContext`): the pool's setup and callbacks,
//! the task switch, the coroutine loop and `Server::finish`, which hands a result
//! back to the worker that sent the task. The context holds the server, the pool,
//! the worker and, per operation, the task or the result being finished.
use super::event_worker::signal_init;
use super::log::{
    LOG_COROUTINE_MSGQUEUE, LOG_FINISH_IN_PIPE, LOG_FINISH_NOREPLY, LOG_LARGE_PACK,
    LOG_NO_TASK_WORKERS, LOG_SEND_RESULT,
};
use super::ops::{
    EVENT_COMMAND_REQUEST, EVENT_PIPE_MESSAGE, EVENT_SHUTDOWN, LOG_INVALID_SOURCE, LOG_READ_TASK,
    LOG_RESULT_WRITE, LOG_UNKNOWN_COMMAND, OP_ADD_REQUEST_COUNT, OP_ALL_CORE_WORKER_NUM,
    OP_COMMAND_REPLY, OP_CREATE_TASK_WORKER, OP_ENABLE_SIGNALFD, OP_EVENT_AVAILABLE, OP_EVENT_FREE,
    OP_EVENT_INIT, OP_EVENT_WAIT, OP_EXCEEDED_MAX_REQUEST, OP_FAIL_REACTOR, OP_FINISH_COUNT,
    OP_FINISH_HEADER, OP_HAS_COMMAND, OP_INFO_TYPE, OP_LISTEN_TASK_PIPE, OP_LOCK_WORKER,
    OP_NOTIFY_RESULT, OP_NOTIFY_WOULD_BLOCK, OP_ON_PIPE_MESSAGE, OP_ON_TASK, OP_ON_TASK_POOL,
    OP_OPEN_RESULT_FILE, OP_PACK_RESULT, OP_POOL_INIT, OP_READ_TASK, OP_RESULT_IN_SHM,
    OP_SELECT_TASK, OP_SEND_PIPE, OP_SEND_STREAM, OP_SEND_TIMED_OUT, OP_SET_ASYNC_LOOP,
    OP_SET_LATEST_TASK, OP_SET_SYSVMSG, OP_SOURCE_WORKER, OP_STOP_ASYNC, OP_STREAM_CONNECTION,
    OP_TASK_ENABLE_COROUTINE, OP_TASK_EXT_FLAGS, OP_TASK_IPC_MODE, OP_TASK_WORKER_NUM,
    OP_UNLIMIT_BUS_PIPES, OP_UNLOCK_WORKER, OP_UNPACK_COMMAND, OP_WAIT_NOTIFY_WRITABLE,
    OP_WORKER_ENTER, OP_WORKER_INIT, OP_WORKER_SHUTDOWN, OP_WORKER_START_CALLBACK,
    OP_WORKER_STATUS, OP_WORKER_STOP_CALLBACK, OP_WRITE_RESULT_FILE, OP_ZERO_RESULT, RUN_FINISH,
    RUN_INIT_TASK_WORKERS, RUN_TASK_COMMAND, RUN_TASK_LOOP_ASYNC, RUN_TASK_ON_PIPE,
    RUN_TASK_ON_START, RUN_TASK_ON_STOP, RUN_TASK_ON_TASK, STATUS_BUSY, STATUS_IDLE,
};
use super::{HANDLER_NONE, Host, SW_ERR, SW_OK};

/// `swoole::TaskFlag` bits.
pub(crate) const TASK_NONBLOCK: u64 = 1 << 2;
pub(crate) const TASK_CALLBACK: u64 = 1 << 3;
pub(crate) const TASK_WAITALL: u64 = 1 << 4;
pub(crate) const TASK_COROUTINE: u64 = 1 << 5;
pub(crate) const TASK_NOREPLY: u64 = 1 << 7;

/// `Server::TaskIpcMode`.
const TASK_IPC_MSGQUEUE: u64 = 2;
const TASK_IPC_PREEMPTIVE: u64 = 3;

pub(super) fn run<H: Host>(h: &mut H, operation: u32, arg: u64) -> i64 {
    match operation {
        RUN_INIT_TASK_WORKERS => i64::from(init(h)),
        RUN_TASK_COMMAND => command(h),
        RUN_TASK_ON_TASK => on_task(h),
        RUN_TASK_ON_START => {
            on_start(h);
            SW_OK
        }
        RUN_TASK_ON_STOP => {
            h.get(OP_EVENT_FREE);
            h.get(OP_WORKER_STOP_CALLBACK);
            SW_OK
        }
        RUN_TASK_ON_PIPE => on_pipe(h),
        RUN_TASK_LOOP_ASYNC => loop_async(h),
        RUN_FINISH => i64::from(finish(h, arg)),
        _ => SW_ERR,
    }
}

/// `Server::init_task_workers`, after the pool is created and before it starts.
fn init<H: Host>(h: &mut H) -> bool {
    h.get(OP_POOL_INIT);
    let mode = h.get(OP_TASK_IPC_MODE);
    // Make the task worker support asynchronous.
    if h.is(OP_TASK_ENABLE_COROUTINE) {
        if mode == TASK_IPC_MSGQUEUE || mode == TASK_IPC_PREEMPTIVE {
            h.log(LOG_COROUTINE_MSGQUEUE, 0, 0);
            return false;
        }
        h.get(OP_SET_ASYNC_LOOP);
    }
    if mode == TASK_IPC_PREEMPTIVE {
        h.get(OP_SET_SYSVMSG);
    }
    for worker in 0..h.get(OP_TASK_WORKER_NUM) {
        h.op(OP_CREATE_TASK_WORKER, worker);
    }
    true
}

/// `TaskWorker_call_command_handler`.
fn command<H: Host>(h: &mut H) -> i64 {
    if !h.is(OP_HAS_COMMAND) {
        h.get(LOG_UNKNOWN_COMMAND);
        return SW_OK;
    }
    if !h.is(OP_UNPACK_COMMAND) {
        return SW_OK;
    }
    if h.is(OP_COMMAND_REPLY) {
        SW_OK
    } else {
        SW_ERR
    }
}

/// `TaskWorker_onTask`: one task, pipe message, shutdown or command.
fn on_task<H: Host>(h: &mut H) -> i64 {
    h.get(OP_SET_LATEST_TASK);
    h.op(OP_WORKER_STATUS, STATUS_BUSY);
    let ret = match u32::try_from(h.get(OP_INFO_TYPE)).unwrap_or(u32::MAX) {
        EVENT_PIPE_MESSAGE => {
            h.get(OP_ON_PIPE_MESSAGE);
            SW_OK
        }
        EVENT_SHUTDOWN => {
            h.get(OP_WORKER_SHUTDOWN);
            if h.is(OP_EVENT_AVAILABLE) {
                h.get(OP_STOP_ASYNC);
            }
            return SW_OK;
        }
        EVENT_COMMAND_REQUEST => command(h),
        _ => {
            let ret = h.signed(OP_ON_TASK);
            // Only server tasks count as requests, not pipe messages or commands.
            h.get(OP_ADD_REQUEST_COUNT);
            ret
        }
    };
    h.op(OP_WORKER_STATUS, STATUS_IDLE);
    ret
}

/// `TaskWorker_onStart`.
fn on_start<H: Host>(h: &mut H) {
    h.get(OP_WORKER_ENTER);
    // Make the task worker support asynchronous.
    if h.is(OP_TASK_ENABLE_COROUTINE) {
        if h.signed(OP_EVENT_INIT) < 0 {
            h.get(OP_FAIL_REACTOR);
            return;
        }
        h.op(OP_ENABLE_SIGNALFD, 1);
    } else {
        h.op(OP_ENABLE_SIGNALFD, 0);
    }
    signal_init(h, HANDLER_NONE);
    h.get(OP_WORKER_START_CALLBACK);
    h.get(OP_WORKER_INIT);
}

/// `TaskWorker_onPipeReceive`: a task from a worker, in the coroutine loop.
fn on_pipe<H: Host>(h: &mut H) -> i64 {
    if h.signed(OP_READ_TASK) <= 0 {
        h.get(LOG_READ_TASK);
        return SW_ERR;
    }
    let ret = h.signed(OP_ON_TASK_POOL);
    // The maximum number of requests: the worker exits.
    if h.is(OP_EXCEEDED_MAX_REQUEST) {
        h.get(OP_STOP_ASYNC);
    }
    ret
}

/// `TaskWorker_loop_async`: the coroutine task worker's event loop.
fn loop_async<H: Host>(h: &mut H) -> i64 {
    h.op(OP_WORKER_STATUS, STATUS_IDLE);
    h.get(OP_LISTEN_TASK_PIPE);
    for worker in 0..h.get(OP_ALL_CORE_WORKER_NUM) {
        h.op(OP_UNLIMIT_BUS_PIPES, worker);
    }
    h.signed(OP_EVENT_WAIT)
}

/// `Server::finish`: send the task's result to the worker that dispatched it,
/// through its pipe (non-blocking tasks) or the shared result slot and its
/// notify pipe (blocking ones). `flags` are added to the result's flags.
fn finish<H: Host>(h: &mut H, flags: u64) -> bool {
    if h.get(OP_TASK_WORKER_NUM) < 1 {
        h.log(LOG_NO_TASK_WORKERS, 0, 0);
        return false;
    }
    if !h.is(OP_SELECT_TASK) {
        return false;
    }
    if u32::try_from(h.get(OP_INFO_TYPE)).ok() == Some(EVENT_PIPE_MESSAGE) {
        h.log(LOG_FINISH_IN_PIPE, 0, 0);
        return false;
    }
    let task_flags = h.get(OP_TASK_EXT_FLAGS);
    if task_flags & TASK_NOREPLY != 0 {
        h.log(LOG_FINISH_NOREPLY, 0, 0);
        return false;
    }
    if !h.is(OP_SOURCE_WORKER) {
        h.get(LOG_INVALID_SOURCE);
        return false;
    }
    let retval = if task_flags & TASK_NONBLOCK != 0 {
        if !h.is(OP_PACK_RESULT) {
            h.log(LOG_LARGE_PACK, 0, 0);
            return false;
        }
        // The callback flag wins over the coroutine flag.
        let flags = flags
            | if task_flags & TASK_CALLBACK != 0 {
                TASK_CALLBACK
            } else {
                task_flags & TASK_COROUTINE
            };
        h.op(OP_FINISH_HEADER, flags);
        if h.is(OP_STREAM_CONNECTION) {
            h.signed(OP_SEND_STREAM)
        } else {
            h.signed(OP_SEND_PIPE)
        }
    } else {
        h.get(OP_LOCK_WORKER);
        if task_flags & TASK_WAITALL != 0 {
            if h.is(OP_OPEN_RESULT_FILE) {
                if !h.is(OP_PACK_RESULT) {
                    h.log(LOG_LARGE_PACK, 0, 0);
                    h.get(OP_ZERO_RESULT);
                }
                h.op(OP_FINISH_HEADER, flags);
                if !h.is(OP_WRITE_RESULT_FILE) {
                    h.get(LOG_RESULT_WRITE);
                }
                h.get(OP_FINISH_COUNT);
            }
        } else if !h.is(OP_RESULT_IN_SHM) {
            h.get(OP_UNLOCK_WORKER);
            h.log(LOG_LARGE_PACK, 0, 0);
            return false;
        } else {
            h.op(OP_FINISH_HEADER, flags);
        }
        h.get(OP_UNLOCK_WORKER);
        loop {
            let retval = h.signed(OP_NOTIFY_RESULT);
            if retval < 0 && h.is(OP_NOTIFY_WOULD_BLOCK) && h.signed(OP_WAIT_NOTIFY_WRITABLE) == 0 {
                continue;
            }
            break retval;
        }
    };
    if retval < 0 {
        if h.is(OP_SEND_TIMED_OUT) {
            h.log(LOG_SEND_RESULT, 1, 0);
        } else {
            h.log(LOG_SEND_RESULT, 0, 0);
        }
        return false;
    }
    true
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::workers::ops::OP_SIGNAL_SET;
    use crate::workers::script::Script;

    fn run_op(h: &mut Script, operation: u32, arg: u64) -> i64 {
        crate::workers::run(h, operation, arg)
    }

    #[test]
    fn init_rejects_coroutines_over_message_queues() {
        for (mode, coroutine, ok, sysvmsg) in [
            (1, 0, true, false),
            (TASK_IPC_PREEMPTIVE, 0, true, true),
            (TASK_IPC_MSGQUEUE, 1, false, false),
            (TASK_IPC_PREEMPTIVE, 1, false, false),
            (1, 1, true, false),
        ] {
            let mut h = Script::with(&[
                (OP_TASK_IPC_MODE, mode),
                (OP_TASK_ENABLE_COROUTINE, coroutine),
                (OP_TASK_WORKER_NUM, 2),
            ]);
            assert_eq!(run_op(&mut h, RUN_INIT_TASK_WORKERS, 0) == 1, ok);
            assert_eq!(h.called(OP_SET_SYSVMSG) == 1, sysvmsg);
            assert_eq!(
                h.args(OP_CREATE_TASK_WORKER),
                if ok { vec![0, 1] } else { vec![] }
            );
        }
    }

    #[test]
    fn on_task_counts_only_server_tasks() {
        let mut h = Script::with(&[(OP_ON_TASK, 0)]);
        assert_eq!(run_op(&mut h, RUN_TASK_ON_TASK, 0), SW_OK);
        assert_eq!(h.called(OP_ADD_REQUEST_COUNT), 1);
        assert_eq!(h.args(OP_WORKER_STATUS), vec![STATUS_BUSY, STATUS_IDLE]);

        let mut h = Script::with(&[(OP_INFO_TYPE, u64::from(EVENT_PIPE_MESSAGE))]);
        run_op(&mut h, RUN_TASK_ON_TASK, 0);
        assert_eq!(
            (h.called(OP_ON_PIPE_MESSAGE), h.called(OP_ADD_REQUEST_COUNT)),
            (1, 0)
        );

        let mut h = Script::with(&[
            (OP_INFO_TYPE, u64::from(EVENT_SHUTDOWN)),
            (OP_EVENT_AVAILABLE, 1),
        ]);
        run_op(&mut h, RUN_TASK_ON_TASK, 0);
        assert_eq!(h.called(OP_STOP_ASYNC), 1);
        assert_eq!(h.args(OP_WORKER_STATUS), vec![STATUS_BUSY]);

        let mut h = Script::with(&[(OP_INFO_TYPE, u64::from(EVENT_COMMAND_REQUEST))]);
        assert_eq!(run_op(&mut h, RUN_TASK_ON_TASK, 0), SW_OK);
        assert_eq!(h.called(LOG_UNKNOWN_COMMAND), 1);
        let mut h = Script::with(&[
            (OP_INFO_TYPE, u64::from(EVENT_COMMAND_REQUEST)),
            (OP_HAS_COMMAND, 1),
            (OP_UNPACK_COMMAND, 1),
        ]);
        assert_eq!(run_op(&mut h, RUN_TASK_ON_TASK, 0), SW_ERR);
    }

    #[test]
    fn on_start_and_the_pipe_handler() {
        let mut h = Script::with(&[(OP_TASK_ENABLE_COROUTINE, 1), (OP_EVENT_INIT, u64::MAX)]);
        run_op(&mut h, RUN_TASK_ON_START, 0);
        assert_eq!(
            (
                h.called(OP_FAIL_REACTOR),
                h.called(OP_WORKER_START_CALLBACK)
            ),
            (1, 0)
        );
        let mut h = Script::default();
        run_op(&mut h, RUN_TASK_ON_START, 0);
        assert_eq!(h.args(OP_ENABLE_SIGNALFD), vec![0]);
        assert_eq!(h.args(OP_SIGNAL_SET)[1] >> 32, HANDLER_NONE);
        assert_eq!(h.called(OP_WORKER_INIT), 1);

        let mut h = Script::default();
        assert_eq!(run_op(&mut h, RUN_TASK_ON_PIPE, 0), SW_ERR);
        assert_eq!(h.called(LOG_READ_TASK), 1);
        let mut h = Script::with(&[
            (OP_READ_TASK, 8),
            (OP_ON_TASK_POOL, 0),
            (OP_EXCEEDED_MAX_REQUEST, 1),
        ]);
        assert_eq!(run_op(&mut h, RUN_TASK_ON_PIPE, 0), SW_OK);
        assert_eq!(h.called(OP_STOP_ASYNC), 1);
    }

    #[test]
    fn finish_refuses_pipe_messages_noreply_and_unknown_workers() {
        let mut h = Script::default();
        assert_eq!(run_op(&mut h, RUN_FINISH, 0), 0);
        assert_eq!(h.called(LOG_NO_TASK_WORKERS), 1);
        // No task given and none running: nothing to answer, and no head is read.
        let mut h = Script::with(&[(OP_TASK_WORKER_NUM, 1)]);
        assert_eq!(run_op(&mut h, RUN_FINISH, 0), 0);
        assert_eq!(h.called(OP_INFO_TYPE), 0);
        let base = [(OP_TASK_WORKER_NUM, 1), (OP_SELECT_TASK, 1)];
        let mut h = Script::with(&base);
        h.facts.insert(OP_INFO_TYPE, u64::from(EVENT_PIPE_MESSAGE));
        assert_eq!(run_op(&mut h, RUN_FINISH, 0), 0);
        assert_eq!(h.called(LOG_FINISH_IN_PIPE), 1);
        let mut h = Script::with(&base);
        h.facts.insert(OP_TASK_EXT_FLAGS, TASK_NOREPLY);
        assert_eq!(run_op(&mut h, RUN_FINISH, 0), 0);
        assert_eq!(h.called(LOG_FINISH_NOREPLY), 1);
        let mut h = Script::with(&base);
        assert_eq!(run_op(&mut h, RUN_FINISH, 0), 0);
        assert_eq!(h.called(LOG_INVALID_SOURCE), 1);
    }

    #[test]
    fn finish_nonblocking_adds_the_callback_or_coroutine_flag() {
        for (task_flags, added) in [
            (TASK_NONBLOCK, 0),
            (TASK_NONBLOCK | TASK_COROUTINE, TASK_COROUTINE),
            (
                TASK_NONBLOCK | TASK_CALLBACK | TASK_COROUTINE,
                TASK_CALLBACK,
            ),
        ] {
            let mut h = Script::with(&[
                (OP_TASK_WORKER_NUM, 1),
                (OP_SELECT_TASK, 1),
                (OP_TASK_EXT_FLAGS, task_flags),
                (OP_SOURCE_WORKER, 1),
                (OP_PACK_RESULT, 1),
                (OP_SEND_PIPE, 10),
            ]);
            assert_eq!(run_op(&mut h, RUN_FINISH, 1), 1);
            assert_eq!(h.args(OP_FINISH_HEADER), vec![1 | added]);
        }
        let mut h = Script::with(&[
            (OP_TASK_WORKER_NUM, 1),
            (OP_SELECT_TASK, 1),
            (OP_TASK_EXT_FLAGS, TASK_NONBLOCK),
            (OP_SOURCE_WORKER, 1),
            (OP_PACK_RESULT, 1),
            (OP_STREAM_CONNECTION, 1),
            (OP_SEND_STREAM, u64::MAX),
            (OP_SEND_TIMED_OUT, 1),
        ]);
        assert_eq!(run_op(&mut h, RUN_FINISH, 0), 0);
        assert_eq!(
            (h.called(OP_SEND_PIPE), h.args(LOG_SEND_RESULT)),
            (0, vec![1])
        );
    }

    #[test]
    fn finish_blocking_writes_the_slot_under_the_lock_then_notifies() {
        let mut h = Script::with(&[
            (OP_TASK_WORKER_NUM, 1),
            (OP_SELECT_TASK, 1),
            (OP_SOURCE_WORKER, 1),
            (OP_RESULT_IN_SHM, 1),
            (OP_NOTIFY_RESULT, 8),
        ])
        .queue(OP_NOTIFY_RESULT, &[u64::MAX, 8])
        .queue(OP_NOTIFY_WOULD_BLOCK, &[1]);
        assert_eq!(run_op(&mut h, RUN_FINISH, 0), 1);
        assert_eq!(h.called(OP_NOTIFY_RESULT), 2);
        let order = h.effects(&[
            OP_TASK_WORKER_NUM,
            OP_SELECT_TASK,
            OP_INFO_TYPE,
            OP_TASK_EXT_FLAGS,
            OP_SOURCE_WORKER,
        ]);
        assert_eq!(
            order[..4],
            [
                OP_LOCK_WORKER,
                OP_RESULT_IN_SHM,
                OP_FINISH_HEADER,
                OP_UNLOCK_WORKER
            ]
        );

        let mut h = Script::with(&[
            (OP_TASK_WORKER_NUM, 1),
            (OP_SELECT_TASK, 1),
            (OP_SOURCE_WORKER, 1),
        ]);
        assert_eq!(run_op(&mut h, RUN_FINISH, 0), 0);
        assert_eq!(
            (h.called(OP_UNLOCK_WORKER), h.called(LOG_LARGE_PACK)),
            (1, 1)
        );
        assert_eq!(h.called(OP_NOTIFY_RESULT), 0);

        let mut h = Script::with(&[
            (OP_TASK_WORKER_NUM, 1),
            (OP_SELECT_TASK, 1),
            (OP_TASK_EXT_FLAGS, TASK_WAITALL),
            (OP_SOURCE_WORKER, 1),
            (OP_OPEN_RESULT_FILE, 1),
        ]);
        run_op(&mut h, RUN_FINISH, 0);
        assert_eq!(
            (
                h.called(OP_ZERO_RESULT),
                h.called(LOG_RESULT_WRITE),
                h.called(OP_FINISH_COUNT)
            ),
            (1, 1, 1)
        );
    }
}
