//! Reactor threads: the receive path and dispatch to workers, the worker pipe
//! protocol, TLS readiness, heartbeats and the threads' lifecycle.
use crate::ops::{
    EVENT_CLOSE_FORCE, EVENT_CLOSE_FORWARD, EVENT_COMMAND_REQUEST, EVENT_COMMAND_RESPONSE,
    EVENT_FINISH, EVENT_INCOMING, EVENT_PIPE_MESSAGE, EVENT_SHUTDOWN, EVENT_SHUTDOWN_SIGNAL,
    FIELD_HEARTBEAT_INTERVAL, LOG_NO_SESSION, LOG_RECVFROM, OP_CLIENT_CERT, OP_CLOSE,
    OP_CLOSE_FORWARD, OP_COMMAND_REQUEST, OP_COMMAND_RESPONSE, OP_CONN_AT, OP_CONN_FD,
    OP_CONN_FORCE, OP_CONN_HANDSHAKING, OP_CONN_HEALTHY, OP_CONN_REACTOR, OP_CONN_READING,
    OP_CONN_SESSION, OP_CONN_UNREAD, OP_COUNT_DISPATCH, OP_DELAY_RECEIVE, OP_DESTROY_CALLBACK,
    OP_DGRAM_BUFFER, OP_DISABLE_NOTIFY, OP_DISPATCH_CERT, OP_DTLS_ACCEPT, OP_ERRNO, OP_EVENT_FREE,
    OP_EVENT_INIT, OP_EVENT_WAIT, OP_FACTORY_DISPATCH, OP_FIELD, OP_FORCE_CLOSE, OP_HAS_ON_CONNECT,
    OP_HAVE_DGRAM, OP_HEARTBEAT_THREAD, OP_HEARTBEAT_TIMER, OP_HEARTBEAT_TIMER_CLEAR, OP_INCOMING,
    OP_JOIN_HEARTBEAT, OP_KILL_SELF, OP_MASTER_REACTOR_ID, OP_MAXFD, OP_MICROTIME, OP_MINFD,
    OP_MODE, OP_NOTIFY_CONNECT, OP_ON_FINISH, OP_ON_PIPE_MESSAGE, OP_PACKET_DISPATCH,
    OP_PACKET_INIT, OP_PEER_CERT, OP_PIPE_FD, OP_PIPE_POP, OP_PIPE_READ, OP_PIPE_SEND,
    OP_PIPE_TYPE, OP_PORT_COUNT, OP_PORT_DEL, OP_PORT_DGRAM, OP_PORT_DTLS, OP_PORT_FD,
    OP_PORT_LISTEN, OP_PORT_READ, OP_PORT_REMOVED, OP_QUEUE_FULL, OP_QUEUE_SUB, OP_REACTOR_BARRIER,
    OP_REACTOR_CLOSE, OP_REACTOR_DEL, OP_REACTOR_ID, OP_REACTOR_NUM, OP_REACTOR_THREAD_TYPE,
    OP_RECVFROM, OP_RESUME_READ, OP_RESUME_TIMER, OP_RUNNING, OP_SEND_CLOSE_FORCE, OP_SESSION_CONN,
    OP_SESSION_HANDLERS, OP_SET_WAITING_TIME, OP_SINGLE_THREAD, OP_SPAWN_REACTOR_THREAD,
    OP_SSL_ACCEPT, OP_SSL_READY, OP_SSL_VERIFY, OP_START_MASTER_THREAD, OP_STOP_THREAD_WORKER,
    OP_STORE_LISTEN_SOCKET, OP_TASK_INIT, OP_THREAD_CLEAN, OP_THREAD_ENTER, OP_THREAD_INIT,
    OP_THREAD_JOIN, OP_THREAD_JOINABLE, OP_THREAD_NOTIFY, OP_THREAD0_INIT, OP_VERIFY_PEER,
    OP_WAIT_EXIT, OP_WAITING_TIME, OP_WORKER_NUM, OP_WORKER_PIPE_UNREAD, OP_WORKER_START,
    OP_WORKER_STOP,
};
use crate::{Host, MODE_PROCESS, MODE_THREAD, SW_ERR, SW_OK};

