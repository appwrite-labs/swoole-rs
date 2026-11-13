//! Operation and op numbers, Rust's on both ends.
//! The native context carries the server and, per operation, the reactor, port,
//! socket and connection an op acts on.

// Operations the runner (`crate::run`) matches.
/// `Server::start_master_thread`: context reactor.
pub const RUN_START_MASTER_THREAD: u32 = 17;
/// `Server::destroy` once there is a factory.
pub const RUN_DESTROY: u32 = 19;

// Reactor threads (`src/server/reactor_thread.cc`); context reactor and its thread.

// `BaseFactory` (`src/server/base.cc`, `crates/server/src/base_factory.rs`).
/// One native op, arg the op: the operands are in the context (`server_methods.rs`).
pub const RUN_NATIVE_OP: u32 = 48;

// Server facts.
pub const OP_MODE: u32 = 0;
pub const OP_SINGLE_THREAD: u32 = 1;
/// `errno`.
pub const OP_ERRNO: u32 = 3;
pub const OP_DELAY_RECEIVE: u32 = 7;
pub const OP_DISABLE_NOTIFY: u32 = 9;
pub const OP_IS_STARTED: u32 = 10;
/// `is_running()`.
pub const OP_RUNNING: u32 = 11;
pub const OP_SET_RUNNING: u32 = 12;
pub const OP_MANAGER_PID: u32 = 14;
/// `max_wait_time`.
pub const OP_MAX_WAIT_TIME: u32 = 17;
/// `SIGRTMIN`, or 0 where it is not defined.
pub const OP_SIGRTMIN: u32 = 18;
/// `SwooleG.signal_fd`.
pub const OP_SIGNAL_FD: u32 = 19;
pub const OP_MAXFD: u32 = 20;
pub const OP_MINFD: u32 = 21;
pub const OP_SET_MINFD: u32 = 23;

// Listen ports, by index in `ports`.
pub const OP_PORT_COUNT: u32 = 30;
pub const OP_PORT_DGRAM: u32 = 31;
pub const OP_PORT_REMOVED: u32 = 34;

// The context's listen port.
/// `max_idle_time`, a `double`.
pub const OP_PORT_IDLE_TIME: u32 = 41;

// The context's socket and connection.
/// `gs->session_round`.
pub const OP_SESSION_ROUND: u32 = 61;
/// `lock()`, `unlock()`.
pub const OP_LOCK: u32 = 70;
pub const OP_UNLOCK: u32 = 71;
/// `notify(conn, SW_SERVER_EVENT_CONNECT)`.
pub const OP_NOTIFY_CONNECT: u32 = 76;
/// `conn->closed || conn->close_force`.
pub const OP_CONN_CLOSING: u32 = 81;

// Timers, signals and shutdown.
/// `swoole_signal_set(arg, nullptr)` and `swoole_signal_set(arg, master_signal_handler)`.
pub const OP_SIGNAL_IGNORE: u32 = 105;
pub const OP_SIGNAL_HANDLE: u32 = 106;

