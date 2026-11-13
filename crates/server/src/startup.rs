//! Creating, checking, starting and destroying a server.
use crate::callbacks::{
    CB_BUFFER_EMPTY, CB_BUFFER_FULL, CB_CLOSE, CB_CONNECT, CB_PACKET, CB_RECEIVE, CB_TASK,
};
use crate::control::init_signals;
use crate::ops::{
    FIELD_CPU_NUM, FIELD_DISPATCH_MODE, FIELD_HEARTBEAT_INTERVAL, FIELD_REACTOR_NUM,
    FIELD_TASK_IPC_MODE, FIELD_TASK_WORKER_NUM, FIELD_UNSAFE_EVENT, FIELD_WORKER_NUM,
    LOG_ALLOC_PORT_GS, LOG_ALLOC_SESSIONS, LOG_ALLOC_TASK_RESULTS, LOG_ALLOC_WORKERS,
    LOG_CALLBACK_DISPATCH, LOG_CALLBACK_REQUIRED, LOG_MAX_CONNECTION_MINIMUM, LOG_REACTOR_NUM,
    LOG_TASK_POOL, LOG_TASK_WORKER_NUM, LOG_WORKER_NUM, OP_AFTER_CREATE, OP_AFTER_SHUTDOWN,
    OP_ALLOC_PORT_GS, OP_ALLOC_SESSIONS, OP_ALLOC_TASK_RESULTS, OP_ALLOC_WORKERS, OP_CALLBACK,
    OP_CLEAR_CALLBACK, OP_CLEAR_MAX_QUEUED, OP_CLEAR_SEND_YIELD, OP_COMMAND_HANDLER,
    OP_CREATE_FACTORY, OP_DESTROY_FACTORY, OP_DESTROY_TASK_WORKERS, OP_DESTROY_WORKER,
    OP_EVENT_FREE, OP_EVENT_WAIT, OP_FACTORY_SHUTDOWN, OP_FIELD, OP_FREE_SERVER,
    OP_FREE_USER_WORKERS, OP_HAS_PIPE_COMMAND, OP_HTTP_DEFAULTS, OP_INIT_LOCAL_ADDRS,
    OP_INIT_TASK_WORKERS, OP_IS_STARTED, OP_JOIN_REACTOR_THREADS, OP_LAST_PORT_FD,
    OP_MANAGER_BARRIER, OP_MARK_SHUTDOWN, OP_MASTER_PID, OP_MASTER_REACTOR, OP_MASTER_START,
    OP_MASTER_THREAD, OP_MASTER_TIMER, OP_MASTER_WORKER, OP_MAX_CONNECTION, OP_MAX_SOCKETS,
    OP_MODE, OP_ON_SHUTDOWN, OP_PIPE_COMMAND_ADD, OP_PORT_CLOSE, OP_PORT_COUNT,
    OP_PORT_DEFAULT_IDLE, OP_PORT_DGRAM, OP_PORT_IDLE, OP_PORT_MAX_LENGTH, OP_PORT_MIN_LENGTH,
    OP_PORT_SSL_AT, OP_PORT_STREAM, OP_REACTOR_BARRIER, OP_RELEASE_PIPE_BUFFERS, OP_SEND_TIMEOUT,
    OP_SEND_TIMEOUT_MIN, OP_SET_DISABLE_NOTIFY, OP_SET_FIELD, OP_SET_MASTER_PID,
    OP_SET_MAX_CONNECTION, OP_SET_MAXFD, OP_SET_MINFD, OP_SINGLE_THREAD, OP_STORE_LISTEN,
    OP_TASK_PIPE, OP_TASK_POOL_CREATE, OP_TASK_POOL_LISTEN, OP_TASK_POOL_SETUP, OP_TIMER_DETACHED,
    OP_TIMER_REINIT,
};
use crate::{Host, MODE_BASE, MODE_PROCESS, MODE_THREAD, SW_ERR, SW_OK};

/// `SW_MAX_THREAD_NCPU`, `SW_MAX_WORKER_NCPU`, `SW_TIMER_MIN_SEC`, `SW_BUFFER_MIN_SIZE`;
/// native asserts each.
const MAX_THREAD_NCPU: u64 = 4;
const MAX_WORKER_NCPU: u64 = 1000;
const TIMER_MIN_SEC: f64 = 0.001;
const BUFFER_MIN_SIZE: u64 = 65536;

/// `Server::DispatchMode` values that hash a connection to one worker.
const DISPATCH_FDMOD: u64 = 2;
const DISPATCH_IPMOD: u64 = 4;
const DISPATCH_CO_CONN_LB: u64 = 8;

