//! Event workers (upstream's `src/server/worker.cc`; workers.cc's `WorkerContext`):
//! signals, the event switch, start/stop
//! callbacks, the async stop and its exit wait, and the process-mode main loop.
//! The context holds the server, the current worker and, per operation, the
//! received event, reactor or exit status.
use super::log::{
    LOG_BAD_EVENT, LOG_DISCARD, LOG_EXIT_TIMEOUT, LOG_KILL_BAD_ID, LOG_KILL_NO_ID, LOG_PUSH_STOP,
    LOG_UNPROCESSED,
};
use super::ops::{
    EVENT_BUFFER_EMPTY, EVENT_BUFFER_FULL, EVENT_CLOSE, EVENT_COMMAND_REQUEST, EVENT_CONNECT,
    EVENT_FINISH, EVENT_PIPE_MESSAGE, EVENT_RECV_DATA, EVENT_RECV_DGRAM, EVENT_SHUTDOWN,
    HOOK_BEFORE_SERVER_SHUTDOWN, HOOK_BEFORE_WORKER_START, HOOK_BEFORE_WORKER_STOP, OP_ABORT,
    OP_ABORT_WORKER, OP_ALL_CORE_WORKER_NUM, OP_ARM_EXIT, OP_BUS_CLEAR, OP_BUS_EMPTY, OP_BUS_POP,
    OP_BUS_READ, OP_CALL_WORKER_START, OP_CLEAR_LAST_ERROR, OP_CLEAR_TIMER, OP_CLOSE_CONNECTION,
    OP_CO_DISPATCH, OP_COMMAND_REQUEST, OP_CONN_REACTOR, OP_CONN_READABLE, OP_CONN_UNREAD,
    OP_COUNT_COROUTINES, OP_CPU_AFFINITY, OP_CURRENT_WORKER_ID, OP_DETACH_WORKER,
    OP_DISABLE_NOTIFY, OP_DISCARD_TIMEOUT_REQUEST, OP_DO_TASK, OP_DRAIN_PIPES,
    OP_DRAIN_WORKER_PIPES, OP_EVENT_AVAILABLE, OP_EVENT_BEGIN, OP_EVENT_END, OP_EVENT_FREE,
    OP_EVENT_INIT, OP_EXIT_TIME, OP_FACTORY_END, OP_FREE_CLIENT_CERT, OP_GLOBAL_HOOK,
    OP_HAS_CURRENT_WORKER, OP_HAS_ON_WORKER_EXIT, OP_HAS_REACTOR, OP_INFO_FD, OP_INFO_LEN,
    OP_IS_EVENT_WORKER, OP_LISTEN_PIPE, OP_MAX_WAIT_TIME, OP_MODE, OP_ON_BEFORE_SHUTDOWN,
    OP_ON_BUFFER_EMPTY, OP_ON_BUFFER_FULL, OP_ON_CONNECT, OP_ON_FINISH, OP_ON_PIPE_MESSAGE,
    OP_ON_WORKER_ERROR, OP_ON_WORKER_EXIT, OP_ON_WORKER_START, OP_ON_WORKER_STOP,
    OP_OTHER_PIPE_NONBLOCK, OP_PIPE_EVENT_WRITE, OP_PIPE_SEND_SYNC, OP_POOL_RUNNING, OP_PORT_COUNT,
    OP_PORT_DEL, OP_PROTECT_SESSIONS, OP_PUSH_STOP_MESSAGE, OP_REACTOR_DESTROYED, OP_REACTOR_ID,
    OP_REACTOR_IF_EXIT, OP_REACTOR_NUM, OP_REACTOR_STOP, OP_REACTOR_TIMEOUT, OP_REACTOR_WAIT,
    OP_REACTOR_WAIT_EXIT, OP_RECV_CONN, OP_RELOAD_ASYNC, OP_REOPEN_LOGGER, OP_RESET_WORKER_COUNTER,
    OP_ROOT_USER, OP_SEND_SHUTDOWN, OP_SET_ISOLATION, OP_SET_REACTOR_TIMEOUT, OP_SIGNAL_LIVE,
    OP_STOP_ASYNC, OP_STORE_CLIENT_CERT, OP_THREAD_SHUTDOWN, OP_UNLIMIT_PIPES,
    OP_UNREAD_PIPE_MASTER, OP_UNREAD_PIPE_WORKER, OP_UNREAD_THREAD_PIPE, OP_WORKER_ENTER,
    OP_WORKER_EXISTS, OP_WORKER_ID, OP_WORKER_NUM, OP_WORKER_SHUTDOWN, OP_WORKER_SIGNAL_INIT,
    OP_WORKER_START_CALLBACK, OP_WORKER_STATUS, OP_WORKER_STOP_CALLBACK, OP_WORKER_STOPPED,
    OP_WORKER_TYPE, RUN_ACCEPT_EVENT, RUN_CALL_WORKER_ERROR, RUN_CALL_WORKER_START,
    RUN_CALL_WORKER_STOP, RUN_CLEAN_CONNECTIONS, RUN_DRAIN_PIPES, RUN_KILL_WORKER,
    RUN_PIPE_RECEIVE, RUN_SEND_TO_REACTOR_THREAD, RUN_SIGNAL_INIT, RUN_START_EVENT_WORKER,
    RUN_STOP_ASYNC, RUN_TRY_EXIT, RUN_WORKER_SIGNAL, RUN_WORKER_START, STATUS_IDLE, TASK_PACKET,
    TASK_RECEIVE,
};
use super::{
    HANDLER_IGNORE, HANDLER_NONE, HANDLER_OWN, Host, MODE_BASE, MODE_PROCESS, MODE_THREAD,
    SIGNAL_RTMIN, SW_ERR, SW_OK, for_each_connection, set_signal,
};
use swoole_rs_corebase::CCast;

