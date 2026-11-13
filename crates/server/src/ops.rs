//! Operation and op numbers, Rust's on both ends.
//! The native context carries the server and, per operation, the reactor, port,
//! socket and connection an op acts on.

// Operations the runner (`crate::run`) matches.
/// `Server::is_healthy_connection`: arg the `double` now, context port and connection.
pub const RUN_HEALTHY: u32 = 7;
pub const RUN_CREATE: u32 = 15;
pub const RUN_START_CHECK: u32 = 16;
/// `Server::start_master_thread`: context reactor.
pub const RUN_START_MASTER_THREAD: u32 = 17;
pub const RUN_STORE_LISTEN_SOCKET: u32 = 18;
/// `Server::destroy` once there is a factory.
pub const RUN_DESTROY: u32 = 19;

// Reactor threads (`src/server/reactor_thread.cc`); context reactor and its thread.
/// `ReactorThread_onRead`: context event, socket and connection; arg the `conn_io` read facts.
pub const RUN_RT_READ: u32 = 20;
/// `ReactorThread_onPacketReceived`: context event.
pub const RUN_PACKET_RECEIVED: u32 = 21;
pub const RUN_THREAD_SHUTDOWN: u32 = 22;
/// `ReactorThread_onPipeRead`: context event.
pub const RUN_PIPE_READ: u32 = 24;
pub const RUN_INIT_REACTOR: u32 = 25;
/// `Server::heartbeat_check` in a reactor thread.
pub const RUN_HEARTBEAT_CHECK: u32 = 26;
pub const RUN_START_REACTOR_THREADS: u32 = 27;
/// `Server::reactor_thread_main_loop` for thread arg.
pub const RUN_REACTOR_THREAD_MAIN: u32 = 28;
/// `ReactorThread_resume_data_receiving`: context socket and connection.
pub const RUN_RESUME_RECEIVING: u32 = 29;
/// `Server::dispatch_task`: context socket, connection and received data.
pub const RUN_DISPATCH_TASK: u32 = 30;
pub const RUN_JOIN_REACTOR_THREADS: u32 = 31;
/// The heartbeat thread's loop.
pub const RUN_HEARTBEAT_THREAD: u32 = 32;

// Dispatch, sends and ports (`src/server/master.cc`).
/// `Server::schedule_worker(arg)`: context send data.
pub const RUN_SCHEDULE_WORKER: u32 = 33;
/// `Server::feedback`: context connection, arg the event.
pub const RUN_FEEDBACK: u32 = 34;
/// `Server::sendwait(arg)`: context data and length.
pub const RUN_SENDWAIT: u32 = 35;
pub const RUN_IDLE_WORKERS: u32 = 37;
pub const RUN_IDLE_TASK_WORKERS: u32 = 38;
pub const RUN_TASKING: u32 = 39;
/// `Server::sendfile(arg)`: context file name, offset and length.
pub const RUN_SENDFILE: u32 = 40;
/// `Server::add_port`: context type, host and port; 1 with the port in the context.
pub const RUN_ADD_PORT: u32 = 41;
pub const RUN_SYSTEMD_SOCKETS: u32 = 42;
// `BaseFactory` (`src/server/base.cc`, `crates/server/src/base_factory.rs`).
/// `Server::create_base_factory`: 1 with the factory in the context.
pub const RUN_BASE_CREATE: u32 = 43;
/// `BaseFactory::notify(arg)`: arg the `DataHead`.
pub const RUN_BASE_NOTIFY: u32 = 44;
/// `BaseFactory::end`: arg the session id, with the close flags from bit 48.
pub const RUN_BASE_END: u32 = 45;
/// `BaseFactory::finish(arg)`, `BaseFactory::dispatch(arg)`: arg and context the `SendData`.
pub const RUN_BASE_FINISH: u32 = 46;
pub const RUN_BASE_DISPATCH: u32 = 47;
/// One native op, arg the op: the operands are in the context (`server_methods.rs`).
pub const RUN_NATIVE_OP: u32 = 48;
/// A reactor thread's reactor is destroyed: close the connections it owns.
pub const RUN_CLOSE_REACTOR_CONNECTIONS: u32 = 49;