/// `Server::TaskIpcMode` and `swIPCMode`.
const TASK_IPC_MSGQUEUE: u64 = 2;
const TASK_IPC_PREEMPTIVE: u64 = 3;
const TASK_IPC_STREAM: u64 = 4;
const IPC_UNIXSOCK: u64 = 1;
const IPC_MSGQUEUE: u64 = 2;
const IPC_SOCKET: u64 = 3;

fn field<H: Host>(h: &mut H, id: u32) -> crate::Effect<u64> {
    h.op(OP_FIELD, u64::from(id))
}

fn set_field<H: Host>(h: &mut H, id: u32, value: u64) -> crate::Effect<()> {
    h.op(OP_SET_FIELD, u64::from(id) | (value << 8))?;
    Ok(())
}

/// `Server::start`. This frame holds only scalars and the Host loan: a bailout
/// from an effect longjmps over it as over upstream's C++ frame, and nothing it
/// owns spans the daemon fork or the factory's worker forks.
pub(crate) fn start<H: Host>(h: &mut H) -> crate::Effect<i64> {
    use crate::ops::{
        LOG_LC_ONLY_ONE, OP_IS_CREATED, OP_LC_BEFORE_START, OP_LC_CHECK, OP_LC_CLAIM_START,
        OP_LC_CLEAR_ERROR, OP_LC_DAEMON, OP_LC_DAEMONIZE, OP_LC_FACTORY_START,
        OP_LC_INIT_PROTOCOLS, OP_LC_PREPARE_WORKERS, OP_LC_START_BASE, OP_LC_START_PROCESS,
        OP_LC_START_THREAD, OP_LC_START_TIME, OP_LC_UNLINK_PID, OP_LC_WRITE_PID, OP_SET_RUNNING,
    };
    h.get(OP_LC_CLEAR_ERROR)?;
    if h.signed(OP_LC_CHECK)? < 0 {
        return Ok(SW_ERR);
    }
    h.get(OP_LC_BEFORE_START)?;
    // Cannot start two servers at the same time; use process->exec.
    if !h.is(OP_LC_CLAIM_START)? {
        h.get(LOG_LC_ONLY_ONE)?;
        return Ok(SW_ERR);
    }
    if h.is(OP_LC_DAEMONIZE)? && h.signed(OP_LC_DAEMON)? < 0 {
        return Ok(SW_ERR);
    }
    h.get(OP_LC_START_TIME)?;
    h.get(OP_LC_PREPARE_WORKERS)?;
    h.op(OP_SET_RUNNING, 1)?;
    if !h.is(OP_LC_FACTORY_START)? {
        return Ok(SW_ERR);
    }
    h.get(OP_LC_INIT_PROTOCOLS)?;
    h.get(OP_LC_WRITE_PID)?;
    let started = match h.get(OP_MODE)? {
        MODE_BASE => h.signed(OP_LC_START_BASE)?,
        MODE_PROCESS => h.signed(OP_LC_START_PROCESS)?,
        MODE_THREAD => {
            // Before the worker threads start, which read it in onWorkerStart.
            h.get(OP_SET_MASTER_PID)?;
            h.signed(OP_LC_START_THREAD)?
        }
        _ => swoole_rs_corebase::fatal("an unknown server start mode"),
    };
    if started < 0 {
        return Ok(SW_ERR);
    }
    if h.is(OP_IS_CREATED)? {
        destroy(h)?;
    }
    h.get(OP_LC_UNLINK_PID)?;
    Ok(SW_OK)
}

/// `Server::create` after the global before-create hook: shared memory, bounded
/// thread and process counts, the factory and the task workers.
pub(crate) fn create<H: Host>(h: &mut H) -> crate::Effect<i64> {
    if !h.is(OP_INIT_LOCAL_ADDRS)? {
        return Ok(SW_ERR);
    }
    if !h.is(OP_ALLOC_SESSIONS)? {
        h.get(LOG_ALLOC_SESSIONS)?;
        return Ok(SW_ERR);
    }
    if !h.is(OP_ALLOC_PORT_GS)? {
        h.get(LOG_ALLOC_PORT_GS)?;
        return Ok(SW_ERR);
    }
    h.get(OP_HTTP_DEFAULTS)?;
    bound_max_connection(h)?;
    bound_workers(h)?;
    if !h.is(OP_ALLOC_WORKERS)? {
        h.get(LOG_ALLOC_WORKERS)?;
        return Ok(SW_ERR);
    }
    if !h.is(OP_CREATE_FACTORY)? {
        return Ok(SW_ERR);
    }
    if field(h, FIELD_TASK_WORKER_NUM)? > 0 && !create_task_workers(h)? {
        return Ok(SW_ERR);
    }
    h.get(OP_AFTER_CREATE)?;
    Ok(SW_OK)
}