/// `swReturnCode` values.
pub(crate) const SW_CONTINUE: i64 = 1;
pub(crate) const SW_WAIT: i64 = 2;
pub(crate) const SW_ERROR: i64 = 4;
pub(crate) const SW_READY: i64 = 5;
/// The longest resume back-off, in milliseconds.
const MAX_WAITING_TIME: u64 = 1024;

/// `ReactorThread_verify_ssl_state` once the socket's TLS handshake is pending:
/// finish it, then announce the connection (with the client certificate when the
/// port asks for one).
pub(crate) fn verify_ssl<H: Host>(h: &mut H) -> i64 {
    let code = h.signed(OP_SSL_ACCEPT);
    if code != SW_READY {
        return code;
    }
    h.get(OP_SSL_READY);
    let mut announced = false;
    if h.is(OP_CLIENT_CERT) {
        if h.is(OP_PEER_CERT) {
            if h.is(OP_VERIFY_PEER) && !h.is(OP_SSL_VERIFY) {
                return SW_ERROR;
            }
            h.get(OP_DISPATCH_CERT);
            announced = true;
        } else if h.is(OP_VERIFY_PEER) {
            return SW_ERROR;
        }
    }
    if !announced && h.is(OP_HAS_ON_CONNECT) {
        h.get(OP_NOTIFY_CONNECT);
    }
    if h.is(OP_DELAY_RECEIVE) && h.signed(OP_REACTOR_DEL) < 0 {
        return SW_ERROR;
    }
    SW_READY
}

/// `ReactorThread_onPacketReceived`: dispatch datagrams until the socket would block.
pub(crate) fn packet_received<H: Host>(h: &mut H) -> i64 {
    h.get(OP_PACKET_INIT);
    loop {
        let n = h.signed(OP_RECVFROM);
        if n <= 0 {
            if h.get(OP_ERRNO) == u64::from(libc::EAGAIN.cast_unsigned()) {
                return SW_OK;
            }
            h.get(LOG_RECVFROM);
            return SW_ERR;
        }
        let length = n.cast_unsigned();
        if h.is(OP_PORT_DTLS) {
            return h.op(OP_DTLS_ACCEPT, length).cast_signed();
        }
        if h.op(OP_PACKET_DISPATCH, length) == 0 {
            return SW_ERR;
        }
    }
}

fn reactor_owns<H: Host>(h: &mut H, fd: u64) -> bool {
    let reactors = h.get(OP_REACTOR_NUM);
    reactors != 0
        && fd
            .cast_signed()
            .rem_euclid(reactors.cast_signed())
            .cast_unsigned()
            == h.get(OP_REACTOR_ID)
}

/// `Server::foreach_connection`: each valid connection from `min_fd` to `max_fd`
/// (read afresh each step), selected in turn.
fn for_each_connection<H: Host>(h: &mut H, mut each: impl FnMut(&mut H)) {
    let mut fd = h.signed(OP_MINFD);
    while fd <= h.signed(OP_MAXFD) {
        if h.op(OP_CONN_AT, fd.cast_unsigned()) != 0 {
            each(h);
        }
        fd += 1;
    }
}

/// `ReactorThread::shutdown`: stop reading datagrams, worker pipes and connections
/// owned by this reactor, and let it exit once its writes drain.
pub(crate) fn shutdown<H: Host>(h: &mut H) {
    if h.is(OP_HAVE_DGRAM) {
        for port in 0..h.get(OP_PORT_COUNT) {
            if h.op(OP_PORT_DGRAM, port) == 0 {
                continue;
            }
            let fd = h.op(OP_PORT_FD, port);
            if reactor_owns(h, fd) && h.op(OP_PORT_REMOVED, port) == 0 {
                h.op(OP_PORT_DEL, port);
            }
        }
    }
    h.get(OP_HEARTBEAT_TIMER_CLEAR);
    h.get(OP_DESTROY_CALLBACK);
    if h.get(OP_MODE) == MODE_THREAD {
        h.get(OP_STOP_THREAD_WORKER);
        return;
    }
    for worker in 0..h.get(OP_WORKER_NUM) {
        if reactor_owns(h, worker) {
            h.op(OP_WORKER_PIPE_UNREAD, worker);
        }
    }
    for_each_connection(h, |h| {
        let fd = h.get(OP_CONN_FD);
        if reactor_owns(h, fd) && h.is(OP_CONN_READING) {
            h.get(OP_CONN_UNREAD);
        }
    });
    h.get(OP_WAIT_EXIT);
}

