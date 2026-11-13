//! The process-mode factory (upstream's `src/server/process.cc`): its setup and teardown,
//! forking event and user workers, and the reactor threads' dispatch to workers
//! and the workers' sends and closes back through them. The context holds the
//! server and, per operation, the worker, the data sent and its connection.
use super::event_worker::EVENT_WORKER;
use super::log::{
    LOG_CONN_NOT_ACTIVE, LOG_END_CLOSED, LOG_END_NOT_EXIST, LOG_FORK_EVENT, LOG_FORK_USER,
    LOG_INVALID_TARGET, LOG_OVERFLOW, LOG_SESSION_CLOSED, LOG_SESSION_CLOSING,
    LOG_SESSION_NOT_EXIST, LOG_TOO_LARGE,
};
use super::ops::{
    EVENT_CLOSE, EVENT_RECV_DATA, EVENT_SEND_DATA, EVENT_SEND_FILE, LOG_ALLOC_CONNECTIONS,
    LOG_KILL_MANAGER, LOG_WAIT_MANAGER, LOG_WAIT_TASK_WORKER, OP_ALLOC_CONNECTIONS,
    OP_CALL_WORKER_ERROR, OP_CLEAR_CLOSE_ERRNO, OP_CONN_CLOSE_ACTIVELY, OP_CONN_CLOSE_FORCE,
    OP_CONN_CLOSE_RESET, OP_CONN_CLOSED, OP_CONN_CLOSING, OP_CONN_FD, OP_CONN_OVERFLOW,
    OP_CONN_PEER_CLOSED, OP_COUNT_DISPATCH, OP_CREATE_PIPE_BUFFERS, OP_CREATE_WORKER_PIPE,
    OP_CURRENT_WORKER_ID, OP_DELETE_REACTOR_THREADS, OP_DESTROY_BARRIERS, OP_DESTROY_MESSAGE_BOX,
    OP_DO_CLOSE_CALLBACK, OP_ENTER_CHILD, OP_EXIT, OP_EXIT_STATUS, OP_FINISH_CLOSE,
    OP_FORGET_USER_WORKER, OP_FORK, OP_FORWARD_CLOSE, OP_FREE_CONNECTIONS, OP_HAS_MESSAGE_BOX,
    OP_HASH_DISPATCH, OP_INIT_BARRIERS, OP_INIT_IPC_MAX_SIZE, OP_IS_WORKER, OP_KILL_MANAGER,
    OP_KILL_TASK_POOL, OP_MODE, OP_NEW_PROCESS_FACTORY, OP_NEW_REACTOR_THREADS, OP_ON_CLOSE,
    OP_ON_USER_WORKER_START, OP_OUTPUT_BUFFER_SIZE, OP_POOL_MAIN_LOOP, OP_REGISTER_USER_WORKER,
    OP_REGISTER_WORKER, OP_REPORT_ERROR, OP_SCHEDULE_CONN, OP_SCHEDULE_WORKER, OP_SEND_CONN_ACTIVE,
    OP_SEND_FD, OP_SEND_LEN, OP_SEND_TYPE, OP_SEND_YIELD, OP_SESSION_CONN, OP_SET_CLOSE_ACTIVELY,
    OP_SET_CLOSE_RESET, OP_SET_REACTOR_PIPE_NUM, OP_SET_SEND_YIELD, OP_START_EVENT_WORKER,
    OP_START_MANAGER, OP_STREAM_EVENT, OP_TASK_WORKER_NUM, OP_TO_SESSION, OP_WAIT_MANAGER,
    OP_WAIT_TASK_WORKER, OP_WORKER_NUM, OP_WORKER_PID, OP_WRITE_TO_REACTOR, OP_WRITE_TO_WORKER,
    RUN_CHECK_EXIT_STATUS, RUN_CREATE_PROCESS_FACTORY, RUN_CREATE_WORKER_PIPES,
    RUN_DESTROY_PROCESS_FACTORY, RUN_KILL_TASK_WORKERS, RUN_PROCESS_DISPATCH, RUN_PROCESS_END,
    RUN_PROCESS_FACTORY_SHUTDOWN, RUN_PROCESS_FACTORY_START, RUN_PROCESS_FINISH,
    RUN_SPAWN_EVENT_WORKER, RUN_SPAWN_USER_WORKER,
};
use super::{Host, MODE_BASE, SW_ERR, SW_OK};