/// `swoole::WorkerType::SW_EVENT_WORKER`.
pub(crate) const EVENT_WORKER: u64 = 2;

pub(super) fn run<H: Host>(h: &mut H, operation: u32, arg: u64) -> i64 {
    match operation {
        RUN_SIGNAL_INIT => signal_init(h, HANDLER_IGNORE),
        RUN_WORKER_SIGNAL => worker_signal(h, i32::try_from(arg).unwrap_or(0)),
        RUN_ACCEPT_EVENT => accept_event(h),
        RUN_PIPE_RECEIVE => {
            if h.signed(OP_BUS_READ) > 0 {
                accept_event(h);
                h.get(OP_BUS_POP);
            }
        }
        RUN_WORKER_START => worker_start(h),
        RUN_CALL_WORKER_START => call_worker_start(h),
        RUN_CALL_WORKER_STOP => call_worker_stop(h),
        RUN_CALL_WORKER_ERROR => {
            h.get(OP_ON_WORKER_ERROR);
            if h.get(OP_WORKER_TYPE) == EVENT_WORKER {
                h.get(OP_ABORT_WORKER);
            }
        }
        RUN_KILL_WORKER => return i64::from(kill_worker(h, arg.cast_signed())),
        RUN_STOP_ASYNC => stop_async(h),
        RUN_TRY_EXIT => try_exit(h),
        RUN_DRAIN_PIPES => {
            if h.is(OP_HAS_REACTOR) {
                for worker in 0..h.get(OP_ALL_CORE_WORKER_NUM) {
                    h.op(OP_DRAIN_WORKER_PIPES, worker);
                }
            }
        }
        RUN_CLEAN_CONNECTIONS => clean_connections(h),
        RUN_START_EVENT_WORKER => return start_event_worker(h),
        // `Server::send_to_reactor_thread`: `arg` bytes of the event to the session's
        // reactor pipe, through the event loop when there is one.
        RUN_SEND_TO_REACTOR_THREAD => {
            let op = if h.is(OP_EVENT_AVAILABLE) {
                OP_PIPE_EVENT_WRITE
            } else {
                OP_PIPE_SEND_SYNC
            };
            return h.op(op, arg).cast_signed();
        }
        _ => return SW_ERR,
    }
    SW_OK
}

/// `Server::worker_signal_init`, and the task workers' (`sigpipe` ignored or not).
pub(crate) fn signal_init<H: Host>(h: &mut H, sigpipe: u64) {
    if h.get(OP_MODE) == MODE_THREAD {
        return;
    }
    set_signal(h, libc::SIGHUP, HANDLER_NONE);
    set_signal(h, libc::SIGPIPE, sigpipe);
    set_signal(h, libc::SIGUSR1, HANDLER_NONE);
    set_signal(h, libc::SIGUSR2, HANDLER_NONE);
    set_signal(h, libc::SIGTERM, HANDLER_OWN);
    set_signal(h, libc::SIGWINCH, HANDLER_OWN);
    set_signal(h, SIGNAL_RTMIN, HANDLER_OWN);
}