/// `ReactorThread::close_connection`: close now when nobody is told, else notify
/// the worker through the close event.
pub(crate) fn close<H: Host>(h: &mut H, session: u64) -> i64 {
    if h.op(OP_SESSION_CONN, session) == 0 {
        h.op(LOG_NO_SESSION, session);
        return SW_OK;
    }
    if h.is(OP_DISABLE_NOTIFY) || h.is(OP_CONN_FORCE) {
        return h.signed(OP_CLOSE);
    }
    // A TLS connection that never finished its handshake has nobody to notify.
    if h.is(OP_CONN_HANDSHAKING) {
        return h.signed(OP_CLOSE);
    }
    h.get(OP_FORCE_CLOSE);
    SW_OK
}

/// `ReactorThread_onPipeRead`: the messages workers send their reactor thread.
pub(crate) fn pipe_read<H: Host>(h: &mut H) -> i64 {
    loop {
        if h.signed(OP_PIPE_READ) <= 0 {
            return SW_OK;
        }
        let kind = u32::try_from(h.get(OP_PIPE_TYPE)).unwrap_or(u32::MAX);
        match kind {
            EVENT_INCOMING => {
                let session = h.get(OP_PIPE_FD);
                if h.op(OP_SESSION_CONN, session) != 0 && h.signed(OP_INCOMING) < 0 {
                    h.get(OP_REACTOR_CLOSE);
                }
            }
            EVENT_COMMAND_REQUEST => {
                h.get(OP_COMMAND_REQUEST);
            }
            EVENT_COMMAND_RESPONSE => {
                h.get(OP_COMMAND_RESPONSE);
            }
            EVENT_SHUTDOWN => shutdown(h),
            EVENT_SHUTDOWN_SIGNAL => {
                h.get(OP_KILL_SELF);
            }
            EVENT_FINISH => {
                h.get(OP_ON_FINISH);
            }
            EVENT_PIPE_MESSAGE => {
                h.get(OP_ON_PIPE_MESSAGE);
            }
            EVENT_CLOSE_FORCE => {
                let session = h.get(OP_PIPE_FD);
                close(h, session);
            }
            EVENT_CLOSE_FORWARD => {
                let session = h.get(OP_PIPE_FD);
                h.op(OP_CLOSE_FORWARD, session);
            }
            _ => {
                h.get(OP_PIPE_SEND);
            }
        }
        h.get(OP_PIPE_POP);
    }
}

/// `Server::init_reactor`.
pub(crate) fn init_reactor<H: Host>(h: &mut H) {
    // 64K datagrams need the larger thread buffer.
    if h.is(OP_HAVE_DGRAM) {
        h.get(OP_DGRAM_BUFFER);
    }
    h.get(OP_SESSION_HANDLERS);
}

/// `Server::heartbeat_check`: close this reactor's connections idle past their port's
/// heartbeat time.
pub(crate) fn heartbeat_check<H: Host>(h: &mut H) {
    let now = h.get(OP_MICROTIME);
    let reactor = h.get(OP_REACTOR_ID);
    for_each_connection(h, |h| {
        let session = h.signed(OP_CONN_SESSION);
        if session > 0 && h.get(OP_CONN_REACTOR) == reactor && !h.is_op(OP_CONN_HEALTHY, now) {
            close(h, session.cast_unsigned());
        }
    });
}