// Server facts.
pub const OP_MODE: u32 = 0;
pub const OP_SINGLE_THREAD: u32 = 1;
pub const OP_MAX_CONNECTION: u32 = 2;
/// `errno`.
pub const OP_ERRNO: u32 = 3;
/// `SwooleG.max_sockets`.
pub const OP_MAX_SOCKETS: u32 = 5;
pub const OP_SET_MAX_CONNECTION: u32 = 6;
pub const OP_DELAY_RECEIVE: u32 = 7;
/// `onConnect` is set.
pub const OP_HAS_ON_CONNECT: u32 = 8;
pub const OP_DISABLE_NOTIFY: u32 = 9;
pub const OP_IS_STARTED: u32 = 10;
/// `is_running()`.
pub const OP_RUNNING: u32 = 11;
pub const OP_SET_RUNNING: u32 = 12;
pub const OP_MASTER_PID: u32 = 13;
pub const OP_MANAGER_PID: u32 = 14;
/// `max_wait_time`.
pub const OP_MAX_WAIT_TIME: u32 = 17;
/// `SIGRTMIN`, or 0 where it is not defined.
pub const OP_SIGRTMIN: u32 = 18;
/// `SwooleG.signal_fd`.
pub const OP_SIGNAL_FD: u32 = 19;
pub const OP_MAXFD: u32 = 20;
pub const OP_MINFD: u32 = 21;
pub const OP_SET_MAXFD: u32 = 22;
pub const OP_SET_MINFD: u32 = 23;

// Listen ports, by index in `ports`.
pub const OP_PORT_COUNT: u32 = 30;
pub const OP_PORT_DGRAM: u32 = 31;
pub const OP_PORT_REMOVED: u32 = 34;

// The context's listen port.
/// `max_idle_time`, a `double`.
pub const OP_PORT_IDLE_TIME: u32 = 41;
/// The context has a port (`get_port_by_session_id` found one).
pub const OP_HAS_PORT: u32 = 45;
/// `heartbeat_idle_time`, a `double`.
pub const OP_PORT_HEARTBEAT: u32 = 46;

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
/// `close_force = 1` and the reactor's close event.
pub const OP_FORCE_CLOSE: u32 = 83;
/// `conn->last_recv_time`, a `double`.
pub const OP_CONN_LAST_RECV: u32 = 84;

// Timers, signals and shutdown.
/// `swoole_signal_set(arg, nullptr)` and `swoole_signal_set(arg, master_signal_handler)`.
pub const OP_SIGNAL_IGNORE: u32 = 105;
pub const OP_SIGNAL_HANDLE: u32 = 106;

// Startup and teardown (`RUN_CREATE`, `RUN_START_CHECK`, `RUN_START_MASTER_THREAD`,
// `RUN_STORE_LISTEN_SOCKET`, `RUN_DESTROY`).
/// A numeric server field (`FIELD_*`).
pub const OP_FIELD: u32 = 150;
/// Sets field `arg & 0xff` to `arg >> 8`.
pub const OP_SET_FIELD: u32 = 151;
/// `init_network_interface_addr_map()`.
pub const OP_INIT_LOCAL_ADDRS: u32 = 152;
/// The shared session list, port statistics (also given to each port) and workers.
pub const OP_ALLOC_SESSIONS: u32 = 153;
pub const OP_ALLOC_PORT_GS: u32 = 154;
pub const OP_ALLOC_WORKERS: u32 = 155;
/// The static handler's locations and the compression flag when types are set.
pub const OP_HTTP_DEFAULTS: u32 = 156;
/// The fd of the last listen port.
pub const OP_LAST_PORT_FD: u32 = 157;
/// The base, thread or process factory for the mode; whether there is one.
pub const OP_CREATE_FACTORY: u32 = 158;
/// `SW_GLOBAL_HOOK_AFTER_SERVER_CREATE`, when set.
pub const OP_AFTER_CREATE: u32 = 159;
/// Creates the task pool with `swIPCMode` arg; whether it succeeded.
pub const OP_TASK_POOL_CREATE: u32 = 160;
/// Its max request, start id and type.
pub const OP_TASK_POOL_SETUP: u32 = 161;
/// Listens on the NUL-terminated path at address arg; the `listen` result.
pub const OP_TASK_POOL_LISTEN: u32 = 162;
pub const OP_ALLOC_TASK_RESULTS: u32 = 163;
/// One task notify pipe; whether it is ready (when not, the task results are freed).
pub const OP_TASK_PIPE: u32 = 164;
pub const OP_INIT_TASK_WORKERS: u32 = 165;
/// Whether callback `CB_*` arg is set; clears it.
pub const OP_CALLBACK: u32 = 166;
pub const OP_CLEAR_CALLBACK: u32 = 167;
/// `disable_notify = true`, `send_yield = false`, `max_queued_bytes = 0`.
pub const OP_SET_DISABLE_NOTIFY: u32 = 168;
pub const OP_CLEAR_SEND_YIELD: u32 = 169;
pub const OP_CLEAR_MAX_QUEUED: u32 = 170;
/// `send_timeout`, a `double`; raising it to `SW_TIMER_MIN_SEC`.
pub const OP_SEND_TIMEOUT: u32 = 171;
pub const OP_SEND_TIMEOUT_MIN: u32 = 172;
/// Port arg: `heartbeat_idle_time`; set to twice the check interval.
pub const OP_PORT_IDLE: u32 = 173;
pub const OP_PORT_DEFAULT_IDLE: u32 = 174;
/// Port arg: `protocol.package_max_length`; raise to `SW_BUFFER_MIN_SIZE`.
pub const OP_PORT_MAX_LENGTH: u32 = 175;
pub const OP_PORT_MIN_LENGTH: u32 = 176;
pub const OP_PORT_STREAM: u32 = 177;
pub const OP_PORT_SSL_AT: u32 = 178;
pub const OP_PORT_WEBSOCKET: u32 = 179;
/// Unused: the port's WebSocket compression, which nothing read.
pub const OP_PORT_WS_COMPRESSION: u32 = 180;
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
/// Port arg into `connection_list` at its fd; returns the fd.
pub const OP_STORE_LISTEN: u32 = 196;
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