/// `Server::worker_signal_handler`.
fn worker_signal<H: Host>(h: &mut H, signo: i32) {
    if !h.is(OP_SIGNAL_LIVE) {
        return;
    }
    if signo == libc::SIGTERM {
        // An event worker stops asynchronously; a task worker just shuts down.
        if h.is(OP_EVENT_AVAILABLE) {
            h.get(OP_STOP_ASYNC);
        } else {
            h.get(OP_WORKER_SHUTDOWN);
        }
    } else if signo == libc::SIGWINCH || crate::is_sigrtmin(signo) {
        h.get(OP_REOPEN_LOGGER);
    }
}

/// `Server::worker_accept_event`: one event the worker received.
fn accept_event<H: Host>(h: &mut H) {
    let kind = h.get(OP_EVENT_BEGIN);
    match u32::try_from(kind).unwrap_or(u32::MAX) {
        EVENT_RECV_DATA => {
            // 0 no connection, 1 open, 2 closed by the server.
            let conn = h.get(OP_RECV_CONN);
            let discard = if conn == 0 {
                !h.is(OP_DISABLE_NOTIFY) || h.is(OP_DISCARD_TIMEOUT_REQUEST)
            } else {
                conn == 2
            };
            if discard {
                let (len, session) = (h.signed(OP_INFO_LEN), h.signed(OP_INFO_FD));
                h.log(LOG_DISCARD, len, session);
            } else {
                h.op(OP_DO_TASK, TASK_RECEIVE);
            }
        }
        EVENT_RECV_DGRAM => {
            h.op(OP_DO_TASK, TASK_PACKET);
        }
        EVENT_CLOSE => {
            h.get(OP_FREE_CLIENT_CERT);
            h.get(OP_FACTORY_END);
        }
        EVENT_CONNECT => {
            if h.get(OP_INFO_LEN) > 0 {
                h.get(OP_STORE_CLIENT_CERT);
            }
            h.get(OP_ON_CONNECT);
        }
        EVENT_BUFFER_FULL => {
            h.get(OP_ON_BUFFER_FULL);
        }
        EVENT_BUFFER_EMPTY => {
            h.get(OP_ON_BUFFER_EMPTY);
        }
        EVENT_FINISH => {
            h.get(OP_ON_FINISH);
        }
        EVENT_PIPE_MESSAGE => {
            h.get(OP_ON_PIPE_MESSAGE);
        }
        EVENT_COMMAND_REQUEST => {
            h.get(OP_COMMAND_REQUEST);
        }
        EVENT_SHUTDOWN => {
            h.get(OP_STOP_ASYNC);
        }
        _ => {
            h.log(LOG_BAD_EVENT, kind.cast_signed(), 0);
        }
    }
    // The maximum number of requests: the worker exits.
    if h.is(OP_EVENT_END) {
        if h.get(OP_MODE) == MODE_THREAD {
            h.get(OP_THREAD_SHUTDOWN);
        } else {
            h.get(OP_STOP_ASYNC);
        }
    }
}

/// `Server::worker_start_callback`.
fn worker_start<H: Host>(h: &mut H) {
    if h.is(OP_ROOT_USER) {
        h.get(OP_SET_ISOLATION);
    }
    let me = h.get(OP_WORKER_ID);
    for worker in 0..h.get(OP_ALL_CORE_WORKER_NUM) {
        if worker != me {
            h.op(OP_OTHER_PIPE_NONBLOCK, worker);
        }
    }
    h.op(OP_WORKER_STATUS, STATUS_IDLE);
    if h.get(OP_MODE) == MODE_PROCESS {
        h.get(OP_PROTECT_SESSIONS);
    }
    h.get(OP_CALL_WORKER_START);
}

/// `Server::call_worker_start_callback`.
fn call_worker_start<H: Host>(h: &mut H) {
    h.get(OP_CPU_AFFINITY);
    h.op(OP_GLOBAL_HOOK, HOOK_BEFORE_WORKER_START);
    h.get(OP_CLEAR_LAST_ERROR);
    h.get(OP_ON_WORKER_START);
}