/// Room for every worker's pipes and the listen sockets, in `uint32_t` as upstream.
fn bound_max_connection<H: Host>(h: &mut H) -> crate::Effect<()> {
    let workers = u32::try_from(field(h, FIELD_WORKER_NUM)?).unwrap_or(u32::MAX);
    let tasks = u32::try_from(field(h, FIELD_TASK_WORKER_NUM)?).unwrap_or(u32::MAX);
    let mut minimum = workers.wrapping_add(tasks).wrapping_mul(2).wrapping_add(32);
    if h.get(OP_PORT_COUNT)? > 0 {
        let fd = i32::try_from(h.signed(OP_LAST_PORT_FD)?).unwrap_or(-1);
        minimum = minimum.wrapping_add(fd.cast_unsigned());
    }
    let max = u32::try_from(h.get(OP_MAX_CONNECTION)?).unwrap_or(u32::MAX);
    if max < minimum {
        let sockets = u32::try_from(h.get(OP_MAX_SOCKETS)?).unwrap_or(u32::MAX);
        let real = minimum.wrapping_add(1).max(sockets);
        h.op(
            LOG_MAX_CONNECTION_MINIMUM,
            (u64::from(minimum) << 32) | u64::from(real),
        )?;
        h.op(OP_SET_MAX_CONNECTION, u64::from(real))?;
    }
    Ok(())
}

fn bound_workers<H: Host>(h: &mut H) -> crate::Effect<()> {
    let cpu = field(h, FIELD_CPU_NUM)?;
    let reactors = field(h, FIELD_REACTOR_NUM)?;
    if reactors > cpu * MAX_THREAD_NCPU {
        h.get(LOG_REACTOR_NUM)?;
        set_field(h, FIELD_REACTOR_NUM, cpu * MAX_THREAD_NCPU)?;
    } else if reactors == 0 {
        set_field(h, FIELD_REACTOR_NUM, cpu)?;
    }
    if h.is(OP_SINGLE_THREAD)? {
        set_field(h, FIELD_REACTOR_NUM, 1)?;
    }
    if field(h, FIELD_WORKER_NUM)? > cpu * MAX_WORKER_NCPU {
        h.get(LOG_WORKER_NUM)?;
        set_field(h, FIELD_WORKER_NUM, cpu * MAX_WORKER_NCPU)?;
    }
    let workers = field(h, FIELD_WORKER_NUM)?;
    if workers < field(h, FIELD_REACTOR_NUM)? {
        set_field(h, FIELD_REACTOR_NUM, workers)?;
    }
    if field(h, FIELD_TASK_WORKER_NUM)? > cpu * MAX_WORKER_NCPU {
        h.get(LOG_TASK_WORKER_NUM)?;
        set_field(h, FIELD_TASK_WORKER_NUM, cpu * MAX_WORKER_NCPU)?;
    }
    Ok(())
}

/// `Server::create_task_workers`: the task pool's memory, not its processes.
fn create_task_workers<H: Host>(h: &mut H) -> crate::Effect<bool> {
    let ipc = match field(h, FIELD_TASK_IPC_MODE)? {
        TASK_IPC_MSGQUEUE | TASK_IPC_PREEMPTIVE => IPC_MSGQUEUE,
        TASK_IPC_STREAM => IPC_SOCKET,
        _ => IPC_UNIXSOCK,
    };
    if h.op(OP_TASK_POOL_CREATE, ipc)? == 0 {
        h.get(LOG_TASK_POOL)?;
        return Ok(false);
    }
    h.get(OP_TASK_POOL_SETUP)?;
    if ipc == IPC_SOCKET {
        let path = format!("/tmp/swoole.task.{}.sock\0", h.signed(OP_MASTER_PID)?);
        let address = u64::try_from(path.as_ptr().addr()).unwrap_or(0);
        if h.op(OP_TASK_POOL_LISTEN, address)?.cast_signed() < 0 {
            return Ok(false);
        }
    }
    // Task results and notify pipes for `Server::task_sync()`.
    if !h.is(OP_ALLOC_TASK_RESULTS)? {
        h.get(LOG_ALLOC_TASK_RESULTS)?;
        return Ok(false);
    }
    for _ in 0..field(h, FIELD_WORKER_NUM)? {
        if !h.is(OP_TASK_PIPE)? {
            return Ok(false);
        }
    }
    h.is(OP_INIT_TASK_WORKERS)
}

