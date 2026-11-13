//! Operation (`RUN_*`) and op (`OP_*`, `LOG_*`) numbers between crates/process's
//! operations and crates/server, which answers each op.

/// `swoole::ServerEventType`; native asserts each.
pub const EVENT_RECV_DATA: u32 = 0;
pub const EVENT_RECV_DGRAM: u32 = 1;
pub const EVENT_SEND_DATA: u32 = 2;
pub const EVENT_SEND_FILE: u32 = 3;
pub const EVENT_CLOSE: u32 = 4;
pub const EVENT_CONNECT: u32 = 5;
pub const EVENT_CLOSE_FORWARD: u32 = 7;
pub const EVENT_FINISH: u32 = 9;
pub const EVENT_PIPE_MESSAGE: u32 = 10;
pub const EVENT_BUFFER_FULL: u32 = 13;
pub const EVENT_BUFFER_EMPTY: u32 = 14;
pub const EVENT_SHUTDOWN: u32 = 16;
pub const EVENT_COMMAND_REQUEST: u32 = 17;
pub const EVENT_COMMAND_RESPONSE: u32 = 18;

/// `OP_WORKER_STATUS`'s argument.
/// `OP_DO_TASK`'s argument: the callback that gets the received data.
/// `OP_GLOBAL_HOOK`'s argument.
pub const HOOK_BEFORE_WORKER_START: u64 = 0;
pub const HOOK_BEFORE_WORKER_STOP: u64 = 1;
pub const HOOK_BEFORE_SERVER_SHUTDOWN: u64 = 2;