pub const LOG_ALLOC_SESSIONS: u32 = 220;
pub const LOG_ALLOC_PORT_GS: u32 = 221;
pub const LOG_ALLOC_WORKERS: u32 = 222;
/// arg the minimum in the high 32 bits and the new value in the low.
pub const LOG_MAX_CONNECTION_MINIMUM: u32 = 223;
pub const LOG_REACTOR_NUM: u32 = 224;
pub const LOG_WORKER_NUM: u32 = 225;
pub const LOG_TASK_WORKER_NUM: u32 = 226;
pub const LOG_TASK_POOL: u32 = 227;
pub const LOG_ALLOC_TASK_RESULTS: u32 = 228;
/// arg the `CB_*` that `dispatch_mode` does not allow.
pub const LOG_CALLBACK_DISPATCH: u32 = 229;
/// arg the `CB_*` the server requires.
pub const LOG_CALLBACK_REQUIRED: u32 = 230;

// Numeric server fields for `OP_FIELD`/`OP_SET_FIELD`; `CPU_NUM` is `SwooleG.cpu_num`.
pub const FIELD_REACTOR_NUM: u32 = 0;
pub const FIELD_WORKER_NUM: u32 = 1;
pub const FIELD_TASK_WORKER_NUM: u32 = 2;
pub const FIELD_HEARTBEAT_INTERVAL: u32 = 3;
pub const FIELD_DISPATCH_MODE: u32 = 4;
pub const FIELD_UNSAFE_EVENT: u32 = 5;
pub const FIELD_TASK_IPC_MODE: u32 = 6;
pub const FIELD_CPU_NUM: u32 = 7;