/// `Server::start_check`: settings the mode cannot honour, required callbacks, and
/// heartbeat and package-length defaults.
pub(crate) fn start_check<H: Host>(h: &mut H) -> crate::Effect<i64> {
    if h.get(OP_MODE)? == MODE_PROCESS {
        let hashed = matches!(
            field(h, FIELD_DISPATCH_MODE)?,
            DISPATCH_FDMOD | DISPATCH_IPMOD | DISPATCH_CO_CONN_LB
        );
        if !hashed && field(h, FIELD_UNSAFE_EVENT)? == 0 {
            for callback in [CB_CONNECT, CB_CLOSE, CB_BUFFER_FULL, CB_BUFFER_EMPTY] {
                if h.op(OP_CALLBACK, u64::from(callback))? != 0 {
                    h.op(LOG_CALLBACK_DISPATCH, u64::from(callback))?;
                    h.op(OP_CLEAR_CALLBACK, u64::from(callback))?;
                }
            }
            h.get(OP_SET_DISABLE_NOTIFY)?;
        }
        if !hashed {
            h.get(OP_CLEAR_SEND_YIELD)?;
        }
    } else {
        h.get(OP_CLEAR_MAX_QUEUED)?;
    }
    if field(h, FIELD_TASK_WORKER_NUM)? > 0 && !callback(h, CB_TASK)? {
        h.op(LOG_CALLBACK_REQUIRED, u64::from(CB_TASK))?;
        return Ok(SW_ERR);
    }
    let send_timeout = h.float(OP_SEND_TIMEOUT)?;
    if send_timeout > 0.0 && send_timeout < TIMER_MIN_SEC {
        h.get(OP_SEND_TIMEOUT_MIN)?;
    }
    let ports = h.get(OP_PORT_COUNT)?;
    if field(h, FIELD_HEARTBEAT_INTERVAL)? > 0 {
        for port in 0..ports {
            if h.op(OP_PORT_IDLE, port)? == 0 {
                h.op(OP_PORT_DEFAULT_IDLE, port)?;
            }
        }
    }
    for port in 0..ports {
        if check_port(h, port)?.is_err() {
            return Ok(SW_ERR);
        }
    }
    Ok(SW_OK)
}

fn callback<H: Host>(h: &mut H, id: u32) -> crate::Effect<bool> {
    Ok(h.op(OP_CALLBACK, u64::from(id))? != 0)
}

fn check_port<H: Host>(h: &mut H, port: u64) -> crate::Effect<Result<(), ()>> {
    if h.op(OP_PORT_MAX_LENGTH, port)? < BUFFER_MIN_SIZE {
        h.op(OP_PORT_MIN_LENGTH, port)?;
    }
    let dgram = h.op(OP_PORT_DGRAM, port)? != 0;
    let ssl = h.op(OP_PORT_SSL_AT, port)? != 0;
    let stream = h.op(OP_PORT_STREAM, port)? != 0;
    if ((dgram && ssl) || stream) && !callback(h, CB_RECEIVE)? {
        h.op(LOG_CALLBACK_REQUIRED, u64::from(CB_RECEIVE))?;
        return Ok(Err(()));
    }
    if dgram && !ssl && !callback(h, CB_PACKET)? {
        h.op(LOG_CALLBACK_REQUIRED, u64::from(CB_PACKET))?;
        return Ok(Err(()));
    }
    let idle = h.op(OP_PORT_IDLE, port)?;
    if idle > 0 {
        let expect = if idle > 2 { idle / 2 } else { 1 };
        let interval = field(h, FIELD_HEARTBEAT_INTERVAL)?;
        if interval == 0 || interval > expect {
            set_field(h, FIELD_HEARTBEAT_INTERVAL, expect)?;
        }
    }
    Ok(Ok(()))
}

/// `Server::start_master_thread`: the master's reactor, signals and timer, then the event loop.
pub(crate) fn start_master_thread<H: Host>(h: &mut H) -> crate::Effect<i64> {
    let single = h.is(OP_SINGLE_THREAD)?;
    let thread_id = if single {
        0
    } else {
        field(h, FIELD_REACTOR_NUM)?
    };
    h.op(OP_MASTER_THREAD, thread_id)?;
    if h.is(OP_TIMER_DETACHED)? {
        h.get(OP_TIMER_REINIT)?;
    }
    init_signals(h)?;
    let mode = h.get(OP_MODE)?;
    h.op(OP_MASTER_WORKER, mode)?;
    h.get(OP_MASTER_REACTOR)?;
    if h.is(OP_HAS_PIPE_COMMAND)? {
        if !single {
            h.get(OP_COMMAND_HANDLER)?;
        }
        h.get(OP_PIPE_COMMAND_ADD)?;
    }
    if !h.is(OP_MASTER_TIMER)? {
        h.get(OP_EVENT_FREE)?;
        return Ok(SW_ERR);
    }
    if !single && mode != MODE_THREAD {
        h.get(OP_REACTOR_BARRIER)?;
    }
    if mode == MODE_PROCESS {
        h.get(OP_MANAGER_BARRIER)?;
    }
    if mode != MODE_THREAD {
        h.get(OP_SET_MASTER_PID)?;
    }
    h.get(OP_MASTER_START)?;
    h.signed(OP_EVENT_WAIT)
}