/// `Server::DISPATCH_RESULT_*`.
const DISPATCH_DISCARD_PACKET: i64 = -1;
const DISPATCH_CLOSE_CONNECTION: i64 = -2;
/// `Server::CLOSE_RESET`, `Server::CLOSE_ACTIVELY`.
const CLOSE_RESET: u64 = 1 << 1;
const CLOSE_ACTIVELY: u64 = 1 << 2;

pub(super) fn run<H: Host>(h: &mut H, operation: u32, arg: u64) -> i64 {
    match operation {
        RUN_CREATE_PROCESS_FACTORY => create(h),
        RUN_DESTROY_PROCESS_FACTORY => {
            h.get(OP_FREE_CONNECTIONS);
            h.get(OP_DELETE_REACTOR_THREADS);
            h.get(OP_DESTROY_BARRIERS);
            if h.is(OP_HAS_MESSAGE_BOX) {
                h.get(OP_DESTROY_MESSAGE_BOX);
            }
            SW_OK
        }
        RUN_KILL_TASK_WORKERS => {
            kill_task_workers(h);
            SW_OK
        }
        RUN_SPAWN_EVENT_WORKER => spawn_event_worker(h),
        RUN_SPAWN_USER_WORKER => spawn_user_worker(h),
        RUN_CHECK_EXIT_STATUS => {
            if h.get(OP_EXIT_STATUS) != 0 {
                h.get(OP_REPORT_ERROR);
                h.get(OP_CALL_WORKER_ERROR);
            }
            SW_OK
        }
        RUN_PROCESS_FACTORY_SHUTDOWN => {
            if h.signed(OP_KILL_MANAGER) < 0 {
                h.get(LOG_KILL_MANAGER);
            }
            if h.signed(OP_WAIT_MANAGER) < 0 {
                h.get(LOG_WAIT_MANAGER);
            }
            // Upstream returns SW_OK from this bool function.
            i64::from(SW_OK != 0)
        }
        RUN_CREATE_WORKER_PIPES => i64::from(create_worker_pipes(h)),
        RUN_PROCESS_FACTORY_START => {
            i64::from(create_worker_pipes(h) && h.signed(OP_START_MANAGER) == SW_OK)
        }
        RUN_PROCESS_DISPATCH => i64::from(dispatch(h)),
        RUN_PROCESS_FINISH => i64::from(finish(h)),
        RUN_PROCESS_END => i64::from(end(h, arg)),
        _ => SW_ERR,
    }
}

/// `Server::create_process_factory`: the factory's address, or 0.
fn create<H: Host>(h: &mut H) -> i64 {
    h.get(OP_NEW_REACTOR_THREADS);
    if !h.is(OP_ALLOC_CONNECTIONS) {
        h.get(LOG_ALLOC_CONNECTIONS);
        return 0;
    }
    h.get(OP_SET_REACTOR_PIPE_NUM);
    h.get(OP_INIT_BARRIERS);
    h.signed(OP_NEW_PROCESS_FACTORY)
}

/// `Factory::kill_task_workers`: SIGTERM each task worker, then wait for each.
fn kill_task_workers<H: Host>(h: &mut H) {
    if h.get(OP_TASK_WORKER_NUM) == 0 {
        return;
    }
    h.get(OP_KILL_TASK_POOL);
    for worker in 0..h.get(OP_TASK_WORKER_NUM) {
        if h.op(OP_WAIT_TASK_WORKER, worker).cast_signed() < 0 {
            h.op(LOG_WAIT_TASK_WORKER, worker);
        }
    }
}

/// `Factory::spawn_event_worker`: fork; the child runs the worker and exits.
fn spawn_event_worker<H: Host>(h: &mut H) -> i64 {
    let pid = h.signed(OP_FORK);
    if pid < 0 {
        h.log(LOG_FORK_EVENT, 0, 0);
        return SW_ERR;
    }
    if pid > 0 {
        h.op(OP_REGISTER_WORKER, pid.cast_unsigned());
        return pid;
    }
    h.op(OP_ENTER_CHILD, EVENT_WORKER);
    if h.get(OP_MODE) == MODE_BASE {
        h.get(OP_POOL_MAIN_LOOP);
    } else {
        h.get(OP_START_EVENT_WORKER);
    }
    h.get(OP_EXIT);
    0
}

/// `Factory::spawn_user_worker`.
fn spawn_user_worker<H: Host>(h: &mut H) -> i64 {
    let pid = h.signed(OP_FORK);
    // Both processes forget the worker's previous pid.
    let previous = h.get(OP_WORKER_PID);
    if previous != 0 {
        h.op(OP_FORGET_USER_WORKER, previous);
    }
    if pid < 0 {
        h.log(LOG_FORK_USER, 0, 0);
        return SW_ERR;
    }
    if pid == 0 {
        h.op(OP_ENTER_CHILD, USER_WORKER_TYPE);
        h.get(OP_ON_USER_WORKER_START);
        h.get(OP_EXIT);
        return 0;
    }
    // The worker is local memory, the server's copy shared memory: set both.
    h.op(OP_REGISTER_USER_WORKER, pid.cast_unsigned());
    pid
}