// Reactor threads. Ops 0-144 keep their meaning where master.cc's reactor-thread ops answer them.
/// `swReturnCode` of `socket->ssl_accept()`.
pub const OP_SSL_ACCEPT: u32 = 250;
/// Runs the port's `onRead` (stamping and counting the bytes); its result in the low 32
/// bits, with `conn_io`'s `ACTIVE`, `READ_NOTIFIED` and `QUEUE_OVER`.
pub const OP_RD_PORT_READ: u32 = 251;
/// `conn->ssl_ready = 1`.
pub const OP_SSL_READY: u32 = 252;
/// The port has a client certificate file; verifies peers; allows self-signed.
pub const OP_CLIENT_CERT: u32 = 253;
pub const OP_VERIFY_PEER: u32 = 254;
/// `ssl_get_peer_certificate` into the thread buffer; `ssl_verify`.
pub const OP_PEER_CERT: u32 = 255;
pub const OP_SSL_VERIFY: u32 = 256;
/// Fills the context's task with `SW_SERVER_EVENT_CONNECT` and the peer
/// certificate, for `OP_FACTORY_DISPATCH`.
pub const OP_CERT_TASK: u32 = 257;
/// `reactor->del(socket)`.
pub const OP_REACTOR_DEL: u32 = 258;
/// Fills the datagram task header.
pub const OP_PACKET_INIT: u32 = 259;
/// `recvfrom` into the packet; the `ssize_t`.
pub const OP_RECVFROM: u32 = 260;
/// The datagram's port is DTLS; accepts the DTLS session (arg the length) and returns
/// `onPacketReceived`'s result.
pub const OP_PORT_DTLS: u32 = 261;
pub const OP_DTLS_ACCEPT: u32 = 262;
/// Fills the task for the datagram of arg bytes; the task's address.
pub const OP_PACKET_TASK: u32 = 263;
pub const OP_HAVE_DGRAM: u32 = 264;
/// Port arg's fd.
pub const OP_PORT_FD: u32 = 265;
/// `reactor->id`.
pub const OP_REACTOR_ID: u32 = 266;
pub const OP_REACTOR_NUM: u32 = 267;
/// `reactor->del(port->socket)` for port arg.
pub const OP_PORT_DEL: u32 = 268;
pub const OP_HEARTBEAT_TIMER_CLEAR: u32 = 269;
/// Closes the reactor's connections when it is destroyed.
pub const OP_DESTROY_CALLBACK: u32 = 270;
/// `stop_async_worker(get_worker(reactor->id))`.
pub const OP_STOP_THREAD_WORKER: u32 = 271;
pub const OP_WORKER_NUM: u32 = 272;
/// Stops reading worker arg's master pipe.
pub const OP_WORKER_PIPE_UNREAD: u32 = 273;
/// Selects the connection at fd arg when valid.
pub const OP_CONN_AT: u32 = 274;
pub const OP_CONN_FD: u32 = 275;
pub const OP_CONN_REACTOR: u32 = 276;
pub const OP_CONN_SESSION: u32 = 277;
/// Neither the peer closed nor the socket left the reactor.
pub const OP_CONN_READING: u32 = 278;
pub const OP_CONN_UNREAD: u32 = 279;
pub const OP_WAIT_EXIT: u32 = 280;
/// Selects the connection of session arg (`get_connection_verify_no_ssl`).
pub const OP_SESSION_CONN: u32 = 281;
pub const OP_CONN_FORCE: u32 = 282;
/// `conn->ssl && !conn->ssl_ready`.
pub const OP_CONN_HANDSHAKING: u32 = 283;
/// `Server::close_connection(reactor, conn->socket)`.
pub const OP_CLOSE: u32 = 284;
/// Reads the pipe into the thread's message bus; the `ssize_t`.
pub const OP_PIPE_READ: u32 = 285;
pub const OP_PIPE_TYPE: u32 = 286;
pub const OP_PIPE_FD: u32 = 287;
pub const OP_PIPE_POP: u32 = 288;
/// `connection_incoming`; `reactor->close` of the connection.
pub const OP_INCOMING: u32 = 289;
pub const OP_REACTOR_CLOSE: u32 = 290;
pub const OP_COMMAND_REQUEST: u32 = 291;
pub const OP_COMMAND_RESPONSE: u32 = 292;
/// `swoole_kill(getpid(), SIGTERM)`.
pub const OP_KILL_SELF: u32 = 293;
pub const OP_ON_FINISH: u32 = 294;
pub const OP_ON_PIPE_MESSAGE: u32 = 295;
/// `factory_->end(arg, CLOSE_ACTIVELY)`.
pub const OP_CLOSE_FORWARD: u32 = 296;
/// `send_to_connection` of the pipe's packet.
pub const OP_PIPE_SEND: u32 = 297;
/// The thread buffer grows for 64K datagrams, then the session handlers.
pub const OP_DGRAM_BUFFER: u32 = 298;
pub const OP_SESSION_HANDLERS: u32 = 299;
/// `microtime()`, a `double`.
pub const OP_MICROTIME: u32 = 300;
/// `is_healthy_connection(arg as double)` of the connection.
pub const OP_CONN_HEALTHY: u32 = 301;
/// `swoole_event_init(0)`; the context reactor becomes `sw_reactor()`.
pub const OP_EVENT_INIT: u32 = 302;
/// Port arg listens; then reads.
pub const OP_PORT_LISTEN: u32 = 303;
pub const OP_PORT_READ: u32 = 304;
pub const OP_STORE_LISTEN_SOCKET: u32 = 305;
/// `get_thread(0)->init(serv, reactor, 0)`.
pub const OP_THREAD0_INIT: u32 = 306;
/// The master reactor's and thread's id become `reactor_num`.
pub const OP_MASTER_REACTOR_ID: u32 = 307;
/// The entry a reactor thread runs: `void (*)(void *serv, uint64_t id)`.
pub const OP_REACTOR_THREAD_ENTRY: u32 = 308;
pub const OP_HEARTBEAT_TIMER: u32 = 309;
/// The entry the heartbeat thread runs, as `OP_REACTOR_THREAD_ENTRY`'s.
pub const OP_HEARTBEAT_ENTRY: u32 = 310;
pub const OP_START_MASTER_THREAD: u32 = 311;
/// The context thread is reactor thread arg: its id and the thread's message bus.
pub const OP_THREAD_ENTER: u32 = 312;
pub const OP_WORKER_START: u32 = 313;
pub const OP_WORKER_STOP: u32 = 314;
/// `swoole_set_thread_id(arg)`, `THREAD_REACTOR`.
pub const OP_REACTOR_THREAD_TYPE: u32 = 315;
/// `thread->init(serv, reactor, id)`.
pub const OP_THREAD_INIT: u32 = 316;
pub const OP_THREAD_CLEAN: u32 = 317;
/// `recv_queued_bytes > max_queued_bytes`.
pub const OP_QUEUE_FULL: u32 = 318;
pub const OP_WAITING_TIME: u32 = 319;
pub const OP_SET_WAITING_TIME: u32 = 320;
/// Arms the resume timer after the waiting time; whether it was armed.
pub const OP_RESUME_TIMER: u32 = 321;
/// The reactor reads the socket again and the timer is cleared.
pub const OP_RESUME_READ: u32 = 322;
/// `reactor->remove_read_event(socket)`.
pub const OP_RD_UNREAD: u32 = 324;
/// `recv_queued_bytes` -= arg.
pub const OP_QUEUE_SUB: u32 = 325;
pub const OP_FACTORY_DISPATCH: u32 = 326;
/// The `Server *` the thread entries take.
pub const OP_SERVER: u32 = 328;
/// Tells reactor thread arg to shut down.
pub const OP_THREAD_NOTIFY: u32 = 329;
/// Asks the connection's reactor thread to force-close session arg.
pub const OP_SEND_CLOSE_FORCE: u32 = 330;