// Startup and teardown (`RUN_CREATE`, `RUN_START_CHECK`, `RUN_START_MASTER_THREAD`,
// `RUN_STORE_LISTEN_SOCKET`, `RUN_DESTROY`).
/// A numeric server field (`FIELD_*`).
pub const OP_FIELD: u32 = 150;
pub const OP_PORT_STREAM: u32 = 177;
pub const OP_PORT_SSL_AT: u32 = 178;
pub const OP_PORT_WEBSOCKET: u32 = 179;
/// `swoole_set_thread_type(THREAD_MASTER)`, `swoole_set_thread_id(arg)`.
pub const OP_MASTER_THREAD: u32 = 181;
/// The thread's timer has no reactor; `reinit()` it.
pub const OP_TIMER_DETACHED: u32 = 182;
pub const OP_TIMER_REINIT: u32 = 183;
/// `swoole_set_worker_type(SW_MASTER)` and the worker pid for mode arg.
pub const OP_MASTER_WORKER: u32 = 184;
/// `reactor->ptr` and the accept handler.
pub const OP_MASTER_REACTOR: u32 = 185;
pub const OP_HAS_PIPE_COMMAND: u32 = 186;
/// The command-result handler, and the command pipe added for reading.
pub const OP_COMMAND_HANDLER: u32 = 187;
pub const OP_PIPE_COMMAND_ADD: u32 = 188;
/// Arms the 1s master timer; whether it was armed.
pub const OP_MASTER_TIMER: u32 = 189;
pub const OP_EVENT_FREE: u32 = 190;
pub const OP_REACTOR_BARRIER: u32 = 191;
pub const OP_MANAGER_BARRIER: u32 = 192;
/// `gs->master_pid = getpid()`.
pub const OP_SET_MASTER_PID: u32 = 193;
/// `onStart`, when set.
pub const OP_MASTER_START: u32 = 194;
/// `swoole_event_wait()`.
pub const OP_EVENT_WAIT: u32 = 195;
/// `SW_GLOBAL_HOOK_AFTER_SERVER_SHUTDOWN`, when set.
pub const OP_AFTER_SHUTDOWN: u32 = 197;
pub const OP_JOIN_REACTOR_THREADS: u32 = 198;
pub const OP_FACTORY_SHUTDOWN: u32 = 199;
/// `destroy_worker(&workers[arg])`.
pub const OP_DESTROY_WORKER: u32 = 200;
pub const OP_RELEASE_PIPE_BUFFERS: u32 = 201;
pub const OP_PORT_CLOSE: u32 = 202;
pub const OP_FREE_USER_WORKERS: u32 = 203;
/// `swoole_signal_clear()`, then `gs->start = 0` and `gs->shutdown = 1`.
pub const OP_MARK_SHUTDOWN: u32 = 204;
/// `onShutdown`, when set.
pub const OP_ON_SHUTDOWN: u32 = 205;
/// The base, thread or process factory's teardown for mode arg.
pub const OP_DESTROY_FACTORY: u32 = 207;
pub const OP_DESTROY_TASK_WORKERS: u32 = 208;
/// Frees the session list, port statistics and workers, then the factory, and
/// clears `SwooleG.server`.
pub const OP_FREE_SERVER: u32 = 209;

// Numeric server fields for `OP_FIELD`/`OP_SET_FIELD`; `CPU_NUM` is `SwooleG.cpu_num`.
pub const FIELD_REACTOR_NUM: u32 = 0;
pub const FIELD_WORKER_NUM: u32 = 1;
pub const FIELD_TASK_WORKER_NUM: u32 = 2;

// Reactor threads. Ops 0-144 keep their meaning where master.cc's reactor-thread ops answer them.
pub const OP_HAVE_DGRAM: u32 = 264;
/// Port arg's fd.
pub const OP_PORT_FD: u32 = 265;
/// `reactor->id`.
pub const OP_REACTOR_ID: u32 = 266;
pub const OP_REACTOR_NUM: u32 = 267;
/// `reactor->del(port->socket)` for port arg.
pub const OP_PORT_DEL: u32 = 268;
pub const OP_WORKER_NUM: u32 = 272;
/// Selects the connection at fd arg when valid.
pub const OP_CONN_AT: u32 = 274;
pub const OP_CONN_FD: u32 = 275;
pub const OP_CONN_REACTOR: u32 = 276;
pub const OP_CONN_UNREAD: u32 = 279;
/// Selects the connection of session arg (`get_connection_verify_no_ssl`).
pub const OP_SESSION_CONN: u32 = 281;
pub const OP_COMMAND_REQUEST: u32 = 291;
pub const OP_COMMAND_RESPONSE: u32 = 292;
pub const OP_ON_FINISH: u32 = 294;
pub const OP_ON_PIPE_MESSAGE: u32 = 295;
/// `is_healthy_connection(arg as double)` of the connection.
pub const OP_CONN_HEALTHY: u32 = 301;
/// Port arg listens; then reads.
pub const OP_PORT_LISTEN: u32 = 303;
/// `recv_queued_bytes` -= arg.
pub const OP_QUEUE_SUB: u32 = 325;
/// The `Server *` the thread entries take.
pub const OP_SERVER: u32 = 328;