const USER_WORKER_TYPE: u64 = crate::worker_record::WorkerType::User as u64;

/// `Server::create_worker_pipes`.
fn create_worker_pipes<H: Host>(h: &mut H) -> bool {
    for worker in 0..h.get(OP_WORKER_NUM) {
        if h.op(OP_CREATE_WORKER_PIPE, worker) == 0 {
            return false;
        }
    }
    h.get(OP_INIT_IPC_MAX_SIZE);
    h.signed(OP_CREATE_PIPE_BUFFERS) >= 0
}

/// `ProcessFactory::dispatch`: a reactor thread hands a request to its worker.
fn dispatch<H: Host>(h: &mut H) -> bool {
    let target = h.signed(OP_SCHEDULE_WORKER);
    if target < 0 {
        if target != DISPATCH_DISCARD_PACKET && target != DISPATCH_CLOSE_CONNECTION {
            h.log(LOG_INVALID_TARGET, target, 0);
        }
        return false;
    }
    let kind = u32::try_from(h.get(OP_SEND_TYPE)).unwrap_or(u32::MAX);
    if h.is(OP_STREAM_EVENT) {
        if !h.is(OP_SEND_CONN_ACTIVE) {
            let fd = h.signed(OP_SEND_FD);
            h.log(LOG_CONN_NOT_ACTIVE, kind.into(), fd);
            return false;
        }
        // The server closed it: drop the data, unless this is its forced close.
        if h.is(OP_CONN_CLOSED) && !(kind == EVENT_CLOSE && h.is(OP_CONN_CLOSE_FORCE)) {
            return false;
        }
        h.get(OP_TO_SESSION);
    }
    if kind == EVENT_RECV_DATA {
        h.op(OP_COUNT_DISPATCH, target.cast_unsigned());
    }
    h.op(OP_WRITE_TO_WORKER, target.cast_unsigned()) != 0
}

/// `ProcessFactory::finish`: a worker sends to a client through its reactor thread.
fn finish<H: Host>(h: &mut H) -> bool {
    let (len, size) = (h.signed(OP_SEND_LEN), h.signed(OP_OUTPUT_BUFFER_SIZE));
    if len > size {
        h.log(LOG_TOO_LARGE, len, size);
        return false;
    }
    let kind = u32::try_from(h.get(OP_SEND_TYPE)).unwrap_or(u32::MAX);
    let close = kind == EVENT_CLOSE;
    if h.op(OP_SESSION_CONN, u64::from(close)) == 0 {
        if !close {
            let session = h.signed(OP_SEND_FD);
            h.log(LOG_SESSION_NOT_EXIST, session, 0);
        }
        return false;
    }
    if (h.is(OP_CONN_CLOSED) || h.is(OP_CONN_PEER_CLOSED)) && !close {
        let session = h.signed(OP_SEND_FD);
        h.log(LOG_SESSION_CLOSED, len, session);
        return false;
    }
    if h.is(OP_CONN_OVERFLOW) && (kind == EVENT_SEND_DATA || kind == EVENT_SEND_FILE) {
        // `process_is_supported_send_yield`
        if h.is(OP_SEND_YIELD)
            && h.is(OP_HASH_DISPATCH)
            && h.signed(OP_SCHEDULE_CONN) == h.signed(OP_CURRENT_WORKER_ID)
        {
            h.get(OP_SET_SEND_YIELD);
        } else {
            let session = h.signed(OP_SEND_FD);
            h.log(LOG_OVERFLOW, session, 0);
        }
        return false;
    }
    h.is(OP_WRITE_TO_REACTOR)
}