/// `Server::start_reactor_threads`: listen, start the reactor threads (or set up the
/// single one) and the heartbeat, then run the master thread.
pub(crate) fn start_reactor_threads<H: Host>(h: &mut H) -> i64 {
    if h.signed(OP_EVENT_INIT) < 0 {
        return SW_ERR;
    }
    for port in 0..h.get(OP_PORT_COUNT) {
        if h.op(OP_PORT_DGRAM, port) != 0 {
            continue;
        }
        if h.op(OP_PORT_LISTEN, port).cast_signed() < 0 {
            h.get(OP_EVENT_FREE);
            return SW_ERR;
        }
        h.op(OP_PORT_READ, port);
    }
    h.get(OP_STORE_LISTEN_SOCKET);
    let single = h.is(OP_SINGLE_THREAD);
    if single {
        h.get(OP_THREAD0_INIT);
    } else {
        h.get(OP_MASTER_REACTOR_ID);
        for thread in 0..h.get(OP_REACTOR_NUM) {
            h.op(OP_SPAWN_REACTOR_THREAD, thread);
        }
    }
    if h.op(OP_FIELD, u64::from(FIELD_HEARTBEAT_INTERVAL)) >= 1 {
        h.get(if single {
            OP_HEARTBEAT_TIMER
        } else {
            OP_HEARTBEAT_THREAD
        });
    }
    h.signed(OP_START_MASTER_THREAD)
}

/// `Server::reactor_thread_main_loop`.
pub(crate) fn main_loop<H: Host>(h: &mut H, thread: u64) {
    h.op(OP_THREAD_ENTER, thread);
    if h.signed(OP_EVENT_INIT) < 0 {
        return;
    }
    let mode = h.get(OP_MODE);
    if mode == MODE_THREAD {
        h.get(OP_WORKER_START);
    } else {
        h.op(OP_REACTOR_THREAD_TYPE, thread);
    }
    if h.signed(OP_THREAD_INIT) < 0 {
        return;
    }
    // Every reactor thread is ready before the master runs.
    if mode == MODE_PROCESS {
        h.get(OP_REACTOR_BARRIER);
    }
    h.get(OP_EVENT_WAIT);
    if mode == MODE_THREAD {
        h.get(OP_WORKER_STOP);
    }
    h.get(OP_THREAD_CLEAN);
}

/// `ReactorThread_resume_data_receiving`: back off (doubling up to 1024ms) while the
/// workers still hold too many bytes, then read again.
pub(crate) fn resume_receiving<H: Host>(h: &mut H) {
    if h.is(OP_QUEUE_FULL) {
        let waiting = h.get(OP_WAITING_TIME);
        if waiting != MAX_WAITING_TIME {
            h.op(OP_SET_WAITING_TIME, waiting * 2);
        }
        if h.is(OP_RESUME_TIMER) {
            return;
        }
    }
    h.get(OP_RESUME_READ);
}

/// `Server::dispatch_task`: one received data frame to a worker, counting the bytes
/// queued for it until the worker takes them.
pub(crate) fn dispatch_task<H: Host>(h: &mut H, len: u64) -> i64 {
    h.op(OP_TASK_INIT, len);
    if !h.is(OP_FACTORY_DISPATCH) {
        if len > 0 {
            h.op(OP_QUEUE_SUB, len);
        }
        return SW_ERR;
    }
    h.get(OP_COUNT_DISPATCH);
    SW_OK
}

/// `Server::join_reactor_thread`: tell each running reactor thread to shut down and join it.
pub(crate) fn join_reactor_threads<H: Host>(h: &mut H) {
    if h.is(OP_SINGLE_THREAD) {
        return;
    }
    if h.op(OP_FIELD, u64::from(FIELD_HEARTBEAT_INTERVAL)) > 0 {
        h.get(OP_JOIN_HEARTBEAT);
    }
    for thread in 0..h.get(OP_REACTOR_NUM) {
        if h.op(OP_THREAD_JOINABLE, thread) == 0 {
            continue;
        }
        h.op(OP_THREAD_NOTIFY, thread);
        h.op(OP_THREAD_JOIN, thread);
    }
}

/// The heartbeat thread: every interval, ask reactor threads to force-close
/// connections idle past their port's heartbeat time.
pub(crate) fn heartbeat_thread<H: Host>(h: &mut H) {
    while h.is(OP_RUNNING) {
        let now = h.get(OP_MICROTIME);
        for_each_connection(h, |h| {
            let session = h.signed(OP_CONN_SESSION);
            if session > 0 && !h.is_op(OP_CONN_HEALTHY, now) {
                h.op(OP_SEND_CLOSE_FORCE, session.cast_unsigned());
            }
        });
        let interval = h.op(OP_FIELD, u64::from(FIELD_HEARTBEAT_INTERVAL));
        // SAFETY: `sleep` has no memory preconditions.
        unsafe { libc::sleep(u32::try_from(interval).unwrap_or(u32::MAX)) };
    }
}