/// `Server::call_worker_stop_callback`.
fn call_worker_stop<H: Host>(h: &mut H) {
    h.op(OP_GLOBAL_HOOK, HOOK_BEFORE_WORKER_STOP);
    h.get(OP_ON_WORKER_STOP);
    if !h.is(OP_BUS_EMPTY) {
        h.log(LOG_UNPROCESSED, 0, 0);
        h.get(OP_BUS_CLEAR);
    }
    h.get(OP_WORKER_STOPPED);
}

/// `Server::kill_worker(id)`: ask a worker (the current one when `id < 0`) to stop.
fn kill_worker<H: Host>(h: &mut H, id: i64) -> bool {
    if id < 0 && !h.is(OP_HAS_CURRENT_WORKER) {
        h.log(LOG_KILL_NO_ID, 0, 0);
        return false;
    }
    let id = if id < 0 {
        h.get(OP_CURRENT_WORKER_ID)
    } else {
        id.cast_unsigned()
    };
    if h.op(OP_WORKER_EXISTS, id) == 0 {
        h.log(LOG_KILL_BAD_ID, id.cast_signed(), 0);
        return false;
    }
    h.op(OP_SEND_SHUTDOWN, id).cast_signed() != -1
}

/// `Server::stop_async_worker`: stop reading, leave the pool and wait for the
/// reactor's remaining work before exiting.
fn stop_async<H: Host>(h: &mut H) {
    h.get(OP_WORKER_SHUTDOWN);
    if h.get(OP_WORKER_TYPE) == EVENT_WORKER {
        h.get(OP_RESET_WORKER_COUNTER);
    }
    // Forced termination.
    if !h.is(OP_RELOAD_ASYNC) {
        h.get(OP_REACTOR_STOP);
        return;
    }
    // The worker is already shutting down.
    if h.is(OP_REACTOR_WAIT_EXIT) {
        return;
    }
    // Separated from the event worker pool.
    h.get(OP_DETACH_WORKER);
    h.get(OP_UNREAD_PIPE_WORKER);
    match h.get(OP_MODE) {
        MODE_BASE => {
            if h.is(OP_IS_EVENT_WORKER) {
                if h.get(OP_WORKER_ID) == 0 && !h.is(OP_POOL_RUNNING) {
                    h.op(OP_GLOBAL_HOOK, HOOK_BEFORE_SERVER_SHUTDOWN);
                    h.get(OP_ON_BEFORE_SHUTDOWN);
                }
                h.get(OP_UNREAD_PIPE_MASTER);
                for port in 0..h.get(OP_PORT_COUNT) {
                    h.op(OP_PORT_DEL, port);
                }
                for_each_connection(h, |h| {
                    if h.is(OP_CONN_READABLE) {
                        h.get(OP_CONN_UNREAD);
                    }
                });
                h.get(OP_CLEAR_TIMER);
            }
        }
        MODE_PROCESS => {
            if h.signed(OP_PUSH_STOP_MESSAGE) < 0 {
                h.log(LOG_PUSH_STOP, 0, 0);
            }
        }
        MODE_THREAD => {
            if h.is(OP_IS_EVENT_WORKER) {
                // Thread mode forwards through the master pipes, which may be
                // watched for writes: stop reading them before the thread exits.
                let reactors = h.get(OP_REACTOR_NUM);
                let reactor = h.get(OP_REACTOR_ID);
                for worker in 0..h.get(OP_WORKER_NUM) {
                    if reactors != 0 && worker % reactors == reactor {
                        h.op(OP_UNREAD_THREAD_PIPE, worker);
                    }
                }
                // Only reads stop; sends may still be in flight until the reactor
                // is destroyed.
                for_each_connection(h, |h| {
                    if h.get(OP_CONN_REACTOR) == reactor && h.is(OP_CONN_READABLE) {
                        h.get(OP_CONN_UNREAD);
                    }
                });
            }
        }
        _ => {
            h.get(OP_ABORT);
        }
    }
    h.get(OP_ARM_EXIT);
    try_exit(h);
}