/// `ProcessFactory::end`: close a session, forwarding an active close to the
/// worker that owns the connection.
fn end<H: Host>(h: &mut H, flags: u64) -> bool {
    if h.op(OP_SESSION_CONN, 1) == 0 {
        let session = h.signed(OP_SEND_FD);
        h.log(LOG_END_NOT_EXIST, session, 0);
        return false;
    }
    // Reset the send buffer: close immediately.
    if flags & CLOSE_RESET != 0 {
        h.get(OP_SET_CLOSE_RESET);
    }
    // The server closes the connection itself.
    if flags & CLOSE_ACTIVELY != 0 {
        h.get(OP_SET_CLOSE_ACTIVELY);
    }
    // Only an active close may be in another worker's binding: forward it there.
    if h.is(OP_CONN_CLOSE_ACTIVELY) {
        let hash = h.is(OP_HASH_DISPATCH);
        let worker = if hash {
            h.signed(OP_SCHEDULE_CONN)
        } else {
            let workers = h.signed(OP_WORKER_NUM);
            if workers == 0 {
                0
            } else {
                h.signed(OP_CONN_FD) % workers
            }
        };
        if !(h.is(OP_IS_WORKER) && (!hash || worker == h.signed(OP_CURRENT_WORKER_ID))) {
            return h.op(OP_FORWARD_CLOSE, worker.cast_unsigned()).cast_signed() > 0;
        }
    }
    if h.is(OP_CONN_CLOSING) {
        let session = h.signed(OP_SEND_FD);
        h.log(LOG_SESSION_CLOSING, session, 0);
        return false;
    }
    if !(h.is(OP_CONN_CLOSE_FORCE) || h.is(OP_CONN_CLOSE_RESET)) && h.is(OP_CONN_CLOSED) {
        let session = h.signed(OP_SEND_FD);
        h.log(LOG_END_CLOSED, session, 0);
        return false;
    }
    if h.is(OP_DO_CLOSE_CALLBACK) {
        h.get(OP_ON_CLOSE);
    }
    h.get(OP_CLEAR_CLOSE_ERRNO);
    h.is(OP_FINISH_CLOSE)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::workers::script::Script;

    fn run_op(h: &mut Script, operation: u32, arg: u64) -> i64 {
        crate::workers::run(h, operation, arg)
    }

    #[test]
    fn shutdown_returns_upstreams_sw_ok_as_false() {
        let mut h = Script::with(&[(OP_KILL_MANAGER, u64::MAX)]);
        assert_eq!(run_op(&mut h, RUN_PROCESS_FACTORY_SHUTDOWN, 0), 0);
        assert_eq!(
            (h.called(LOG_KILL_MANAGER), h.called(LOG_WAIT_MANAGER)),
            (1, 0)
        );
    }

    #[test]
    fn spawn_event_worker_parent_and_child() {
        let mut h = Script::with(&[(OP_FORK, 99)]);
        assert_eq!(run_op(&mut h, RUN_SPAWN_EVENT_WORKER, 0), 99);
        assert_eq!(h.args(OP_REGISTER_WORKER), vec![99]);
        let mut h = Script::with(&[(OP_MODE, MODE_BASE)]);
        assert_eq!(run_op(&mut h, RUN_SPAWN_EVENT_WORKER, 0), 0);
        assert_eq!(
            h.effects(&[OP_FORK, OP_MODE]),
            vec![OP_ENTER_CHILD, OP_POOL_MAIN_LOOP, OP_EXIT]
        );
        let mut h = Script::with(&[(OP_FORK, u64::MAX)]);
        assert_eq!(run_op(&mut h, RUN_SPAWN_EVENT_WORKER, 0), SW_ERR);
        assert_eq!(h.called(LOG_FORK_EVENT), 1);
    }

    #[test]
    fn spawn_user_worker_forgets_the_old_pid_in_both_processes() {
        let mut h = Script::with(&[(OP_FORK, 99), (OP_WORKER_PID, 7)]);
        assert_eq!(run_op(&mut h, RUN_SPAWN_USER_WORKER, 0), 99);
        assert_eq!(h.args(OP_FORGET_USER_WORKER), vec![7]);
        assert_eq!(h.args(OP_REGISTER_USER_WORKER), vec![99]);
        let mut h = Script::with(&[(OP_WORKER_PID, 7)]);
        assert_eq!(run_op(&mut h, RUN_SPAWN_USER_WORKER, 0), 0);
        assert_eq!(h.args(OP_FORGET_USER_WORKER), vec![7]);
        assert_eq!(h.called(OP_ON_USER_WORKER_START), 1);
    }

    #[test]
    fn dispatch_drops_closed_connections_but_not_their_forced_close() {
        let mut h = Script::with(&[(OP_SCHEDULE_WORKER, u64::MAX)]);
        assert_eq!(run_op(&mut h, RUN_PROCESS_DISPATCH, 0), 0);
        assert_eq!(h.called(LOG_INVALID_TARGET), 0);
        let mut h = Script::with(&[(OP_SCHEDULE_WORKER, (-5_i64).cast_unsigned())]);
        run_op(&mut h, RUN_PROCESS_DISPATCH, 0);
        assert_eq!(h.called(LOG_INVALID_TARGET), 1);
        for (kind, force, sent) in [
            (EVENT_RECV_DATA, 1, false),
            (EVENT_CLOSE, 1, true),
            (EVENT_CLOSE, 0, false),
        ] {
            let mut h = Script::with(&[
                (OP_SCHEDULE_WORKER, 2),
                (OP_SEND_TYPE, u64::from(kind)),
                (OP_STREAM_EVENT, 1),
                (OP_SEND_CONN_ACTIVE, 1),
                (OP_CONN_CLOSED, 1),
                (OP_CONN_CLOSE_FORCE, force),
                (OP_WRITE_TO_WORKER, 1),
            ]);
            assert_eq!(run_op(&mut h, RUN_PROCESS_DISPATCH, 0) == 1, sent);
        }
        let mut h = Script::with(&[(OP_SCHEDULE_WORKER, 2), (OP_WRITE_TO_WORKER, 1)]);
        assert_eq!(run_op(&mut h, RUN_PROCESS_DISPATCH, 0), 1);
        assert_eq!(h.args(OP_COUNT_DISPATCH), vec![2]);
    }

    #[test]
    fn finish_checks_size_session_and_overflow() {
        let mut h = Script::with(&[(OP_SEND_LEN, 10), (OP_OUTPUT_BUFFER_SIZE, 5)]);
        assert_eq!(run_op(&mut h, RUN_PROCESS_FINISH, 0), 0);
        assert_eq!(h.called(LOG_TOO_LARGE), 1);
        let mut h = Script::with(&[(OP_SEND_TYPE, u64::from(EVENT_CLOSE))]);
        run_op(&mut h, RUN_PROCESS_FINISH, 0);
        assert_eq!(
            (h.args(OP_SESSION_CONN), h.called(LOG_SESSION_NOT_EXIST)),
            (vec![1], 0)
        );
        let mut h = Script::with(&[
            (OP_SEND_TYPE, u64::from(EVENT_SEND_DATA)),
            (OP_SESSION_CONN, 1),
            (OP_CONN_OVERFLOW, 1),
            (OP_SEND_YIELD, 1),
            (OP_HASH_DISPATCH, 1),
        ]);
        assert_eq!(run_op(&mut h, RUN_PROCESS_FINISH, 0), 0);
        assert_eq!(
            (h.called(OP_SET_SEND_YIELD), h.called(LOG_OVERFLOW)),
            (1, 0)
        );
        let mut h = Script::with(&[(OP_SESSION_CONN, 1), (OP_WRITE_TO_REACTOR, 1)]);
        assert_eq!(run_op(&mut h, RUN_PROCESS_FINISH, 0), 1);
    }

    #[test]
    fn end_forwards_active_closes_owned_elsewhere() {
        let mut h = Script::with(&[
            (OP_SESSION_CONN, 1),
            (OP_CONN_CLOSE_ACTIVELY, 1),
            (OP_WORKER_NUM, 4),
            (OP_CONN_FD, 9),
            (OP_FORWARD_CLOSE, 16),
        ]);
        assert_eq!(run_op(&mut h, RUN_PROCESS_END, CLOSE_ACTIVELY), 1);
        assert_eq!(h.args(OP_FORWARD_CLOSE), vec![1]);
        assert_eq!(h.called(OP_SET_CLOSE_ACTIVELY), 1);
        let mut h = Script::with(&[
            (OP_SESSION_CONN, 1),
            (OP_CONN_CLOSE_ACTIVELY, 1),
            (OP_IS_WORKER, 1),
            (OP_DO_CLOSE_CALLBACK, 1),
            (OP_FINISH_CLOSE, 1),
        ]);
        assert_eq!(run_op(&mut h, RUN_PROCESS_END, 0), 1);
        assert_eq!((h.called(OP_FORWARD_CLOSE), h.called(OP_ON_CLOSE)), (0, 1));
        let mut h = Script::with(&[(OP_SESSION_CONN, 1), (OP_CONN_CLOSING, 1)]);
        assert_eq!(run_op(&mut h, RUN_PROCESS_END, 0), 0);
        assert_eq!(h.called(LOG_SESSION_CLOSING), 1);
        let mut h = Script::with(&[(OP_SESSION_CONN, 1), (OP_CONN_CLOSED, 1)]);
        assert_eq!(run_op(&mut h, RUN_PROCESS_END, 0), 0);
        assert_eq!(h.called(LOG_END_CLOSED), 1);
    }
}