// `ServerEventType` values on the reactor pipe.
pub const EVENT_CLOSE_FORCE: u32 = 6;
pub const EVENT_CLOSE_FORWARD: u32 = 7;
pub const EVENT_FINISH: u32 = 9;
pub const EVENT_PIPE_MESSAGE: u32 = 10;
pub const EVENT_INCOMING: u32 = 15;
pub const EVENT_SHUTDOWN: u32 = 16;
pub const EVENT_COMMAND_REQUEST: u32 = 17;
pub const EVENT_COMMAND_RESPONSE: u32 = 18;
pub const EVENT_SHUTDOWN_SIGNAL: u32 = 19;

// Dispatch, sends and ports.
/// `Server::state_`, a `state::ServerState`.
pub const OP_STATE: u32 = 402;
pub const OP_IS_CREATED: u32 = 425;

// `BaseFactory`.

// Rust-owned state and the rest of `master.cc` (`crates/server/src/state.rs`).
/// `gs->concurrency`: subtract arg; read; set 0.
pub const OP_ST_GS_CONCURRENCY_SUB: u32 = 1020;
pub const OP_ST_GS_CONCURRENCY: u32 = 1021;
pub const OP_ST_GS_CONCURRENCY_ZERO: u32 = 1022;

// `Server::start` (`RUN_START`). `OP_LC_CLAIM_START`, `OP_LC_START_TIME` and
// `OP_LC_DAEMONIZE` read and write the Rust records; the rest are native effects.
pub const RUN_START: u32 = 320;
pub const OP_LC_CLEAR_ERROR: u32 = 1200;
/// `Server::start_check()`.
pub const OP_LC_CHECK: u32 = 1201;
/// The `SW_GLOBAL_HOOK_BEFORE_SERVER_START` hook, if set.
pub const OP_LC_BEFORE_START: u32 = 1202;
/// Whether this call set `gs->start` from 0 to 1.
pub const OP_LC_CLAIM_START: u32 = 1203;
pub const LOG_LC_ONLY_ONE: u32 = 1204;
pub const OP_LC_DAEMONIZE: u32 = 1205;
/// Redirects stdout/stderr (to the log file or `/dev/null`), then `swoole_daemon(0, 1)`.
pub const OP_LC_DAEMON: u32 = 1206;
pub const OP_LC_START_TIME: u32 = 1207;
/// Publishes the event workers to their pool and numbers the user workers.
pub const OP_LC_PREPARE_WORKERS: u32 = 1208;
/// `factory_->start()`.
pub const OP_LC_FACTORY_START: u32 = 1209;
pub const OP_LC_WRITE_PID: u32 = 1210;
pub const OP_LC_UNLINK_PID: u32 = 1211;
/// `start_reactor_processes()`, `start_reactor_threads()`, `start_worker_threads()`.
pub const OP_LC_START_BASE: u32 = 1212;
pub const OP_LC_START_PROCESS: u32 = 1213;
pub const OP_LC_START_THREAD: u32 = 1214;
/// `Server::init_reactor`'s `init_protocol` over every port, once a start, before
/// any reactor or worker that reads the ports starts.
pub const OP_LC_INIT_PROTOCOLS: u32 = 1216;

#[cfg(test)]
mod tests {
    #[test]
    fn operations_are_distinct() {
        let mut seen = std::collections::HashMap::new();
        for line in include_str!("ops.rs").lines() {
            let Some(rest) = line.strip_prefix("pub const RUN_") else {
                continue;
            };
            let (name, value) = rest.split_once(": u32 = ").unwrap();
            let value: u32 = value.trim_end_matches(';').parse().unwrap();
            assert!(
                seen.insert(value, name).is_none(),
                "duplicate operation {value}: RUN_{name}"
            );
        }
    }
}