/// `Worker_reactor_try_to_exit`: once the reactor can exit, stop it; meanwhile
/// call `onWorkerExit` once, then wait at most `max_wait_time` since the stop.
fn try_exit<H: Host>(h: &mut H) {
    let mut called = false;
    loop {
        if h.is(OP_REACTOR_IF_EXIT) {
            h.get(OP_REACTOR_STOP);
        } else {
            if !called && h.is(OP_HAS_ON_WORKER_EXIT) {
                called = true;
                h.get(OP_ON_WORKER_EXIT);
                continue;
            }
            // Upstream's int arithmetic over time_t.
            let elapsed = now() - h.signed(OP_EXIT_TIME);
            let remaining = (h.signed(OP_MAX_WAIT_TIME) - elapsed).c_cast::<i32>();
            if remaining <= 0 {
                h.log(LOG_EXIT_TIMEOUT, 0, 0);
                h.get(OP_REACTOR_STOP);
            } else {
                let limit = remaining.wrapping_mul(1000);
                let timeout = h.signed(OP_REACTOR_TIMEOUT).c_cast::<i32>();
                if timeout < 0 || timeout > limit {
                    h.op(OP_SET_REACTOR_TIMEOUT, u64::from(limit.cast_unsigned()));
                }
            }
        }
        break;
    }
}

pub(crate) use swoole_rs_corebase::time_now as now;

/// `Server::clean_worker_connections`.
fn clean_connections<H: Host>(h: &mut H) {
    h.get(OP_REACTOR_DESTROYED);
    match h.get(OP_MODE) {
        MODE_BASE => for_each_connection(h, |h| {
            h.get(OP_CLOSE_CONNECTION);
        }),
        MODE_THREAD => {
            let me = h.get(OP_WORKER_ID);
            for_each_connection(h, |h| {
                if h.get(OP_CONN_REACTOR) == me {
                    h.get(OP_CLOSE_CONNECTION);
                }
            });
        }
        _ => {}
    }
}