/// The reactor-thread operations of `swr_v1_server_run`.
pub(crate) fn run<H: Host>(h: &mut H, operation: u32, arg: u64) -> i64 {
    use crate::ops::{
        RUN_DISPATCH_TASK, RUN_HEARTBEAT_CHECK, RUN_HEARTBEAT_THREAD, RUN_INIT_REACTOR,
        RUN_JOIN_REACTOR_THREADS, RUN_PACKET_RECEIVED, RUN_PIPE_READ, RUN_REACTOR_THREAD_MAIN,
        RUN_RESUME_RECEIVING, RUN_START_REACTOR_THREADS, RUN_THREAD_CLOSE, RUN_THREAD_SHUTDOWN,
    };
    match operation {
        RUN_PACKET_RECEIVED => packet_received(h),
        RUN_THREAD_SHUTDOWN => {
            shutdown(h);
            SW_OK
        }
        RUN_THREAD_CLOSE => close(h, arg),
        RUN_PIPE_READ => pipe_read(h),
        RUN_INIT_REACTOR => {
            init_reactor(h);
            SW_OK
        }
        RUN_HEARTBEAT_CHECK => {
            heartbeat_check(h);
            SW_OK
        }
        RUN_START_REACTOR_THREADS => start_reactor_threads(h),
        RUN_REACTOR_THREAD_MAIN => {
            main_loop(h, arg);
            SW_OK
        }
        RUN_RESUME_RECEIVING => {
            resume_receiving(h);
            SW_OK
        }
        RUN_DISPATCH_TASK => dispatch_task(h, arg),
        RUN_JOIN_REACTOR_THREADS => {
            join_reactor_threads(h);
            SW_OK
        }
        RUN_HEARTBEAT_THREAD => {
            heartbeat_thread(h);
            SW_OK
        }
        _ => crate::dispatch::run(h, operation, arg),
    }
}

trait IsOp {
    fn is_op(&mut self, op: u32, arg: u64) -> bool;
}