pub const LOG_RECVFROM: u32 = 350;
/// arg the session.
pub const LOG_NO_SESSION: u32 = 351;

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
/// A `dispatch_func` is set; its result for the context's fd.
pub const OP_HAS_DISPATCH_FUNC: u32 = 400;
pub const OP_DISPATCH_FUNC: u32 = 401;
/// `Server::state_`, a `state::ServerState`.
pub const OP_STATE: u32 = 402;
/// The IP hash key of the connection or datagram peer.
pub const OP_IP_KEY: u32 = 403;
/// Selects `get_connection(fd)`; whether there is one.
pub const OP_FD_CONN: u32 = 404;
pub const OP_CONN_UID: u32 = 405;
pub const OP_CONN_WORKER: u32 = 406;
/// Pins the connection to worker arg.
pub const OP_PIN_WORKER: u32 = 407;
/// Worker arg's `coroutine_num` and `concurrency`.
pub const OP_WORKER_COROUTINES: u32 = 408;
pub const OP_WORKER_CONCURRENCY: u32 = 409;
/// `scheduler_warning = true`.
pub const OP_SET_WARNING: u32 = 410;
/// The event arg to the connection's reactor thread; whether it was sent.
pub const OP_FEEDBACK_THREAD: u32 = 411;
/// `send_to_connection` of the event arg; whether it succeeded.
pub const OP_FEEDBACK_SEND: u32 = 412;
/// Selects `get_connection_verify(arg)`; whether there is one.
pub const OP_VERIFY_CONN: u32 = 413;
/// `send_sync` of the context data; whether all of it was sent.
pub const OP_SEND_SYNC: u32 = 414;
pub const OP_WORKER_IDLE: u32 = 418;
/// `gs->tasking_num`, an `int`; reset it to 0.
pub const OP_TASKING: u32 = 419;
pub const OP_TASKING_RESET: u32 = 420;
pub const OP_IS_MASTER: u32 = 421;
/// The longest sendfile name; copies the name into the task, returning its address.
pub const OP_SENDFILE_NAME_MAX: u32 = 422;
pub const OP_SENDFILE_NAME: u32 = 423;
/// Finishes the sendfile task of arg bytes; whether the factory took it.
pub const OP_SENDFILE_FINISH: u32 = 424;
pub const OP_IS_CREATED: u32 = 425;
pub const OP_PORT_TYPE: u32 = 426;
pub const OP_PORT_NUMBER: u32 = 427;
/// `strlen(host) + 1` and `SW_HOST_MAXSIZE`.
pub const OP_HOST_SIZE: u32 = 428;
pub const OP_HOST_MAXSIZE: u32 = 429;
/// A new `ListenPort` of the context's type, host and port, with TLS when arg.
pub const OP_PORT_NEW: u32 = 430;
/// `create_socket()`; on failure `swoole_set_last_error(errno)` and the port is freed.
pub const OP_PORT_SOCKET: u32 = 431;
/// The new port's socket type; the dgram socket buffer; the socket for kind arg.
pub const OP_NEW_PORT_TYPE: u32 = 432;
pub const OP_DGRAM_PORT: u32 = 433;
pub const OP_STREAM_PORT: u32 = 434;
/// The new port joins `ports`.
pub const OP_PORT_PUSH: u32 = 435;
/// `LISTEN_PID`, `LISTEN_FDS_START`: the value, or `i64::MIN` when unset.
pub const OP_ENV_LISTEN_PID: u32 = 436;
pub const OP_ENV_FDS_START: u32 = 437;
/// The sendfile name's length and the offset, an `off_t`.
pub const OP_SENDFILE_NAME_LEN: u32 = 438;
pub const OP_SENDFILE_OFFSET: u32 = 441;
/// `swoole_get_systemd_listen_fds()`.
pub const OP_SYSTEMD_FDS: u32 = 439;
/// A new port importing fd arg, non-blocking and close-on-exec; whether it imported.
pub const OP_PORT_IMPORT: u32 = 440;

