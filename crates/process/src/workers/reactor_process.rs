//! Base-mode reactor processes (upstream's `src/server/reactor_process.cc`;
//! workers.cc's `ReactorProcessContext`): every worker
//! accepts, reads and runs callbacks itself. The context holds the server and,
//! per operation, the pool, worker, reactor, event and connection.
use super::log::LOG_INVALID_WORKER;
use super::ops::{
    EVENT_CLOSE, EVENT_CLOSE_FORWARD, EVENT_COMMAND_REQUEST, EVENT_COMMAND_RESPONSE, EVENT_FINISH,
    EVENT_PIPE_MESSAGE, EVENT_SEND_DATA, EVENT_SEND_FILE, EVENT_SHUTDOWN, OP_ADD_HEARTBEAT_TIMER,
    OP_ADD_MASTER_TIMER, OP_ADD_PIPES, OP_ALL_WORKER_NUM, OP_BUS_POP, OP_BUS_READ,
    OP_CLOSE_CONNECTION, OP_CLOSE_EVENT_SOCKET, OP_CLOSE_NOTIFY, OP_CLOSE_SESSION,
    OP_COMMAND_REQUEST, OP_COMMAND_RESPONSE, OP_CONN_CLOSE_FORCE, OP_CONN_CLOSE_QUEUED,
    OP_CONN_HEALTHY, OP_CONN_SSL_PENDING, OP_CREATE_PIPE_BUFFERS, OP_DISABLE_NOTIFY,
    OP_END_ACTIVELY, OP_EVENT_CONN_ACTIVE, OP_EVENT_FREE, OP_EVENT_INIT, OP_EVENT_SOCKET_REMOVED,
    OP_FINISH_SEND_DATA, OP_FINISH_SEND_FILE, OP_HAS_ON_START, OP_HAS_PIPE_WORKER, OP_HAS_REACTOR,
    OP_HAVE_STREAM_SOCK, OP_HEARTBEAT_INTERVAL, OP_HEARTBEAT_NOW, OP_INFO_REACTOR_ID, OP_INFO_TYPE,
    OP_INIT_IPC_MAX_SIZE, OP_INIT_REACTOR, OP_NOTIFY_CLOSE, OP_ON_FINISH, OP_ON_PIPE_MESSAGE,
    OP_ON_START, OP_ONSTART_CALLED, OP_POOL_CREATE, OP_POOL_DESTROY, OP_PORT_ADD,
    OP_PORT_CLOSE_SOCKET, OP_PORT_COUNT, OP_PORT_CREATE_SOCKET, OP_PORT_LISTEN, OP_PORT_REUSE,
    OP_PORT_STREAM, OP_REACTOR_DEL_EVENT, OP_REACTOR_SETUP, OP_REACTOR_WAIT,
    OP_RESET_CONNECTION_NUM, OP_REUSE_PORT, OP_RUN_SINGLE_WORKER, OP_SET_CLOSE_FORCE,
    OP_SET_MASTER_PID, OP_SET_PEER_CLOSED, OP_SET_SINGLE_THREAD, OP_SINGLE_WORKER,
    OP_START_MANAGER, OP_STOP_ASYNC, OP_STORE_LISTEN_SOCKET, OP_TASK_IPC_UNIXSOCK,
    OP_TASK_PIPE_NONBLOCK, OP_TASK_POOL_WORKER_NUM, OP_TASK_WORKER_NUM, OP_TIMER_DETACHED,
    OP_TIMER_REINIT, OP_WORKER_ENTER, OP_WORKER_ID, OP_WORKER_SIGNAL_INIT,
    OP_WORKER_START_CALLBACK, OP_WORKER_STOP_CALLBACK, RUN_REACTOR_PROCESS_CLOSE,
    RUN_REACTOR_PROCESS_MAIN, RUN_REACTOR_PROCESS_PIPE_READ, RUN_REACTOR_PROCESS_START,
    RUN_REACTOR_PROCESS_TIMEOUT,
};
use super::{Host, SW_ERR, SW_OK, for_each_connection};