/// `Server::start_event_worker`: the process-mode worker's main loop.
fn start_event_worker<H: Host>(h: &mut H) -> i64 {
    h.get(OP_WORKER_ENTER);
    if h.signed(OP_EVENT_INIT) < 0 {
        return SW_ERR;
    }
    h.get(OP_WORKER_SIGNAL_INIT);
    for worker in 0..h.get(OP_ALL_CORE_WORKER_NUM) {
        h.op(OP_UNLIMIT_PIPES, worker);
    }
    h.get(OP_LISTEN_PIPE);
    if h.is(OP_CO_DISPATCH) {
        h.get(OP_COUNT_COROUTINES);
    }
    h.get(OP_WORKER_START_CALLBACK);
    let rv = h.signed(OP_REACTOR_WAIT);
    h.get(OP_DRAIN_PIPES);
    h.get(OP_EVENT_FREE);
    h.get(OP_WORKER_STOP_CALLBACK);
    rv
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::workers::ops::{OP_CONN_AT, OP_MAXFD, OP_MINFD, OP_SIGNAL_SET};
    use crate::workers::script::Script;

    fn run_op(h: &mut Script, operation: u32, arg: u64) -> i64 {
        crate::workers::run(h, operation, arg)
    }

    #[test]
    fn send_to_reactor_thread_writes_through_the_loop_when_there_is_one() {
        for (available, write) in [(1, OP_PIPE_EVENT_WRITE), (0, OP_PIPE_SEND_SYNC)] {
            let mut h = Script::with(&[(OP_EVENT_AVAILABLE, available), (write, 48)]);
            assert_eq!(run_op(&mut h, RUN_SEND_TO_REACTOR_THREAD, 48), 48);
            assert_eq!(h.args(write), [48]);
            assert_eq!(h.log.len(), 2);
        }
    }

    #[test]
    fn recv_data_discards_for_closed_or_unknown_connections() {
        // (connection, disable_notify, discard_timeout_request) -> discarded
        for (conn, disable_notify, discard, discarded) in [
            (1, 0, 0, false),
            (2, 0, 0, true),
            (0, 0, 0, true),
            (0, 1, 0, false),
            (0, 1, 1, true),
        ] {
            let mut h = Script::with(&[
                (OP_EVENT_BEGIN, u64::from(EVENT_RECV_DATA)),
                (OP_RECV_CONN, conn),
                (OP_DISABLE_NOTIFY, disable_notify),
                (OP_DISCARD_TIMEOUT_REQUEST, discard),
            ]);
            run_op(&mut h, RUN_ACCEPT_EVENT, 0);
            assert_eq!(h.called(LOG_DISCARD) == 1, discarded);
            assert_eq!(
                h.args(OP_DO_TASK),
                if discarded {
                    vec![]
                } else {
                    vec![TASK_RECEIVE]
                }
            );
            assert_eq!((h.called(OP_EVENT_BEGIN), h.called(OP_EVENT_END)), (1, 1));
        }
    }

    #[test]
    fn each_event_reaches_its_handler_then_max_request_stops_the_worker() {
        for (event, op) in [
            (EVENT_RECV_DGRAM, OP_DO_TASK),
            (EVENT_CLOSE, OP_FACTORY_END),
            (EVENT_CONNECT, OP_ON_CONNECT),
            (EVENT_BUFFER_FULL, OP_ON_BUFFER_FULL),
            (EVENT_BUFFER_EMPTY, OP_ON_BUFFER_EMPTY),
            (EVENT_FINISH, OP_ON_FINISH),
            (EVENT_PIPE_MESSAGE, OP_ON_PIPE_MESSAGE),
            (EVENT_COMMAND_REQUEST, OP_COMMAND_REQUEST),
            (99, LOG_BAD_EVENT),
        ] {
            let mut h = Script::with(&[
                (OP_EVENT_BEGIN, u64::from(event)),
                (OP_EVENT_END, 1),
                (OP_MODE, MODE_PROCESS),
            ]);
            run_op(&mut h, RUN_ACCEPT_EVENT, 0);
            assert_eq!(h.called(op), 1, "event {event}");
            assert_eq!(h.called(OP_STOP_ASYNC), 1);
        }
        let mut h = Script::with(&[(OP_EVENT_END, 1), (OP_MODE, MODE_THREAD)]);
        run_op(&mut h, RUN_ACCEPT_EVENT, 0);
        assert_eq!(
            (h.called(OP_THREAD_SHUTDOWN), h.called(OP_STOP_ASYNC)),
            (1, 0)
        );
    }

    #[test]
    fn connect_stores_the_client_certificate_only_when_sent() {
        let mut h = Script::with(&[(OP_EVENT_BEGIN, u64::from(EVENT_CONNECT)), (OP_INFO_LEN, 5)]);
        run_op(&mut h, RUN_ACCEPT_EVENT, 0);
        assert_eq!(h.called(OP_STORE_CLIENT_CERT), 1);
        let mut h = Script::with(&[(OP_EVENT_BEGIN, u64::from(EVENT_CONNECT))]);
        run_op(&mut h, RUN_ACCEPT_EVENT, 0);
        assert_eq!(h.called(OP_STORE_CLIENT_CERT), 0);
    }

    #[test]
    fn signals_install_in_upstream_order() {
        let mut h = Script::with(&[(OP_MODE, MODE_PROCESS)]);
        run_op(&mut h, RUN_SIGNAL_INIT, HANDLER_IGNORE);
        let set: Vec<(i32, u64)> = h
            .args(OP_SIGNAL_SET)
            .iter()
            .map(|a| {
                (
                    u32::try_from(*a & 0xffff_ffff).map_or(0, u32::cast_signed),
                    a >> 32,
                )
            })
            .collect();
        assert_eq!(
            set,
            vec![
                (libc::SIGHUP, HANDLER_NONE),
                (libc::SIGPIPE, HANDLER_IGNORE),
                (libc::SIGUSR1, HANDLER_NONE),
                (libc::SIGUSR2, HANDLER_NONE),
                (libc::SIGTERM, HANDLER_OWN),
                (libc::SIGWINCH, HANDLER_OWN),
                (SIGNAL_RTMIN, HANDLER_OWN),
            ]
        );
        let mut h = Script::with(&[(OP_MODE, MODE_THREAD)]);
        run_op(&mut h, RUN_SIGNAL_INIT, HANDLER_IGNORE);
        assert_eq!(h.called(OP_SIGNAL_SET), 0);
    }

    #[test]
    fn worker_signal_stops_async_only_with_an_event_loop() {
        let mut h = Script::with(&[(OP_SIGNAL_LIVE, 1), (OP_EVENT_AVAILABLE, 1)]);
        run_op(&mut h, RUN_WORKER_SIGNAL, libc::SIGTERM as u64);
        assert_eq!(
            (h.called(OP_STOP_ASYNC), h.called(OP_WORKER_SHUTDOWN)),
            (1, 0)
        );
        let mut h = Script::with(&[(OP_SIGNAL_LIVE, 1)]);
        run_op(&mut h, RUN_WORKER_SIGNAL, libc::SIGTERM as u64);
        assert_eq!(
            (h.called(OP_STOP_ASYNC), h.called(OP_WORKER_SHUTDOWN)),
            (0, 1)
        );
        let mut h = Script::with(&[(OP_SIGNAL_LIVE, 1)]);
        run_op(&mut h, RUN_WORKER_SIGNAL, libc::SIGWINCH as u64);
        assert_eq!(h.called(OP_REOPEN_LOGGER), 1);
        let mut h = Script::default();
        run_op(&mut h, RUN_WORKER_SIGNAL, libc::SIGWINCH as u64);
        assert_eq!(h.called(OP_REOPEN_LOGGER), 0);
    }

    #[test]
    fn kill_worker_validates_the_id() {
        let mut h = Script::default();
        assert_eq!(run_op(&mut h, RUN_KILL_WORKER, u64::MAX), 0);
        assert_eq!(h.called(LOG_KILL_NO_ID), 1);
        let mut h = Script::with(&[(OP_HAS_CURRENT_WORKER, 1), (OP_CURRENT_WORKER_ID, 3)]);
        assert_eq!(run_op(&mut h, RUN_KILL_WORKER, u64::MAX), 0);
        assert_eq!(h.args(LOG_KILL_BAD_ID), vec![3]);
        let mut h = Script::with(&[(OP_WORKER_EXISTS, 1), (OP_SEND_SHUTDOWN, 16)]);
        assert_eq!(run_op(&mut h, RUN_KILL_WORKER, 2), 1);
        assert_eq!(h.args(OP_SEND_SHUTDOWN), vec![2]);
        let mut h = Script::with(&[(OP_WORKER_EXISTS, 1), (OP_SEND_SHUTDOWN, u64::MAX)]);
        assert_eq!(run_op(&mut h, RUN_KILL_WORKER, 2), 0);
    }

    #[test]
    fn stop_async_without_reload_async_stops_the_reactor() {
        let mut h = Script::with(&[(OP_WORKER_TYPE, EVENT_WORKER)]);
        run_op(&mut h, RUN_STOP_ASYNC, 0);
        assert_eq!(
            h.effects(&[OP_WORKER_TYPE, OP_RELOAD_ASYNC]),
            vec![OP_WORKER_SHUTDOWN, OP_RESET_WORKER_COUNTER, OP_REACTOR_STOP]
        );
    }

    #[test]
    fn stop_async_in_base_mode_stops_reading_then_waits() {
        let mut h = Script::with(&[
            (OP_RELOAD_ASYNC, 1),
            (OP_MODE, MODE_BASE),
            (OP_IS_EVENT_WORKER, 1),
            (OP_PORT_COUNT, 2),
            (OP_MINFD, 3),
            (OP_MAXFD, 5),
            (OP_REACTOR_IF_EXIT, 1),
        ])
        .queue(OP_CONN_AT, &[1, 0, 1])
        .queue(OP_CONN_READABLE, &[1, 0]);
        run_op(&mut h, RUN_STOP_ASYNC, 0);
        assert_eq!(h.args(OP_GLOBAL_HOOK), vec![HOOK_BEFORE_SERVER_SHUTDOWN]);
        assert_eq!(h.called(OP_ON_BEFORE_SHUTDOWN), 1);
        assert_eq!(h.args(OP_PORT_DEL), vec![0, 1]);
        assert_eq!(h.called(OP_CONN_UNREAD), 1);
        assert_eq!(h.called(OP_CLEAR_TIMER), 1);
        assert_eq!((h.called(OP_ARM_EXIT), h.called(OP_REACTOR_STOP)), (1, 1));
    }

    #[test]
    fn stop_async_in_thread_mode_unreads_this_reactors_pipes() {
        let mut h = Script::with(&[
            (OP_RELOAD_ASYNC, 1),
            (OP_MODE, MODE_THREAD),
            (OP_IS_EVENT_WORKER, 1),
            (OP_REACTOR_NUM, 2),
            (OP_REACTOR_ID, 1),
            (OP_WORKER_NUM, 5),
            (OP_MINFD, 1),
            (OP_MAXFD, 0),
            (OP_REACTOR_IF_EXIT, 1),
        ]);
        run_op(&mut h, RUN_STOP_ASYNC, 0);
        assert_eq!(h.args(OP_UNREAD_THREAD_PIPE), vec![1, 3]);
    }

    #[test]
    fn try_exit_calls_on_worker_exit_once_then_bounds_the_wait() {
        let mut h = Script::with(&[
            (OP_HAS_ON_WORKER_EXIT, 1),
            (OP_EXIT_TIME, now().cast_unsigned()),
            (OP_MAX_WAIT_TIME, 3),
            (OP_REACTOR_TIMEOUT, u64::MAX),
        ]);
        run_op(&mut h, RUN_TRY_EXIT, 0);
        assert_eq!(h.called(OP_ON_WORKER_EXIT), 1);
        assert_eq!(h.called(OP_REACTOR_IF_EXIT), 2);
        let timeout = h.args(OP_SET_REACTOR_TIMEOUT);
        assert!(
            timeout == vec![3000] || timeout == vec![2000],
            "{timeout:?}"
        );
        let mut h = Script::with(&[
            (OP_EXIT_TIME, (now() - 10).cast_unsigned()),
            (OP_MAX_WAIT_TIME, 3),
        ]);
        run_op(&mut h, RUN_TRY_EXIT, 0);
        assert_eq!(
            (h.called(LOG_EXIT_TIMEOUT), h.called(OP_REACTOR_STOP)),
            (1, 1)
        );
        let mut h = Script::with(&[
            (OP_EXIT_TIME, now().cast_unsigned()),
            (OP_MAX_WAIT_TIME, 30),
            (OP_REACTOR_TIMEOUT, 500),
        ]);
        run_op(&mut h, RUN_TRY_EXIT, 0);
        assert_eq!(h.called(OP_SET_REACTOR_TIMEOUT), 0);
    }

    #[test]
    fn start_event_worker_runs_the_loop_between_callbacks() {
        let mut h = Script::with(&[(OP_ALL_CORE_WORKER_NUM, 2), (OP_REACTOR_WAIT, 7)]);
        assert_eq!(run_op(&mut h, RUN_START_EVENT_WORKER, 0), 7);
        assert_eq!(
            h.effects(&[OP_ALL_CORE_WORKER_NUM, OP_CO_DISPATCH]),
            vec![
                OP_WORKER_ENTER,
                OP_EVENT_INIT,
                OP_WORKER_SIGNAL_INIT,
                OP_UNLIMIT_PIPES,
                OP_UNLIMIT_PIPES,
                OP_LISTEN_PIPE,
                OP_WORKER_START_CALLBACK,
                OP_REACTOR_WAIT,
                OP_DRAIN_PIPES,
                OP_EVENT_FREE,
                OP_WORKER_STOP_CALLBACK,
            ]
        );
        let mut h = Script::with(&[(OP_EVENT_INIT, u64::MAX)]);
        assert_eq!(run_op(&mut h, RUN_START_EVENT_WORKER, 0), SW_ERR);
    }

    #[test]
    fn worker_start_skips_its_own_pipe() {
        let mut h = Script::with(&[
            (OP_WORKER_ID, 1),
            (OP_ALL_CORE_WORKER_NUM, 3),
            (OP_MODE, MODE_PROCESS),
        ]);
        run_op(&mut h, RUN_WORKER_START, 0);
        assert_eq!(h.args(OP_OTHER_PIPE_NONBLOCK), vec![0, 2]);
        assert_eq!(h.called(OP_PROTECT_SESSIONS), 1);
        assert_eq!(h.called(OP_SET_ISOLATION), 0);
    }

    #[test]
    fn clean_connections_in_thread_mode_closes_only_its_own() {
        let mut h = Script::with(&[
            (OP_MODE, MODE_THREAD),
            (OP_WORKER_ID, 1),
            (OP_MINFD, 0),
            (OP_MAXFD, 1),
        ])
        .queue(OP_CONN_AT, &[1, 1])
        .queue(OP_CONN_REACTOR, &[1, 0]);
        run_op(&mut h, RUN_CLEAN_CONNECTIONS, 0);
        assert_eq!(h.called(OP_CLOSE_CONNECTION), 1);
    }
}