pub const LOG_INVALID_SESSION: u32 = 450;
pub const LOG_SEND_IN_MASTER: u32 = 451;
pub const LOG_SENDFILE_NAME: u32 = 452;
pub const LOG_STAT: u32 = 453;
pub const LOG_NOT_REGULAR: u32 = 454;
pub const LOG_FILE_EMPTY: u32 = 455;
pub const LOG_SENDWAIT: u32 = 456;
pub const LOG_PORT_CREATED: u32 = 457;
pub const LOG_TOO_MANY_PORTS: u32 = 458;
pub const LOG_INVALID_PORT: u32 = 459;
pub const LOG_HOST_TOO_LONG: u32 = 460;
pub const LOG_LISTEN_PID: u32 = 461;
pub const LOG_FDS_START: u32 = 462;

// `BaseFactory`.
/// `reactor_num = worker_num`, then the connection list and the per-worker counters.
pub const OP_BASE_REACTORS: u32 = 500;
pub const OP_ALLOC_CONNECTIONS: u32 = 501;
pub const OP_ALLOC_CONNECTION_NUMS: u32 = 502;
pub const OP_ALLOC_PORT_CONNECTION_NUMS: u32 = 503;
pub const OP_NEW_BASE_FACTORY: u32 = 504;
/// `swoole_get_worker_id()`.
pub const OP_WORKER_ID: u32 = 505;
/// `worker_accept_event` of the `DataHead` at arg.
pub const OP_ACCEPT_EVENT: u32 = 506;
/// `onReceive` of the `RecvData` at arg: whether it returned `SW_OK`.
pub const OP_ON_RECEIVE: u32 = 507;
/// The worker message bus takes the `SendData` at arg, then `worker_accept_event` of it.
pub const OP_BASE_PASS: u32 = 508;
/// The `SendData` at arg written to the pipe of its session's worker; whether it was.
pub const OP_FORWARD: u32 = 509;
/// `onClose` is set; it runs with the `DataHead` at arg.
pub const OP_HAS_ON_CLOSE: u32 = 510;
pub const OP_ON_CLOSE: u32 = 511;
/// The output buffer of the socket at arg is empty; `close_connection` of it, the result;
/// a close chunk queued behind its output, then (`--fix-halfclose`) its read event removed:
/// whether that succeeded.
pub const OP_OUT_EMPTY: u32 = 512;
pub const OP_CLOSE_SOCKET: u32 = 513;
pub const OP_QUEUE_CLOSE: u32 = 514;
/// `send_to_connection` of the context send, the result.
pub const OP_SEND_HERE: u32 = 515;
/// `onPacket` of the `RecvData` at arg: whether it returned `SW_OK`.
pub const OP_ON_PACKET: u32 = 516;

pub const LOG_BASE_ALLOC_CONNECTIONS: u32 = 530;
pub const LOG_BASE_ALLOC_NUMS: u32 = 531;
pub const LOG_BASE_ALLOC_PORT_NUMS: u32 = 532;

// Reactor threads' close, peer-close and pipe-write paths and thread setup
// (`crates/server/src/reactor_io.rs`); far from the other ranges.
pub const RUN_RT_PIPE_WRITE: u32 = 200;
pub const RUN_RT_INIT: u32 = 201;
pub const OP_RT_BUF_EMPTY: u32 = 900;
pub const OP_RT_FRONT: u32 = 901;
pub const OP_RT_FRONT_STREAM: u32 = 902;
pub const OP_RT_FRONT_CONN: u32 = 903;
pub const OP_RT_TOUCH_SEND: u32 = 904;
pub const OP_RT_FRONT_CONN_CLOSED: u32 = 905;
pub const LOG_RT_CLOSED_BY_SERVER: u32 = 906;
pub const OP_RT_DISCARD_TIMEOUT: u32 = 907;
pub const LOG_RT_DISCARD: u32 = 908;
pub const OP_RT_POP: u32 = 909;
pub const OP_RT_SEND_FRONT: u32 = 910;
pub const OP_RT_CATCH_WAIT: u32 = 911;
pub const OP_RT_REMOVE_WRITE: u32 = 912;
pub const LOG_RT_SET_WRITE: u32 = 913;
pub const OP_RT_SETUP: u32 = 914;
pub const OP_RT_DGRAM_BIND: u32 = 915;
pub const OP_RT_INIT_REACTOR: u32 = 916;
pub const OP_RT_INIT_PIPE_SOCKETS: u32 = 917;
pub const OP_RT_THREAD_WORKER: u32 = 918;
pub const OP_RT_HEARTBEAT_TIMER: u32 = 919;
pub const OP_RT_HAS_PIPE_COMMAND: u32 = 920;
pub const OP_RT_PIPE_COMMAND: u32 = 921;
pub const OP_RT_BUS_SETUP: u32 = 922;
pub const OP_RT_ADD_WORKER_PIPE: u32 = 923;