pub(super) fn run<H: Host>(h: &mut H, operation: u32) -> i64 {
    match operation {
        RUN_REACTOR_PROCESS_START => start(h),
        RUN_REACTOR_PROCESS_PIPE_READ => pipe_read(h),
        RUN_REACTOR_PROCESS_MAIN => main_loop(h),
        RUN_REACTOR_PROCESS_CLOSE => close(h),
        RUN_REACTOR_PROCESS_TIMEOUT => {
            heartbeat(h);
            SW_OK
        }
        _ => SW_ERR,
    }
}

/// `Server::start_reactor_processes`.
fn start<H: Host>(h: &mut H) -> i64 {
    h.get(OP_SET_SINGLE_THREAD);
    // Listen on the TCP ports; with SO_REUSEPORT each worker listens itself.
    if h.is(OP_HAVE_STREAM_SOCK) {
        for port in 0..h.get(OP_PORT_COUNT) {
            if h.op(OP_PORT_STREAM, port) == 0 {
                continue;
            }
            if h.is(OP_REUSE_PORT) {
                h.op(OP_PORT_CLOSE_SOCKET, port);
            } else if h.op(OP_PORT_LISTEN, port).cast_signed() < 0 {
                return SW_ERR;
            }
        }
    }
    if h.signed(OP_POOL_CREATE) < 0 {
        return SW_ERR;
    }
    h.get(OP_INIT_IPC_MAX_SIZE);
    if h.signed(OP_CREATE_PIPE_BUFFERS) < 0 {
        return SW_ERR;
    }
    let rv = if h.is(OP_SINGLE_WORKER) {
        h.signed(OP_RUN_SINGLE_WORKER)
    } else {
        h.signed(OP_START_MANAGER)
    };
    if rv == SW_OK {
        h.get(OP_POOL_DESTROY);
    }
    rv
}

/// `ReactorProcess_onPipeRead`: a message from another worker or a task worker.
fn pipe_read<H: Host>(h: &mut H) -> i64 {
    if h.signed(OP_BUS_READ) <= 0 {
        return SW_OK;
    }
    match u32::try_from(h.get(OP_INFO_TYPE)).unwrap_or(u32::MAX) {
        EVENT_PIPE_MESSAGE => {
            h.get(OP_ON_PIPE_MESSAGE);
        }
        EVENT_FINISH => {
            h.get(OP_ON_FINISH);
        }
        EVENT_SHUTDOWN => {
            h.get(OP_STOP_ASYNC);
        }
        EVENT_SEND_FILE => {
            h.get(OP_FINISH_SEND_FILE);
        }
        EVENT_SEND_DATA => {
            let worker = h.signed(OP_INFO_REACTOR_ID);
            if worker < 0 || worker >= h.signed(OP_ALL_WORKER_NUM) {
                h.log(LOG_INVALID_WORKER, worker, 0);
                return SW_OK;
            }
            h.get(OP_FINISH_SEND_DATA);
        }
        EVENT_CLOSE | EVENT_CLOSE_FORWARD => {
            h.get(OP_END_ACTIVELY);
        }
        EVENT_COMMAND_REQUEST => {
            h.get(OP_COMMAND_REQUEST);
        }
        EVENT_COMMAND_RESPONSE => {
            h.get(OP_COMMAND_RESPONSE);
        }
        _ => {}
    }
    h.get(OP_BUS_POP);
    SW_OK
}