impl<H: Host> IsOp for H {
    fn is_op(&mut self, op: u32, arg: u64) -> bool {
        self.op(op, arg) != 0
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::script::Script;

    #[test]
    fn ssl_states() {
        let mut s = Script::with(&[(OP_SSL_ACCEPT, 2)]);
        assert_eq!(verify_ssl(&mut s), 2, "handshake waits");

        let mut s = Script::with(&[
            (OP_SSL_ACCEPT, 5),
            (OP_HAS_ON_CONNECT, 1),
            (OP_DELAY_RECEIVE, 1),
        ]);
        assert_eq!(verify_ssl(&mut s), SW_READY, "ready");
        assert_eq!(s.called(OP_NOTIFY_CONNECT), 1, "onConnect");
        assert_eq!(s.called(OP_REACTOR_DEL), 1, "delayed receive");

        let cert = |peer, verify, ok| {
            let mut s = Script::with(&[
                (OP_SSL_ACCEPT, 5),
                (OP_CLIENT_CERT, 1),
                (OP_PEER_CERT, peer),
                (OP_VERIFY_PEER, verify),
                (OP_SSL_VERIFY, ok),
                (OP_HAS_ON_CONNECT, 1),
            ]);
            let code = verify_ssl(&mut s);
            (
                code,
                s.called(OP_DISPATCH_CERT),
                s.called(OP_NOTIFY_CONNECT),
            )
        };
        assert_eq!(cert(0, 1, 0), (SW_ERROR, 0, 0), "missing certificate");
        assert_eq!(cert(0, 0, 0), (SW_READY, 0, 1), "optional certificate");
        assert_eq!(cert(1, 1, 0), (SW_ERROR, 0, 0), "rejected certificate");
        assert_eq!(cert(1, 1, 1), (SW_READY, 1, 0), "verified certificate");
        assert_eq!(cert(1, 0, 0), (SW_READY, 1, 0), "unverified certificate");
    }

    #[test]
    fn datagrams_until_eagain() {
        let mut s = Script::with(&[
            (OP_ERRNO, libc::EAGAIN.cast_unsigned().into()),
            (OP_PACKET_DISPATCH, 1),
        ])
        .queue(OP_RECVFROM, &[10, 20, u64::MAX]);
        assert_eq!(packet_received(&mut s), SW_OK, "drained");
        assert_eq!(s.args(OP_PACKET_DISPATCH), vec![10, 20], "each datagram");

        let mut s = Script::with(&[(OP_ERRNO, 5)]).queue(OP_RECVFROM, &[u64::MAX]);
        assert_eq!(packet_received(&mut s), SW_ERR, "recvfrom failed");
        assert_eq!(s.called(LOG_RECVFROM), 1, "warned");

        let mut s = Script::with(&[(OP_PACKET_DISPATCH, 0), (OP_RECVFROM, 3)]);
        assert_eq!(packet_received(&mut s), SW_ERR, "dispatch failed");
    }

    #[test]
    fn close_paths() {
        let mut s = Script::with(&[]);
        assert_eq!(close(&mut s, 9), SW_OK, "missing session");
        assert_eq!(s.args(LOG_NO_SESSION), vec![9], "traced");

        let mut s = Script::with(&[
            (OP_SESSION_CONN, 1),
            (OP_CONN_HANDSHAKING, 1),
            (OP_CLOSE, 0),
        ]);
        close(&mut s, 1);
        assert_eq!(s.called(OP_CLOSE), 1, "handshaking closes directly");

        let mut s = Script::with(&[(OP_SESSION_CONN, 1)]);
        close(&mut s, 1);
        assert_eq!(s.called(OP_FORCE_CLOSE), 1, "worker notified");
    }

    #[test]
    fn pipe_messages() {
        let mut s = Script::with(&[
            (OP_PIPE_FD, 7),
            (OP_SESSION_CONN, 1),
            (OP_INCOMING, u64::MAX),
        ])
        .queue(OP_PIPE_READ, &[1, 1, 1, 1, 0])
        .queue(
            OP_PIPE_TYPE,
            &[
                EVENT_INCOMING.into(),
                EVENT_CLOSE_FORWARD.into(),
                EVENT_SHUTDOWN_SIGNAL.into(),
                2,
            ],
        );
        assert_eq!(pipe_read(&mut s), SW_OK, "drained");
        assert_eq!(s.called(OP_REACTOR_CLOSE), 1, "incoming failed");
        assert_eq!(s.args(OP_CLOSE_FORWARD), vec![7], "forwarded close");
        assert_eq!(s.called(OP_KILL_SELF), 1, "shutdown signal");
        assert_eq!(s.called(OP_PIPE_SEND), 1, "data for the connection");
        assert_eq!(s.called(OP_PIPE_POP), 4, "each message popped");
    }

    #[test]
    fn shutdown_owned_resources() {
        let mut s = Script::with(&[
            (OP_HAVE_DGRAM, 1),
            (OP_PORT_COUNT, 2),
            (OP_PORT_DGRAM, 1),
            (OP_REACTOR_NUM, 2),
            (OP_REACTOR_ID, 1),
            (OP_MODE, MODE_PROCESS),
            (OP_WORKER_NUM, 4),
            (OP_MINFD, 3),
            (OP_MAXFD, 6),
            (OP_CONN_AT, 1),
            (OP_CONN_READING, 1),
        ])
        .queue(OP_PORT_FD, &[4, 5])
        .queue(OP_CONN_FD, &[3, 4, 5, 6]);
        shutdown(&mut s);
        assert_eq!(s.args(OP_PORT_DEL), vec![1], "odd fd belongs to reactor 1");
        assert_eq!(s.args(OP_WORKER_PIPE_UNREAD), vec![1, 3], "odd workers");
        assert_eq!(s.called(OP_CONN_UNREAD), 2, "odd connections");
        assert_eq!(s.called(OP_WAIT_EXIT), 1, "waits for writes");
    }

    #[test]
    fn heartbeat_closes_stale_sessions_of_this_reactor() {
        let mut s = Script::with(&[
            (OP_MINFD, 1),
            (OP_MAXFD, 3),
            (OP_CONN_AT, 1),
            (OP_REACTOR_ID, 0),
            (OP_SESSION_CONN, 1),
        ])
        .queue(OP_CONN_SESSION, &[0, 11, 12])
        .queue(OP_CONN_REACTOR, &[0, 1])
        .queue(OP_CONN_HEALTHY, &[0]);
        heartbeat_check(&mut s);
        assert_eq!(
            s.args(OP_SESSION_CONN),
            vec![11],
            "only the stale session here"
        );
    }

    #[test]
    fn start_and_join_threads() {
        let mut s = Script::with(&[(OP_PORT_COUNT, 2), (OP_REACTOR_NUM, 3), (OP_FIELD, 5)])
            .queue(OP_PORT_DGRAM, &[1, 0]);
        assert_eq!(start_reactor_threads(&mut s), SW_OK, "master ran");
        assert_eq!(s.args(OP_PORT_LISTEN), vec![1], "stream port listens");
        assert_eq!(s.args(OP_SPAWN_REACTOR_THREAD), vec![0, 1, 2], "threads");
        assert_eq!(s.called(OP_HEARTBEAT_THREAD), 1, "heartbeat thread");

        let mut s = Script::with(&[(OP_PORT_COUNT, 1), (OP_PORT_LISTEN, u64::MAX)]);
        assert_eq!(start_reactor_threads(&mut s), SW_ERR, "listen failed");
        assert_eq!(s.called(OP_EVENT_FREE), 1, "loop freed");

        let mut s =
            Script::with(&[(OP_REACTOR_NUM, 2), (OP_FIELD, 1)]).queue(OP_THREAD_JOINABLE, &[0, 1]);
        join_reactor_threads(&mut s);
        assert_eq!(s.called(OP_JOIN_HEARTBEAT), 1, "heartbeat joined");
        assert_eq!(s.args(OP_THREAD_JOIN), vec![1], "joinable thread");
    }

    #[test]
    fn main_loop_and_backoff() {
        let mut s = Script::with(&[(OP_MODE, MODE_PROCESS)]);
        main_loop(&mut s, 2);
        assert_eq!(s.args(OP_REACTOR_THREAD_TYPE), vec![2], "reactor thread");
        assert_eq!(s.called(OP_REACTOR_BARRIER), 1, "barrier");
        assert_eq!(s.called(OP_THREAD_CLEAN), 1, "cleaned");

        let mut s = Script::with(&[
            (OP_QUEUE_FULL, 1),
            (OP_WAITING_TIME, 64),
            (OP_RESUME_TIMER, 1),
        ]);
        resume_receiving(&mut s);
        assert_eq!(s.args(OP_SET_WAITING_TIME), vec![128], "doubled");
        assert_eq!(s.called(OP_RESUME_READ), 0, "still waiting");

        let mut s = Script::with(&[(OP_QUEUE_FULL, 1), (OP_WAITING_TIME, 1024)]);
        resume_receiving(&mut s);
        assert_eq!(s.called(OP_SET_WAITING_TIME), 0, "capped");
        assert_eq!(s.called(OP_RESUME_READ), 1, "timer failed: read now");
    }

    #[test]
    fn dispatch_counts_queued_bytes() {
        let mut s = Script::with(&[(OP_FACTORY_DISPATCH, 1)]);
        assert_eq!(dispatch_task(&mut s, 10), SW_OK, "dispatched");
        assert_eq!(s.args(OP_TASK_INIT), vec![10], "queued");
        assert_eq!(s.called(OP_COUNT_DISPATCH), 1, "counted");

        let mut s = Script::default();
        assert_eq!(dispatch_task(&mut s, 0), SW_ERR, "failed");
        assert_eq!(s.called(OP_QUEUE_SUB), 0, "nothing queued");
        let mut s = Script::default();
        assert_eq!(dispatch_task(&mut s, 10), SW_ERR, "failed");
        assert_eq!(s.args(OP_QUEUE_SUB), vec![10], "unqueued");
    }
}