// A connection's read, write and close paths (`crates/server/src/conn_io.rs`); the
// context holds the reactor, event, socket, connection and port.
/// `ReactorThread_onWrite`: arg the write facts.
pub const RUN_RT_WRITE: u32 = 211;
/// `ReactorThread_onClose`: arg the peer-close facts.
pub const RUN_PEER_CLOSE: u32 = 212;
/// `Server::close_connection`: context reactor and socket.
pub const RUN_CLOSE_CONNECTION: u32 = 213;
/// Feeds a DTLS port's session the datagram; 1 when its handshake failed.
pub const OP_RD_DTLS: u32 = 940;
/// `serv->abort_connection(reactor, port, socket)`.
pub const OP_RD_ABORT: u32 = 941;
/// `ReactorThread_onWrite`'s facts (and context port and connection), for a read.
pub const OP_RD_WRITE_FACTS: u32 = 942;
/// Notifies the close and clears `close_notify`.
pub const OP_WR_NOTIFY_CLOSE: u32 = 943;
/// 0 when the output buffer is empty, else 1 + its front chunk's type.
pub const OP_WR_FRONT: u32 = 944;
/// `reactor->close(reactor, socket)`.
pub const OP_WR_CLOSE: u32 = 945;
pub const OP_WR_SENDFILE: u32 = 946;
pub const OP_WR_SEND: u32 = 947;
/// `send_queued_bytes` = the output buffer's length.
pub const OP_WR_QUEUED: u32 = 948;
/// `CLOSE_WAIT` and `SEND_WAIT`.
pub const OP_WR_WAIT: u32 = 949;
/// `close_errno = errno`, then the close event's result.
pub const OP_WR_CLOSE_ERRNO: u32 = 950;
/// The flushed connection's `OVERFLOW_DRAINED`, `LOW_WATERMARK`, `SEND_TIMER`,
/// `OUT_EMPTY`, `PEER_CLOSED` and `REMOVED`.
pub const OP_WR_STATE: u32 = 951;
pub const OP_WR_CLEAR_OVERFLOW: u32 = 952;
/// Clears `high_watermark` and notifies the buffer is empty.
pub const OP_WR_BUFFER_EMPTY: u32 = 953;
pub const OP_WR_TIMER_DEL: u32 = 954;
/// Delays the send timer by the port's idle time.
pub const OP_WR_TIMER_DELAY: u32 = 955;
/// `reactor->set(socket, SW_EVENT_READ)`.
pub const OP_WR_READ_ONLY: u32 = 956;
/// `peer_closed = arg`, `close_notified = 1`, then the factory's close notification.
pub const OP_PC_NOTIFY: u32 = 957;
/// `Server::close_connection`'s facts, the fd above bit 32 (and context connection and port).
pub const OP_CC_FACTS: u32 = 958;
pub const OP_CC_TIMER_DEL: u32 = 959;
/// `socket->ssl_shutdown()`.
pub const OP_CC_SSL_SHUTDOWN: u32 = 960;
/// TLS wants to read or write.
pub const OP_CC_SSL_WANT: u32 = 961;
/// Queues a close chunk and sets `close_queued`.
pub const OP_CC_QUEUE_CLOSE: u32 = 962;
/// Watches the socket for writes, adding it when arg (it is removed); the result.
pub const OP_CC_WATCH_WRITE: u32 = 963;
pub const OP_CC_WEBSOCKET_FREE: u32 = 964;
/// Close counts and connection numbers, per reactor when arg (base mode); traced.
pub const OP_CC_COUNT: u32 = 965;
/// `ssl_quiet_shutdown = peer_closed`, then `ssl_close()`.
pub const OP_CC_SSL_CLOSE: u32 = 966;
pub const OP_CC_DTLS_FREE: u32 = 967;
pub const OP_CC_RECV_FREE: u32 = 968;
/// The port speaks HTTP and the connection holds a request.
pub const OP_CC_HTTP_OBJECT: u32 = 969;
pub const OP_CC_HTTP_DESTROY: u32 = 970;
/// The port speaks Redis and the connection holds an object.
pub const OP_CC_REDIS_OBJECT: u32 = 971;
pub const OP_CC_REDIS_FREE: u32 = 972;
/// Sets `SO_LINGER` to reset; 1 when it failed.
pub const OP_CC_LINGER: u32 = 973;
/// The session's fd = 0.
pub const OP_CC_SESSION_CLEAR: u32 = 974;
/// The connection at fd arg is valid.
pub const OP_CC_VALID: u32 = 975;
/// Marks the connection closed, then `Reactor::_close`'s result.
pub const OP_CC_FINISH: u32 = 976;
pub const LOG_CC_LINGER: u32 = 990;