/// `Server::reactor_process_main_loop`: one base-mode worker.
fn main_loop<H: Host>(h: &mut H) -> i64 {
    h.get(OP_WORKER_ENTER);
    if !h.is(OP_HAS_REACTOR) && h.signed(OP_EVENT_INIT) < 0 {
        return SW_ERR;
    }
    if h.is(OP_TIMER_DETACHED) {
        h.get(OP_TIMER_REINIT);
    }
    h.get(OP_WORKER_SIGNAL_INIT);
    h.get(OP_RESET_CONNECTION_NUM);
    for port in 0..h.get(OP_PORT_COUNT) {
        if h.op(OP_PORT_REUSE, port) != 0 {
            if h.op(OP_PORT_CREATE_SOCKET, port).cast_signed() < 0 {
                h.get(OP_EVENT_FREE);
                return SW_ERR;
            }
            if h.op(OP_PORT_LISTEN, port).cast_signed() < 0 {
                return SW_ERR;
            }
        }
        if h.op(OP_PORT_ADD, port).cast_signed() < 0 {
            return SW_ERR;
        }
    }
    h.get(OP_REACTOR_SETUP);
    h.get(OP_STORE_LISTEN_SOCKET);
    if h.is(OP_HAS_PIPE_WORKER) && h.signed(OP_ADD_PIPES) < 0 {
        return SW_ERR;
    }
    if h.get(OP_TASK_WORKER_NUM) > 0 && h.is(OP_TASK_IPC_UNIXSOCK) {
        for worker in 0..h.get(OP_TASK_POOL_WORKER_NUM) {
            h.op(OP_TASK_PIPE_NONBLOCK, worker);
        }
    }
    h.get(OP_INIT_REACTOR);
    if h.get(OP_WORKER_ID) == 0 {
        h.get(OP_SET_MASTER_PID);
        if h.is(OP_HAS_ON_START) && !h.is(OP_ONSTART_CALLED) {
            h.get(OP_ON_START);
        }
    }
    if !h.is(OP_ADD_MASTER_TIMER) {
        h.get(OP_EVENT_FREE);
        return SW_ERR;
    }
    h.get(OP_WORKER_START_CALLBACK);
    // The heartbeat check.
    if h.get(OP_HEARTBEAT_INTERVAL) > 0 && !h.is(OP_ADD_HEARTBEAT_TIMER) {
        h.get(OP_EVENT_FREE);
        return SW_ERR;
    }
    let rv = h.signed(OP_REACTOR_WAIT);
    // Close every connection.
    for_each_connection(h, |h| {
        h.get(OP_CLOSE_SESSION);
    });
    h.get(OP_EVENT_FREE);
    h.get(OP_WORKER_STOP_CALLBACK);
    rv
}

/// `ReactorProcess_onClose`: the peer closed (or errored) a connection.
fn close<H: Host>(h: &mut H) -> i64 {
    if !h.is(OP_EVENT_CONN_ACTIVE) {
        return SW_ERR;
    }
    if h.is(OP_EVENT_SOCKET_REMOVED) {
        return h.signed(OP_CLOSE_EVENT_SOCKET);
    }
    if h.signed(OP_REACTOR_DEL_EVENT) != 0 {
        return SW_ERR;
    }
    if h.is(OP_CONN_CLOSE_QUEUED) {
        return h.signed(OP_CLOSE_EVENT_SOCKET);
    }
    // The client closed the connection: it is no longer usable.
    h.get(OP_SET_PEER_CLOSED);
    if h.is(OP_NOTIFY_CLOSE) { SW_OK } else { SW_ERR }
}