/// `Server::store_listen_socket`: each listen socket in the connection list.
pub(crate) fn store_listen_socket<H: Host>(h: &mut H) -> crate::Effect<()> {
    for port in 0..h.get(OP_PORT_COUNT)? {
        let fd = h.op(OP_STORE_LISTEN, port)?;
        if fd.cast_signed() >= 0 {
            h.op(OP_SET_MINFD, fd)?;
            h.op(OP_SET_MAXFD, fd)?;
        }
    }
    Ok(())
}

/// `Server::destroy` once the factory exists. In process mode the reactor threads
/// finish their write events before the factory shuts down.
pub(crate) fn destroy<H: Host>(h: &mut H) -> crate::Effect<()> {
    h.get(OP_AFTER_SHUTDOWN)?;
    let mode = h.get(OP_MODE)?;
    if mode == MODE_PROCESS {
        h.get(OP_JOIN_REACTOR_THREADS)?;
    }
    if h.is(OP_IS_STARTED)? {
        h.get(OP_FACTORY_SHUTDOWN)?;
    }
    for worker in 0..field(h, FIELD_WORKER_NUM)? {
        h.op(OP_DESTROY_WORKER, worker)?;
    }
    h.get(OP_RELEASE_PIPE_BUFFERS)?;
    for port in 0..h.get(OP_PORT_COUNT)? {
        h.op(OP_PORT_CLOSE, port)?;
    }
    h.get(OP_FREE_USER_WORKERS)?;
    h.get(OP_MARK_SHUTDOWN)?;
    h.get(OP_ON_SHUTDOWN)?;
    h.op(OP_DESTROY_FACTORY, mode)?;
    if field(h, FIELD_TASK_WORKER_NUM)? > 0 {
        h.get(OP_DESTROY_TASK_WORKERS)?;
    }
    h.get(OP_FREE_SERVER)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::script::Script;
    use crate::{Host, MODE_BASE};

    fn starting(mode: u64, daemon: bool) -> Script {
        use crate::ops::{OP_IS_CREATED, OP_LC_CLAIM_START, OP_LC_DAEMONIZE, OP_LC_FACTORY_START};
        Script::with(&[
            (OP_MODE, mode),
            (OP_IS_CREATED, 1),
            (OP_LC_CLAIM_START, 1),
            (OP_LC_FACTORY_START, 1),
            (OP_LC_DAEMONIZE, u64::from(daemon)),
        ])
    }

    #[test]
    fn start_orders_the_daemon_workers_factory_mode_and_pid_file() {
        use crate::ops::*;
        for (mode, run) in [
            (MODE_BASE, OP_LC_START_BASE),
            (MODE_PROCESS, OP_LC_START_PROCESS),
            (MODE_THREAD, OP_LC_START_THREAD),
        ] {
            for daemon in [false, true] {
                let mut s = starting(mode, daemon);
                assert_eq!(start(&mut s).unwrap(), SW_OK);
                let mut expected = vec![
                    OP_LC_CLEAR_ERROR,
                    OP_LC_CHECK,
                    OP_LC_BEFORE_START,
                    OP_LC_CLAIM_START,
                    OP_LC_DAEMONIZE,
                ];
                if daemon {
                    expected.push(OP_LC_DAEMON);
                }
                expected.extend([
                    OP_LC_START_TIME,
                    OP_LC_PREPARE_WORKERS,
                    OP_SET_RUNNING,
                    OP_LC_FACTORY_START,
                    OP_LC_INIT_PROTOCOLS,
                    OP_LC_WRITE_PID,
                    OP_MODE,
                ]);
                if mode == MODE_THREAD {
                    expected.push(OP_SET_MASTER_PID);
                }
                expected.extend([run, OP_IS_CREATED, OP_AFTER_SHUTDOWN]);
                let ops: Vec<u32> = s.log.iter().map(|&(op, _)| op).collect();
                assert_eq!(
                    ops[..expected.len()],
                    expected,
                    "mode {mode}, daemon {daemon}"
                );
                assert_eq!(ops.last(), Some(&OP_LC_UNLINK_PID));
                assert_eq!(s.args(OP_SET_RUNNING), [1]);
            }
        }
    }

    #[test]
    fn start_failures_return_before_later_effects() {
        use crate::ops::*;
        let failed = (-1_i64).cast_unsigned();
        for (op, answer, absent) in [
            (OP_LC_CHECK, failed, OP_LC_BEFORE_START),
            (OP_LC_CLAIM_START, 0, OP_LC_DAEMONIZE),
            (OP_LC_DAEMON, failed, OP_LC_START_TIME),
            (OP_LC_FACTORY_START, 0, OP_LC_WRITE_PID),
            (OP_LC_START_PROCESS, failed, OP_AFTER_SHUTDOWN),
        ] {
            let mut s = starting(MODE_PROCESS, true);
            s.facts.insert(op, answer);
            assert_eq!(start(&mut s).unwrap(), SW_ERR);
            assert_eq!(s.called(absent), 0, "{op}");
            assert_eq!(s.called(OP_LC_UNLINK_PID), 0, "the PID file stays");
            assert_eq!(
                s.called(LOG_LC_ONLY_ONE),
                usize::from(op == OP_LC_CLAIM_START)
            );
        }
    }

    #[test]
    fn start_skips_destroy_once_the_factory_is_gone() {
        use crate::ops::*;
        let mut s = starting(MODE_BASE, false);
        s.facts.insert(OP_IS_CREATED, 0);
        assert_eq!(start(&mut s).unwrap(), SW_OK);
        assert_eq!(s.called(OP_AFTER_SHUTDOWN), 0);
        assert_eq!(s.called(OP_LC_UNLINK_PID), 1);
    }

    /// A script whose fields live in a table that `OP_SET_FIELD` writes.
    struct Fields {
        script: Script,
        fields: [u64; 8],
    }

    impl Host for Fields {
        fn op(&mut self, op: u32, arg: u64) -> crate::Effect<u64> {
            let answer = self.script.op(op, arg)?;
            Ok(match op {
                OP_FIELD => self.fields[usize::try_from(arg).unwrap_or(usize::MAX)],
                OP_SET_FIELD => {
                    self.fields[usize::try_from(arg & 0xff).unwrap_or(usize::MAX)] = arg >> 8;
                    0
                }
                _ => answer,
            })
        }
        fn state(&mut self) -> crate::Effect<&crate::ServerState> {
            self.script.state()
        }
    }

    fn server(facts: &[(u32, u64)], fields: [(u32, u64); 3]) -> Fields {
        let mut out = Fields {
            script: Script::with(facts),
            fields: [0; 8],
        };
        out.fields[FIELD_CPU_NUM as usize] = 4;
        for (id, value) in fields {
            out.fields[id as usize] = value;
        }
        out
    }

    const CREATED: &[(u32, u64)] = &[
        (OP_INIT_LOCAL_ADDRS, 1),
        (OP_ALLOC_SESSIONS, 1),
        (OP_ALLOC_PORT_GS, 1),
        (OP_ALLOC_WORKERS, 1),
        (OP_CREATE_FACTORY, 1),
        (OP_PORT_COUNT, 1),
        (OP_LAST_PORT_FD, 5),
        (OP_MAX_CONNECTION, 100_000),
        (OP_TASK_PIPE, 1),
        (OP_TASK_POOL_CREATE, 1),
        (OP_ALLOC_TASK_RESULTS, 1),
        (OP_INIT_TASK_WORKERS, 1),
    ];

    #[test]
    fn create_bounds_counts() {
        let mut s = server(
            CREATED,
            [
                (FIELD_REACTOR_NUM, 100),
                (FIELD_WORKER_NUM, 8),
                (FIELD_TASK_WORKER_NUM, 0),
            ],
        );
        assert_eq!(create(&mut s).unwrap(), SW_OK, "created");
        assert_eq!(s.script.called(LOG_REACTOR_NUM), 1, "too many reactors");
        assert_eq!(
            s.fields[FIELD_REACTOR_NUM as usize], 8,
            "16 then bounded by 8 workers"
        );
        assert_eq!(s.script.called(OP_AFTER_CREATE), 1, "hook");
        assert_eq!(s.script.called(OP_TASK_POOL_CREATE), 0, "no task workers");

        let mut s = server(
            CREATED,
            [
                (FIELD_REACTOR_NUM, 0),
                (FIELD_WORKER_NUM, 5000),
                (FIELD_TASK_WORKER_NUM, 5000),
            ],
        );
        assert_eq!(create(&mut s).unwrap(), SW_OK, "created");
        assert_eq!(s.fields[FIELD_WORKER_NUM as usize], 4000, "workers bounded");
        assert_eq!(
            s.fields[FIELD_TASK_WORKER_NUM as usize], 4000,
            "task workers bounded"
        );
        assert_eq!(s.fields[FIELD_REACTOR_NUM as usize], 4, "default reactors");
        assert_eq!(s.script.called(OP_TASK_PIPE), 4000, "one pipe per worker");
    }

    #[test]
    fn create_raises_max_connection_to_the_minimum() {
        let mut facts = CREATED.to_vec();
        facts.push((OP_MAX_CONNECTION, 10));
        facts.push((OP_MAX_SOCKETS, 1024));
        let mut s = server(
            &facts,
            [
                (FIELD_WORKER_NUM, 2),
                (FIELD_TASK_WORKER_NUM, 1),
                (FIELD_REACTOR_NUM, 1),
            ],
        );
        create(&mut s).unwrap();
        // (2 + 1) * 2 + 32 + fd 5
        assert_eq!(
            s.script.args(LOG_MAX_CONNECTION_MINIMUM),
            vec![(43 << 32) | 1024],
            "warned"
        );
        assert_eq!(
            s.script.args(OP_SET_MAX_CONNECTION),
            vec![1024],
            "max_sockets wins"
        );
    }

    #[test]
    fn create_stops_at_the_first_failure() {
        let mut s = server(&[(OP_INIT_LOCAL_ADDRS, 1)], [(FIELD_WORKER_NUM, 1); 3]);
        assert_eq!(create(&mut s).unwrap(), SW_ERR, "sessions");
        assert_eq!(s.script.called(LOG_ALLOC_SESSIONS), 1, "warned");
        assert_eq!(s.script.called(OP_ALLOC_PORT_GS), 0, "stopped");
    }

    #[test]
    fn task_workers_by_ipc_mode() {
        let mut facts = CREATED.to_vec();
        facts.extend([
            (OP_TASK_POOL_CREATE, 1),
            (OP_ALLOC_TASK_RESULTS, 1),
            (OP_INIT_TASK_WORKERS, 1),
            (OP_MASTER_PID, 42),
        ]);
        let mut s = server(
            &facts,
            [
                (FIELD_WORKER_NUM, 2),
                (FIELD_TASK_WORKER_NUM, 1),
                (FIELD_TASK_IPC_MODE, TASK_IPC_STREAM),
            ],
        );
        assert_eq!(create(&mut s).unwrap(), SW_OK, "created");
        assert_eq!(
            s.script.args(OP_TASK_POOL_CREATE),
            vec![IPC_SOCKET],
            "stream mode listens"
        );
        assert_eq!(
            s.script.called(OP_TASK_POOL_LISTEN),
            1,
            "on the socket file"
        );
        assert_eq!(s.script.called(OP_TASK_PIPE), 2, "a pipe per event worker");

        let mut s = server(
            &facts,
            [
                (FIELD_WORKER_NUM, 2),
                (FIELD_TASK_WORKER_NUM, 1),
                (FIELD_TASK_IPC_MODE, TASK_IPC_PREEMPTIVE),
            ],
        );
        create(&mut s).unwrap();
        assert_eq!(
            s.script.args(OP_TASK_POOL_CREATE),
            vec![IPC_MSGQUEUE],
            "message queue"
        );
        assert_eq!(s.script.called(OP_TASK_POOL_LISTEN), 0, "no socket");
    }

    #[test]
    fn start_check_drops_callbacks_unsafe_for_the_dispatch_mode() {
        let mut s = server(
            &[(OP_MODE, MODE_PROCESS), (OP_CALLBACK, 1)],
            [
                (FIELD_DISPATCH_MODE, 1),
                (FIELD_UNSAFE_EVENT, 0),
                (FIELD_TASK_WORKER_NUM, 0),
            ],
        );
        assert_eq!(start_check(&mut s).unwrap(), SW_OK, "no ports");
        assert_eq!(
            s.script.args(OP_CLEAR_CALLBACK),
            vec![0, 1, 2, 3],
            "connect, close, buffer full, buffer empty"
        );
        assert_eq!(s.script.called(OP_SET_DISABLE_NOTIFY), 1, "notify off");
        assert_eq!(
            s.script.called(OP_CLEAR_SEND_YIELD),
            1,
            "round robin cannot yield"
        );

        let mut s = server(
            &[(OP_MODE, MODE_PROCESS), (OP_CALLBACK, 1)],
            [(FIELD_DISPATCH_MODE, 2); 3],
        );
        start_check(&mut s).unwrap();
        assert_eq!(
            s.script.called(OP_CLEAR_CALLBACK) + s.script.called(OP_CLEAR_SEND_YIELD),
            0,
            "hash mode keeps all"
        );

        let mut s = server(&[(OP_MODE, MODE_BASE)], [(FIELD_TASK_WORKER_NUM, 1); 3]);
        assert_eq!(
            start_check(&mut s).unwrap(),
            SW_ERR,
            "task workers need onTask"
        );
        assert_eq!(s.script.called(OP_CLEAR_MAX_QUEUED), 1, "base mode");
        assert_eq!(
            s.script.args(LOG_CALLBACK_REQUIRED),
            vec![u64::from(CB_TASK)],
            "warned"
        );
    }

    #[test]
    fn start_check_ports() {
        let mut s = server(
            &[
                (OP_MODE, MODE_BASE),
                (OP_PORT_COUNT, 2),
                (OP_PORT_STREAM, 1),
                (OP_CALLBACK, 1),
                (OP_PORT_MAX_LENGTH, 1024),
                (OP_SEND_TIMEOUT, 0.0001f64.to_bits()),
            ],
            [
                (FIELD_HEARTBEAT_INTERVAL, 60),
                (FIELD_TASK_WORKER_NUM, 0),
                (FIELD_DISPATCH_MODE, 0),
            ],
        )
        .with_queue(OP_PORT_IDLE, &[0, 30, 30, 7]);
        assert_eq!(start_check(&mut s).unwrap(), SW_OK, "valid");
        assert_eq!(
            s.script.args(OP_PORT_DEFAULT_IDLE),
            vec![0],
            "port without idle time"
        );
        assert_eq!(
            s.fields[FIELD_HEARTBEAT_INTERVAL as usize], 3,
            "60 -> 15 -> 3"
        );
        assert_eq!(
            s.script.called(OP_PORT_MIN_LENGTH),
            2,
            "package length raised"
        );
        assert_eq!(
            s.script.called(OP_SEND_TIMEOUT_MIN),
            1,
            "send timeout raised"
        );

        let mut s = server(
            &[(OP_PORT_COUNT, 1), (OP_PORT_DGRAM, 1)],
            [(FIELD_DISPATCH_MODE, 0); 3],
        );
        assert_eq!(start_check(&mut s).unwrap(), SW_ERR, "UDP needs onPacket");
        assert_eq!(
            s.script.args(LOG_CALLBACK_REQUIRED),
            vec![u64::from(CB_PACKET)],
            "warned"
        );
    }

    impl Fields {
        fn with_queue(mut self, op: u32, answers: &[u64]) -> Self {
            self.script = self.script.queue(op, answers);
            self
        }
    }

    #[test]
    fn master_thread_by_mode() {
        let mut s = server(
            &[
                (OP_MODE, MODE_PROCESS),
                (OP_HAS_PIPE_COMMAND, 1),
                (OP_MASTER_TIMER, 1),
                (OP_EVENT_WAIT, 0),
            ],
            [(FIELD_REACTOR_NUM, 4); 3],
        );
        assert_eq!(start_master_thread(&mut s).unwrap(), SW_OK, "loop ran");
        assert_eq!(
            s.script.args(OP_MASTER_THREAD),
            vec![4],
            "thread id after the reactors"
        );
        assert_eq!(
            s.script.called(OP_COMMAND_HANDLER),
            1,
            "multi-threaded command results"
        );
        assert_eq!(s.script.called(OP_REACTOR_BARRIER), 1, "reactor barrier");
        assert_eq!(s.script.called(OP_MANAGER_BARRIER), 1, "manager barrier");
        assert_eq!(s.script.called(OP_SET_MASTER_PID), 1, "master pid");

        let mut s = server(
            &[(OP_MODE, MODE_THREAD), (OP_MASTER_TIMER, 1)],
            [(FIELD_REACTOR_NUM, 4); 3],
        );
        start_master_thread(&mut s).unwrap();
        assert_eq!(
            s.script.called(OP_SET_MASTER_PID),
            0,
            "set before the worker threads start"
        );

        let mut s = server(
            &[(OP_MODE, MODE_BASE), (OP_SINGLE_THREAD, 1)],
            [(FIELD_REACTOR_NUM, 4); 3],
        );
        assert_eq!(start_master_thread(&mut s).unwrap(), SW_ERR, "timer failed");
        assert_eq!(s.script.called(OP_EVENT_FREE), 1, "event loop freed");
        assert_eq!(
            s.script.args(OP_MASTER_THREAD),
            vec![0],
            "single thread is 0"
        );
    }

    #[test]
    fn listen_sockets_and_destroy() {
        let mut s = Script::with(&[(OP_PORT_COUNT, 2)]).queue(OP_STORE_LISTEN, &[3, u64::MAX]);
        store_listen_socket(&mut s).unwrap();
        assert_eq!(s.args(OP_SET_MAXFD), vec![3], "valid fd only");

        let mut s = server(
            &[
                (OP_MODE, MODE_PROCESS),
                (OP_IS_STARTED, 1),
                (OP_PORT_COUNT, 1),
            ],
            [
                (FIELD_WORKER_NUM, 2),
                (FIELD_TASK_WORKER_NUM, 1),
                (FIELD_REACTOR_NUM, 1),
            ],
        );
        destroy(&mut s).unwrap();
        let order: Vec<u32> = s
            .script
            .log
            .iter()
            .map(|(op, _)| *op)
            .filter(|op| *op != OP_FIELD)
            .collect();
        let join = order
            .iter()
            .position(|op| *op == OP_JOIN_REACTOR_THREADS)
            .unwrap();
        let shutdown = order
            .iter()
            .position(|op| *op == OP_FACTORY_SHUTDOWN)
            .unwrap();
        assert!(
            join < shutdown,
            "reactor threads join before the factory stops"
        );
        assert_eq!(s.script.args(OP_DESTROY_WORKER), vec![0, 1], "each worker");
        assert_eq!(
            s.script.args(OP_DESTROY_FACTORY),
            vec![MODE_PROCESS],
            "process factory"
        );
        assert_eq!(order.last(), Some(&OP_FREE_SERVER), "server memory last");
    }
}