// Rust-owned state and the rest of `master.cc` (`crates/server/src/state.rs`).
/// `Server::add_command`: arg a `commands::AddCommand`.
pub const RUN_ST_ADD_COMMAND: u32 = 300;
/// `Server::send`: context session, data and length.
pub const RUN_ST_SEND: u32 = 308;
/// `Server::command`: arg a `commands::SendCommand`.
pub const RUN_ST_COMMAND: u32 = 312;

/// Writes the command request at arg (a `commands::CommandSend`) to its process;
/// whether it went. A new command pipe, whether it is ready.
pub const OP_ST_CMD_SEND: u32 = 1000;
pub const OP_ST_PIPE_COMMAND_NEW: u32 = 1001;
/// `gs->concurrency`: subtract arg; read; set 0.
pub const OP_ST_GS_CONCURRENCY_SUB: u32 = 1020;
pub const OP_ST_GS_CONCURRENCY: u32 = 1021;
pub const OP_ST_GS_CONCURRENCY_ZERO: u32 = 1022;
/// `factory_->finish` of the send.
pub const OP_ST_FINISH: u32 = 1030;

// `Server::send_to_connection` (`crates/server/src/send.rs`); context send, data and length.
/// arg the send's type.
pub const RUN_SC_SEND: u32 = 310;
/// Selects `get_connection_verify` (`get_connection_verify_no_ssl` for a close) and, when
/// found, the reactor, port and socket of the connection, asserting the thread owns it: 0, or
/// `send::FOUND` with the `OP_SC_FLAGS` and `send::OPEN` bits and the send's length above bit 32.
pub const OP_SC_VERIFY: u32 = 1040;
/// The connection's and socket's `send::*` flag bits.
pub const OP_SC_FLAGS: u32 = 1041;
/// `swoole_set_last_error(SW_ERROR_OUTPUT_SEND_YIELD)`.
pub const OP_SC_YIELD_ERROR: u32 = 1042;
/// `reactor->close(reactor, socket)`.
pub const OP_SC_CLOSE: u32 = 1043;
/// `reactor->set(socket, arg)`, `del`, `add(socket, arg)`, with `send::READ`/`WRITE` bits.
pub const OP_SC_SET: u32 = 1044;
pub const OP_SC_DEL: u32 = 1045;
pub const OP_SC_ADD: u32 = 1046;
/// `reactor->remove_read_event(socket)`, `add_read_event(socket)`.
pub const OP_SC_REMOVE_READ: u32 = 1047;
pub const OP_SC_ADD_READ: u32 = 1048;
/// `send` of the bytes left, the result; skip arg sent bytes.
pub const OP_SC_SEND: u32 = 1049;
pub const OP_SC_ADVANCE: u32 = 1050;
/// `last_send_time = microtime()`.
pub const OP_SC_TOUCH_SEND: u32 = 1051;
/// An output buffer when there is none.
pub const OP_SC_ENSURE_BUFFER: u32 = 1052;
/// A close chunk queued and `close_queued = 1`.
pub const OP_SC_QUEUE_CLOSE: u32 = 1053;
/// `sendfile_async` of the task; its result.
pub const OP_SC_SENDFILE: u32 = 1054;
/// The output buffer holds `buffer_size` or more; `overflow = 1`; `high_watermark = 1`.
pub const OP_SC_OUT_FULL: u32 = 1055;
pub const OP_SC_OVERFLOW: u32 = 1056;
pub const OP_SC_HIGH_WATERMARK: u32 = 1057;
/// The data appended and `send_queued_bytes` updated; the buffer reached the port's
/// high watermark; `SW_SERVER_EVENT_BUFFER_FULL` notified and `high_watermark = 1`.
pub const OP_SC_APPEND: u32 = 1058;
pub const OP_SC_OUT_HIGH: u32 = 1059;
pub const OP_SC_NOTIFY_FULL: u32 = 1060;
/// The socket has a send timer; one for the port's idle time is added.
pub const OP_SC_HAS_SEND_TIMER: u32 = 1061;
pub const OP_SC_IDLE_TIMER: u32 = 1062;
/// The socket has its writable event; it is added.
pub const OP_SC_WRITABLE: u32 = 1063;
pub const OP_SC_ADD_WRITE: u32 = 1064;

pub const LOG_SC_NO_SESSION_DATA: u32 = 1110;
pub const LOG_SC_NO_SESSION_EVENT: u32 = 1111;
pub const LOG_SC_SOCKET_OVERFLOW: u32 = 1112;
pub const LOG_SC_CLOSED_BY_CLIENT: u32 = 1113;
pub const LOG_SC_CONN_OVERFLOW: u32 = 1114;

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