/// `ReactorProcess_onTimeout`: close the connections idle past the heartbeat.
fn heartbeat<H: Host>(h: &mut H) {
    h.get(OP_HEARTBEAT_NOW);
    for_each_connection(h, |h| {
        if h.is(OP_CONN_HEALTHY) {
            return;
        }
        if h.is(OP_CONN_SSL_PENDING) || h.is(OP_DISABLE_NOTIFY) || h.is(OP_CONN_CLOSE_FORCE) {
            h.get(OP_CLOSE_CONNECTION);
            return;
        }
        h.get(OP_SET_CLOSE_FORCE);
        h.get(OP_CLOSE_NOTIFY);
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::workers::ops::{OP_CONN_AT, OP_MAXFD, OP_MINFD};
    use crate::workers::script::Script;

    fn run_op(h: &mut Script, operation: u32) -> i64 {
        crate::workers::run(h, operation, 0)
    }

    #[test]
    fn start_listens_unless_reuse_port_then_runs_the_workers() {
        let mut h = Script::with(&[
            (OP_HAVE_STREAM_SOCK, 1),
            (OP_PORT_COUNT, 3),
            (OP_SINGLE_WORKER, 1),
        ])
        .queue(OP_PORT_STREAM, &[1, 0, 1]);
        assert_eq!(run_op(&mut h, RUN_REACTOR_PROCESS_START), SW_OK);
        assert_eq!(h.args(OP_PORT_LISTEN), vec![0, 2]);
        assert_eq!(
            (h.called(OP_RUN_SINGLE_WORKER), h.called(OP_START_MANAGER)),
            (1, 0)
        );
        assert_eq!(h.called(OP_POOL_DESTROY), 1);

        let mut h = Script::with(&[
            (OP_HAVE_STREAM_SOCK, 1),
            (OP_PORT_COUNT, 1),
            (OP_PORT_STREAM, 1),
            (OP_REUSE_PORT, 1),
        ]);
        run_op(&mut h, RUN_REACTOR_PROCESS_START);
        assert_eq!(
            (h.called(OP_PORT_CLOSE_SOCKET), h.called(OP_PORT_LISTEN)),
            (1, 0)
        );
        assert_eq!(h.called(OP_START_MANAGER), 1);

        let mut h = Script::with(&[
            (OP_HAVE_STREAM_SOCK, 1),
            (OP_PORT_COUNT, 1),
            (OP_PORT_STREAM, 1),
            (OP_PORT_LISTEN, u64::MAX),
        ]);
        assert_eq!(run_op(&mut h, RUN_REACTOR_PROCESS_START), SW_ERR);
        assert_eq!(h.called(OP_POOL_CREATE), 0);

        let mut h = Script::with(&[(OP_START_MANAGER, u64::MAX)]);
        assert_eq!(run_op(&mut h, RUN_REACTOR_PROCESS_START), SW_ERR);
        assert_eq!(h.called(OP_POOL_DESTROY), 0);
    }

    #[test]
    fn pipe_read_rejects_send_data_from_an_unknown_worker_without_popping() {
        for (worker, valid) in [(u64::MAX, false), (0, true), (3, true), (4, false)] {
            let mut h = Script::with(&[
                (OP_BUS_READ, 1),
                (OP_INFO_TYPE, u64::from(EVENT_SEND_DATA)),
                (OP_INFO_REACTOR_ID, worker),
                (OP_ALL_WORKER_NUM, 4),
            ]);
            run_op(&mut h, RUN_REACTOR_PROCESS_PIPE_READ);
            assert_eq!(h.called(OP_FINISH_SEND_DATA) == 1, valid);
            assert_eq!(h.called(OP_BUS_POP) == 1, valid);
            assert_eq!(h.called(LOG_INVALID_WORKER) == 1, !valid);
        }
        let mut h = Script::with(&[
            (OP_BUS_READ, 1),
            (OP_INFO_TYPE, u64::from(EVENT_CLOSE_FORWARD)),
        ]);
        run_op(&mut h, RUN_REACTOR_PROCESS_PIPE_READ);
        assert_eq!((h.called(OP_END_ACTIVELY), h.called(OP_BUS_POP)), (1, 1));
        let mut h = Script::default();
        run_op(&mut h, RUN_REACTOR_PROCESS_PIPE_READ);
        assert_eq!(h.called(OP_BUS_POP), 0);
    }

    #[test]
    fn main_loop_order_and_cleanup() {
        let mut h = Script::with(&[
            (OP_HAS_REACTOR, 1),
            (OP_PORT_COUNT, 1),
            (OP_ADD_MASTER_TIMER, 1),
            (OP_REACTOR_WAIT, 0),
            (OP_MINFD, 1),
            (OP_MAXFD, 2),
            (OP_HAS_ON_START, 1),
        ])
        .queue(OP_CONN_AT, &[1, 1]);
        assert_eq!(run_op(&mut h, RUN_REACTOR_PROCESS_MAIN), SW_OK);
        assert_eq!(h.called(OP_ON_START), 1);
        assert_eq!(h.called(OP_CLOSE_SESSION), 2);
        assert_eq!(h.called(OP_EVENT_INIT), 0);
        let tail = h.effects(&[OP_MINFD, OP_MAXFD, OP_CONN_AT, OP_CLOSE_SESSION]);
        assert_eq!(
            tail[tail.len() - 3..],
            [OP_REACTOR_WAIT, OP_EVENT_FREE, OP_WORKER_STOP_CALLBACK]
        );

        let mut h = Script::with(&[(OP_HAS_REACTOR, 1)]);
        assert_eq!(run_op(&mut h, RUN_REACTOR_PROCESS_MAIN), SW_ERR);
        assert_eq!(h.called(OP_EVENT_FREE), 1);

        let mut h = Script::with(&[
            (OP_HAS_REACTOR, 1),
            (OP_ADD_MASTER_TIMER, 1),
            (OP_HEARTBEAT_INTERVAL, 5),
        ]);
        assert_eq!(run_op(&mut h, RUN_REACTOR_PROCESS_MAIN), SW_ERR);
        assert_eq!(h.called(OP_WORKER_START_CALLBACK), 1);
        assert_eq!(h.called(OP_REACTOR_WAIT), 0);
    }

    #[test]
    fn close_notifies_the_worker_once_the_socket_left_the_reactor() {
        let mut h = Script::with(&[(OP_EVENT_CONN_ACTIVE, 1), (OP_NOTIFY_CLOSE, 1)]);
        assert_eq!(run_op(&mut h, RUN_REACTOR_PROCESS_CLOSE), SW_OK);
        assert_eq!(h.called(OP_SET_PEER_CLOSED), 1);
        let mut h = Script::with(&[
            (OP_EVENT_CONN_ACTIVE, 1),
            (OP_CONN_CLOSE_QUEUED, 1),
            (OP_CLOSE_EVENT_SOCKET, 0),
        ]);
        assert_eq!(run_op(&mut h, RUN_REACTOR_PROCESS_CLOSE), SW_OK);
        assert_eq!(h.called(OP_SET_PEER_CLOSED), 0);
        let mut h = Script::with(&[(OP_EVENT_CONN_ACTIVE, 1), (OP_REACTOR_DEL_EVENT, u64::MAX)]);
        assert_eq!(run_op(&mut h, RUN_REACTOR_PROCESS_CLOSE), SW_ERR);
        let mut h = Script::default();
        assert_eq!(run_op(&mut h, RUN_REACTOR_PROCESS_CLOSE), SW_ERR);
        assert_eq!(h.called(OP_EVENT_SOCKET_REMOVED), 0);
    }

    #[test]
    fn heartbeat_closes_or_notifies_unhealthy_connections() {
        let mut h = Script::with(&[(OP_MINFD, 0), (OP_MAXFD, 2)])
            .queue(OP_CONN_AT, &[1, 1, 1])
            .queue(OP_CONN_HEALTHY, &[1, 0, 0])
            .queue(OP_CONN_SSL_PENDING, &[1, 0]);
        run_op(&mut h, RUN_REACTOR_PROCESS_TIMEOUT);
        assert_eq!(h.called(OP_CLOSE_CONNECTION), 1);
        assert_eq!(
            (h.called(OP_SET_CLOSE_FORCE), h.called(OP_CLOSE_NOTIFY)),
            (1, 1)
        );
    }
}