pub const RUN_RELOAD: u32 = 32;
pub const RUN_THREAD_MANAGER_WAIT: u32 = 45;
pub const RUN_THREAD_RELOAD: u32 = 46;
pub const RUN_THREAD_TERMINATE_MANAGER: u32 = 47;
pub const RUN_START_WORKER_THREADS: u32 = 48;
pub const RUN_STOP_WORKER_THREADS: u32 = 49;
pub const RUN_THREAD_BODY: u32 = 50;
pub const RUN_THREAD_MAIN: u32 = 51;
pub const RUN_THREAD_FACTORY_START: u32 = 52;
pub const RUN_CREATE_THREAD_FACTORY: u32 = 54;
pub const RUN_DESTROY_THREAD_FACTORY: u32 = 55;
pub const RUN_THREAD_ID: u32 = 56;
pub const OP_WORKER_TYPE: u32 = 3;
pub const OP_ALL_CORE_WORKER_NUM: u32 = 6;
pub const OP_EVENT_AVAILABLE: u32 = 10;
pub const OP_WORKER_SHUTDOWN: u32 = 12;
pub const OP_REOPEN_LOGGER: u32 = 13;
pub const OP_FREE_CLIENT_CERT: u32 = 21;
pub const OP_STORE_CLIENT_CERT: u32 = 24;
pub const OP_ROOT_USER: u32 = 34;
pub const OP_WORKER_ID: u32 = 36;
pub const OP_ON_WORKER_START: u32 = 44;
pub const OP_ON_WORKER_STOP: u32 = 45;
pub const OP_PORT_COUNT: u32 = 66;
pub const OP_WORKER_NUM: u32 = 75;
pub const OP_ABORT: u32 = 78;
pub const OP_EVENT_INIT: u32 = 91;
pub const OP_UNLIMIT_PIPES: u32 = 93;
pub const OP_EVENT_FREE: u32 = 100;
pub const OP_PORT_CLOSE_SOCKET: u32 = 111;
pub const OP_PORT_LISTEN: u32 = 112;
pub const OP_ALL_WORKER_NUM: u32 = 122;
pub const OP_PORT_ADD: u32 = 132;
pub const OP_STORE_LISTEN_SOCKET: u32 = 134;
pub const OP_TASK_WORKER_NUM: u32 = 137;
pub const OP_WORKER_INIT: u32 = 180;
pub const OP_EVENT_WAIT: u32 = 186;
pub const OP_CREATE_EVENT_WORKERS: u32 = 215;
pub const OP_USER_WORKER_NUM: u32 = 218;
pub const OP_CREATE_USER_WORKERS: u32 = 219;
pub const OP_DEATH_SIGNAL: u32 = 231;
pub const OP_ON_MANAGER_START: u32 = 235;
pub const OP_TIMER_AVAILABLE: u32 = 239;
pub const OP_TIMER_FREE: u32 = 240;
/// The server-owned Rust `WorkerRegistry`, lent to the manager while it runs.
pub const OP_ON_MANAGER_STOP: u32 = 248;
pub const OP_RUNNING: u32 = 249;
/// Whether a worker thread asked the server to shut down (`OP_SET_STOPPING`).
pub const OP_STOPPING: u32 = 1216;
pub const OP_SIGNAL_DISPATCH: u32 = 255;
pub const OP_POP_MESSAGE: u32 = 257;
pub const OP_ON_WORKER_MESSAGE: u32 = 260;
pub const OP_HAS_TIMER: u32 = 265;
pub const OP_TIMER_SELECT: u32 = 266;
pub const OP_ON_BEFORE_RELOAD: u32 = 270;
pub const OP_ON_AFTER_RELOAD: u32 = 276;
pub const OP_IS_MANAGER: u32 = 278;
pub const OP_DELETE_REACTOR_THREADS: u32 = 297;
pub const OP_CALL_WORKER_ERROR: u32 = 302;
pub const OP_NEW_REACTOR_THREADS: u32 = 307;
pub const OP_FORK: u32 = 316;
pub const OP_POOL_MAIN_LOOP: u32 = 320;
pub const OP_ON_USER_WORKER_START: u32 = 326;
pub const OP_THREAD_MANAGER: u32 = 369;
pub const OP_JOIN_THREAD: u32 = 370;
pub const OP_MANAGER_THREAD_ID: u32 = 371;
pub const OP_TERMINATE_MANAGER_THREAD: u32 = 373;
pub const OP_KILL_WORKER: u32 = 374;
pub const OP_TIMER_NEXT_MSEC: u32 = 375;
pub const OP_SELECT_WORKER: u32 = 377;
pub const OP_THREAD_EXIT_STATUS: u32 = 378;
pub const OP_SET_NOT_SUPPORTED: u32 = 383;
pub const OP_PORT_DGRAM: u32 = 386;
pub const OP_SET_MASTER_THREAD_ID: u32 = 387;
pub const OP_START_MASTER_THREAD: u32 = 388;
/// `Server::worker_accept_event`'s ends: the worker is busy, the event's type; the worker
/// is idle, whether it exceeded its maximum request count.
pub const OP_SPAWN_THREAD: u32 = 391;
pub const OP_THREAD_ENTER: u32 = 392;
pub const OP_THREAD_EXIT: u32 = 393;
pub const OP_CREATE_MESSAGE_BUS: u32 = 394;
pub const OP_DESTROY_MESSAGE_BUS: u32 = 395;
pub const OP_GET_WORKER: u32 = 396;
pub const OP_WORKER_THREAD_START: u32 = 397;
pub const OP_REACTOR_THREAD_MAIN: u32 = 398;
pub const OP_TIMER_CREATE: u32 = 399;
pub const OP_CREATE_WORKER_PIPES: u32 = 401;
pub const OP_TASK_START_CHECK: u32 = 402;
pub const OP_THREAD_SUPPORTED: u32 = 405;
pub const OP_THREAD_REACTOR_NUM: u32 = 407;
pub const OP_CALLOC_CONNECTIONS: u32 = 408;
pub const LOG_CALLOC_CONNECTIONS: u32 = 409;
pub const OP_SINGLE_REACTOR_PIPE: u32 = 410;
pub const OP_NEW_THREAD_FACTORY: u32 = 411;
pub const OP_FREE_THREAD_CONNECTIONS: u32 = 412;
