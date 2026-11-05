#ifndef SWOOLE_RS_H
#define SWOOLE_RS_H

#include <stdbool.h>
#include <stddef.h>
#include <stdint.h>

#define SWR_ABI_VERSION 1

#ifdef __cplusplus
extern "C" {
#endif

/* Stateless, allocation-free queries; safe on any thread, including after fork. */
uint32_t swr_v1_abi_version(void);
uint64_t swr_v1_feature_bits(void);

/*
 * swoole::Server operations (crates/server). Rust decides each step and asks native
 * code for every fact or effect through call(context, op, arg); the numbers are
 * crates/server/src/ops.rs.
 */
typedef uint64_t (*swr_v1_server_call)(void *context, uint32_t op, uint64_t arg);
int64_t swr_v1_server_run(uint32_t operation, uint64_t arg, void *context, swr_v1_server_call call);
struct swr_v1_server_state;
int64_t swr_v1_server_run_state(uint32_t operation, uint64_t arg, void *context, swr_v1_server_call call, const struct swr_v1_server_state *state);

enum {
    SWR_SERVER_RUN_ACCEPT, SWR_SERVER_RUN_ADD_CONNECTION, SWR_SERVER_RUN_INCOMING,
    SWR_SERVER_RUN_IDLE_TIMEOUT, SWR_SERVER_RUN_DISABLE_ACCEPT, SWR_SERVER_RUN_ENABLE_ACCEPT,
    SWR_SERVER_RUN_SET_MAX_CONNECTION, SWR_SERVER_RUN_HEALTHY, SWR_SERVER_RUN_SHUTDOWN,
    SWR_SERVER_RUN_SIGNAL, SWR_SERVER_RUN_SIGNAL_SHUTDOWN, SWR_SERVER_RUN_CHILD_EXIT,
    SWR_SERVER_RUN_STOP_MASTER_THREAD, SWR_SERVER_RUN_MASTER_TIMER, SWR_SERVER_RUN_INIT_SIGNALS,
    SWR_SERVER_RUN_CREATE, SWR_SERVER_RUN_START_CHECK, SWR_SERVER_RUN_START_MASTER_THREAD,
    SWR_SERVER_RUN_STORE_LISTEN_SOCKET, SWR_SERVER_RUN_DESTROY,
    SWR_SERVER_RUN_RT_READ, SWR_SERVER_RUN_PACKET_RECEIVED, SWR_SERVER_RUN_THREAD_SHUTDOWN,
    SWR_SERVER_RUN_THREAD_CLOSE, SWR_SERVER_RUN_PIPE_READ, SWR_SERVER_RUN_INIT_REACTOR,
    SWR_SERVER_RUN_HEARTBEAT_CHECK, SWR_SERVER_RUN_START_REACTOR_THREADS, SWR_SERVER_RUN_REACTOR_THREAD_MAIN,
    SWR_SERVER_RUN_RESUME_RECEIVING, SWR_SERVER_RUN_DISPATCH_TASK, SWR_SERVER_RUN_JOIN_REACTOR_THREADS,
    SWR_SERVER_RUN_HEARTBEAT_THREAD, SWR_SERVER_RUN_SCHEDULE_WORKER, SWR_SERVER_RUN_FEEDBACK,
    SWR_SERVER_RUN_SENDWAIT, SWR_SERVER_RUN_KERNEL_NOBUFS, SWR_SERVER_RUN_IDLE_WORKERS,
    SWR_SERVER_RUN_IDLE_TASK_WORKERS, SWR_SERVER_RUN_TASKING, SWR_SERVER_RUN_SENDFILE, SWR_SERVER_RUN_ADD_PORT,
    SWR_SERVER_RUN_SYSTEMD_SOCKETS, SWR_SERVER_RUN_BASE_CREATE, SWR_SERVER_RUN_BASE_NOTIFY,
    SWR_SERVER_RUN_BASE_END, SWR_SERVER_RUN_BASE_FINISH,
};

enum { SWR_SERVER_END_CLOSE = 2 };

enum {
    SWR_SERVER_EVENT_CLOSE_FORCE = 6, SWR_SERVER_EVENT_CLOSE_FORWARD = 7, SWR_SERVER_EVENT_FINISH = 9,
    SWR_SERVER_EVENT_PIPE_MESSAGE = 10, SWR_SERVER_EVENT_INCOMING = 15, SWR_SERVER_EVENT_SHUTDOWN = 16,
    SWR_SERVER_EVENT_COMMAND_REQUEST = 17, SWR_SERVER_EVENT_COMMAND_RESPONSE = 18,
    SWR_SERVER_EVENT_SHUTDOWN_SIGNAL = 19,
};

enum {
    SWR_SERVER_FIELD_REACTOR_NUM, SWR_SERVER_FIELD_WORKER_NUM, SWR_SERVER_FIELD_TASK_WORKER_NUM,
    SWR_SERVER_FIELD_HEARTBEAT_INTERVAL, SWR_SERVER_FIELD_DISPATCH_MODE, SWR_SERVER_FIELD_UNSAFE_EVENT,
    SWR_SERVER_FIELD_TASK_IPC_MODE, SWR_SERVER_FIELD_CPU_NUM,
};

enum {
    SWR_SERVER_CB_CONNECT, SWR_SERVER_CB_CLOSE, SWR_SERVER_CB_BUFFER_FULL, SWR_SERVER_CB_BUFFER_EMPTY,
    SWR_SERVER_CB_TASK, SWR_SERVER_CB_RECEIVE, SWR_SERVER_CB_PACKET,
};

enum {
    SWR_SERVER_OP_MODE = 0, SWR_SERVER_OP_SINGLE_THREAD, SWR_SERVER_OP_MAX_CONNECTION, SWR_SERVER_OP_ERRNO,
    SWR_SERVER_OP_CONNECTION_LIST, SWR_SERVER_OP_MAX_SOCKETS, SWR_SERVER_OP_SET_MAX_CONNECTION,
    SWR_SERVER_OP_DELAY_RECEIVE, SWR_SERVER_OP_HAS_ON_CONNECT, SWR_SERVER_OP_DISABLE_NOTIFY,
    SWR_SERVER_OP_IS_STARTED, SWR_SERVER_OP_RUNNING, SWR_SERVER_OP_SET_RUNNING, SWR_SERVER_OP_MASTER_PID,
    SWR_SERVER_OP_MANAGER_PID, SWR_SERVER_OP_IS_MASTER_THREAD, SWR_SERVER_OP_SIGNAL_ACTIVE,
    SWR_SERVER_OP_MAX_WAIT_TIME, SWR_SERVER_OP_SIGRTMIN, SWR_SERVER_OP_SIGNAL_FD, SWR_SERVER_OP_MAXFD,
    SWR_SERVER_OP_MINFD, SWR_SERVER_OP_SET_MAXFD, SWR_SERVER_OP_SET_MINFD, SWR_SERVER_OP_WRONG_OPERATION,
    SWR_SERVER_OP_WARNING, SWR_SERVER_OP_WARNING_TIME, SWR_SERVER_OP_WARNING_RESET,

    SWR_SERVER_OP_PORT_COUNT = 30, SWR_SERVER_OP_PORT_DGRAM, SWR_SERVER_OP_PORT_EVENT_ADD,
    SWR_SERVER_OP_PORT_EVENT_DEL, SWR_SERVER_OP_PORT_REMOVED, SWR_SERVER_OP_PORT_REACTOR_DEL,

    SWR_SERVER_OP_PORT_SSL = 40, SWR_SERVER_OP_PORT_IDLE_TIME, SWR_SERVER_OP_PORT_NODELAY,
    SWR_SERVER_OP_PORT_RCVBUF, SWR_SERVER_OP_PORT_SNDBUF, SWR_SERVER_OP_HAS_PORT, SWR_SERVER_OP_PORT_HEARTBEAT,

    SWR_SERVER_OP_ACCEPT = 50, SWR_SERVER_OP_SOCK_FD, SWR_SERVER_OP_SSL_CREATE, SWR_SERVER_OP_SSL_CLEAR,
    SWR_SERVER_OP_ABORT, SWR_SERVER_OP_CHUNK_SIZE, SWR_SERVER_OP_SEND_INCOMING, SWR_SERVER_OP_EVENT_FD,
    SWR_SERVER_OP_CONN_RESET, SWR_SERVER_OP_SPIN_LOCK, SWR_SERVER_OP_SPIN_UNLOCK, SWR_SERVER_OP_SESSION_ROUND,
    SWR_SERVER_OP_SESSION_CLAIM, SWR_SERVER_OP_SESSION_BIND, SWR_SERVER_OP_SET_NODELAY,
    SWR_SERVER_OP_NODELAY_ENABLED, SWR_SERVER_OP_SET_RCVBUF, SWR_SERVER_OP_SET_SNDBUF, SWR_SERVER_OP_CONN_INIT,
    SWR_SERVER_OP_DIRECT_SEND, SWR_SERVER_OP_LOCK, SWR_SERVER_OP_UNLOCK, SWR_SERVER_OP_COUNT_ACCEPT,
    SWR_SERVER_OP_ADD_RECV_TIMER, SWR_SERVER_OP_SOCK_SSL, SWR_SERVER_OP_REACTOR_ADD_READ,
    SWR_SERVER_OP_NOTIFY_CONNECT, SWR_SERVER_OP_CONN_PROTECT, SWR_SERVER_OP_NOW_MS, SWR_SERVER_OP_LAST_RECEIVED,
    SWR_SERVER_OP_LAST_SENT, SWR_SERVER_OP_CONN_CLOSING, SWR_SERVER_OP_CLOSE_CONNECTION,
    SWR_SERVER_OP_FORCE_CLOSE, SWR_SERVER_OP_CONN_LAST_RECV,

    SWR_SERVER_OP_ACCEPT_TIMER = 90, SWR_SERVER_OP_ACCEPT_TIMER_CLEAR, SWR_SERVER_OP_SEND_SHUTDOWN,
    SWR_SERVER_OP_KILL_TERM, SWR_SERVER_OP_REACTOR_WAIT_EXIT, SWR_SERVER_OP_PIPE_COMMAND_DEL,
    SWR_SERVER_OP_KILL_WINCH, SWR_SERVER_OP_FORCED_EXIT, SWR_SERVER_OP_STOP_WORKER_THREADS,
    SWR_SERVER_OP_THREAD0_SHUTDOWN, SWR_SERVER_OP_STOP_SINGLE_WORKER, SWR_SERVER_OP_BEFORE_SHUTDOWN,
    SWR_SERVER_OP_SIGNAL_RELOAD, SWR_SERVER_OP_SIGNAL_READ_MESSAGE, SWR_SERVER_OP_SIGNAL_REOPEN,
    SWR_SERVER_OP_SIGNAL_IGNORE, SWR_SERVER_OP_SIGNAL_HANDLE, SWR_SERVER_OP_MASTER_TIMER_HOOK,
    SWR_SERVER_OP_REACTOR_STOP,

    SWR_SERVER_LOG_ACCEPT_FAILED = 120, SWR_SERVER_LOG_TOO_MANY_CONNECTIONS, SWR_SERVER_LOG_NO_SESSION_SLOT,
    SWR_SERVER_LOG_NODELAY, SWR_SERVER_LOG_RCVBUF, SWR_SERVER_LOG_SNDBUF, SWR_SERVER_LOG_MAX_CONNECTION_CREATED,
    SWR_SERVER_LOG_MAX_CONNECTION_SESSIONS, SWR_SERVER_LOG_MAX_CONNECTION_SOCKETS,
    SWR_SERVER_LOG_SHUTDOWN_FAILED, SWR_SERVER_LOG_MANAGER_EXIT, SWR_SERVER_LOG_NO_IDLE_WORKER,
    SWR_SERVER_TRACE_ACCEPT = 140, SWR_SERVER_TRACE_TIMEOUT, SWR_SERVER_TRACE_SIGNAL,
    SWR_SERVER_TRACE_SHUTDOWN_BEGIN, SWR_SERVER_TRACE_SHUTDOWN_END,

    SWR_SERVER_OP_FIELD = 150, SWR_SERVER_OP_SET_FIELD, SWR_SERVER_OP_INIT_LOCAL_ADDRS, SWR_SERVER_OP_ALLOC_SESSIONS,
    SWR_SERVER_OP_ALLOC_PORT_GS, SWR_SERVER_OP_ALLOC_WORKERS, SWR_SERVER_OP_HTTP_DEFAULTS, SWR_SERVER_OP_LAST_PORT_FD,
    SWR_SERVER_OP_CREATE_FACTORY, SWR_SERVER_OP_AFTER_CREATE, SWR_SERVER_OP_TASK_POOL_CREATE,
    SWR_SERVER_OP_TASK_POOL_SETUP, SWR_SERVER_OP_TASK_POOL_LISTEN, SWR_SERVER_OP_ALLOC_TASK_RESULTS,
    SWR_SERVER_OP_TASK_PIPE, SWR_SERVER_OP_INIT_TASK_WORKERS, SWR_SERVER_OP_CALLBACK, SWR_SERVER_OP_CLEAR_CALLBACK,
    SWR_SERVER_OP_SET_DISABLE_NOTIFY, SWR_SERVER_OP_CLEAR_SEND_YIELD, SWR_SERVER_OP_CLEAR_MAX_QUEUED,
    SWR_SERVER_OP_SEND_TIMEOUT, SWR_SERVER_OP_SEND_TIMEOUT_MIN, SWR_SERVER_OP_PORT_IDLE,
    SWR_SERVER_OP_PORT_DEFAULT_IDLE, SWR_SERVER_OP_PORT_MAX_LENGTH, SWR_SERVER_OP_PORT_MIN_LENGTH,
    SWR_SERVER_OP_PORT_STREAM, SWR_SERVER_OP_PORT_SSL_AT, SWR_SERVER_OP_PORT_WEBSOCKET,
    SWR_SERVER_OP_PORT_WS_COMPRESSION, SWR_SERVER_OP_MASTER_THREAD, SWR_SERVER_OP_TIMER_DETACHED,
    SWR_SERVER_OP_TIMER_REINIT, SWR_SERVER_OP_MASTER_WORKER, SWR_SERVER_OP_MASTER_REACTOR,
    SWR_SERVER_OP_HAS_PIPE_COMMAND, SWR_SERVER_OP_COMMAND_HANDLER, SWR_SERVER_OP_PIPE_COMMAND_ADD,
    SWR_SERVER_OP_MASTER_TIMER, SWR_SERVER_OP_EVENT_FREE, SWR_SERVER_OP_REACTOR_BARRIER,
    SWR_SERVER_OP_MANAGER_BARRIER, SWR_SERVER_OP_SET_MASTER_PID, SWR_SERVER_OP_MASTER_START,
    SWR_SERVER_OP_EVENT_WAIT, SWR_SERVER_OP_STORE_LISTEN, SWR_SERVER_OP_AFTER_SHUTDOWN,
    SWR_SERVER_OP_JOIN_REACTOR_THREADS, SWR_SERVER_OP_FACTORY_SHUTDOWN, SWR_SERVER_OP_DESTROY_WORKER,
    SWR_SERVER_OP_RELEASE_PIPE_BUFFERS, SWR_SERVER_OP_PORT_CLOSE, SWR_SERVER_OP_FREE_USER_WORKERS,
    SWR_SERVER_OP_MARK_SHUTDOWN, SWR_SERVER_OP_ON_SHUTDOWN, SWR_SERVER_OP_FREE_HOOKS,
    SWR_SERVER_OP_DESTROY_FACTORY, SWR_SERVER_OP_DESTROY_TASK_WORKERS, SWR_SERVER_OP_FREE_SERVER,

    SWR_SERVER_LOG_ALLOC_SESSIONS = 220, SWR_SERVER_LOG_ALLOC_PORT_GS, SWR_SERVER_LOG_ALLOC_WORKERS,
    SWR_SERVER_LOG_MAX_CONNECTION_MINIMUM, SWR_SERVER_LOG_REACTOR_NUM, SWR_SERVER_LOG_WORKER_NUM,
    SWR_SERVER_LOG_TASK_WORKER_NUM, SWR_SERVER_LOG_TASK_POOL, SWR_SERVER_LOG_ALLOC_TASK_RESULTS,
    SWR_SERVER_LOG_CALLBACK_DISPATCH, SWR_SERVER_LOG_CALLBACK_REQUIRED, SWR_SERVER_TRACE_RELEASE,
    SWR_SERVER_TRACE_REACTOR_THREADS, SWR_SERVER_TRACE_TASK_WORKERS,

    SWR_SERVER_OP_SSL_ACCEPT = 250, SWR_SERVER_OP_RD_PORT_READ, SWR_SERVER_OP_SSL_READY, SWR_SERVER_OP_CLIENT_CERT,
    SWR_SERVER_OP_VERIFY_PEER, SWR_SERVER_OP_PEER_CERT, SWR_SERVER_OP_SSL_VERIFY, SWR_SERVER_OP_DISPATCH_CERT,
    SWR_SERVER_OP_REACTOR_DEL, SWR_SERVER_OP_PACKET_INIT, SWR_SERVER_OP_RECVFROM, SWR_SERVER_OP_PORT_DTLS,
    SWR_SERVER_OP_DTLS_ACCEPT, SWR_SERVER_OP_PACKET_DISPATCH, SWR_SERVER_OP_HAVE_DGRAM, SWR_SERVER_OP_PORT_FD,
    SWR_SERVER_OP_REACTOR_ID, SWR_SERVER_OP_REACTOR_NUM, SWR_SERVER_OP_PORT_DEL, SWR_SERVER_OP_HEARTBEAT_TIMER_CLEAR,
    SWR_SERVER_OP_DESTROY_CALLBACK, SWR_SERVER_OP_STOP_THREAD_WORKER, SWR_SERVER_OP_WORKER_NUM,
    SWR_SERVER_OP_WORKER_PIPE_UNREAD, SWR_SERVER_OP_CONN_AT, SWR_SERVER_OP_CONN_FD, SWR_SERVER_OP_CONN_REACTOR,
    SWR_SERVER_OP_CONN_SESSION, SWR_SERVER_OP_CONN_READING, SWR_SERVER_OP_CONN_UNREAD, SWR_SERVER_OP_WAIT_EXIT,
    SWR_SERVER_OP_SESSION_CONN, SWR_SERVER_OP_CONN_FORCE, SWR_SERVER_OP_CONN_HANDSHAKING, SWR_SERVER_OP_CLOSE,
    SWR_SERVER_OP_PIPE_READ, SWR_SERVER_OP_PIPE_TYPE, SWR_SERVER_OP_PIPE_FD, SWR_SERVER_OP_PIPE_POP,
    SWR_SERVER_OP_INCOMING, SWR_SERVER_OP_REACTOR_CLOSE, SWR_SERVER_OP_COMMAND_REQUEST,
    SWR_SERVER_OP_COMMAND_RESPONSE, SWR_SERVER_OP_KILL_SELF, SWR_SERVER_OP_ON_FINISH, SWR_SERVER_OP_ON_PIPE_MESSAGE,
    SWR_SERVER_OP_CLOSE_FORWARD, SWR_SERVER_OP_PIPE_SEND, SWR_SERVER_OP_DGRAM_BUFFER, SWR_SERVER_OP_SESSION_HANDLERS,
    SWR_SERVER_OP_MICROTIME, SWR_SERVER_OP_CONN_HEALTHY, SWR_SERVER_OP_EVENT_INIT, SWR_SERVER_OP_PORT_LISTEN,
    SWR_SERVER_OP_PORT_READ, SWR_SERVER_OP_STORE_LISTEN_SOCKET, SWR_SERVER_OP_THREAD0_INIT,
    SWR_SERVER_OP_MASTER_REACTOR_ID, SWR_SERVER_OP_SPAWN_REACTOR_THREAD, SWR_SERVER_OP_HEARTBEAT_TIMER,
    SWR_SERVER_OP_HEARTBEAT_THREAD, SWR_SERVER_OP_START_MASTER_THREAD, SWR_SERVER_OP_THREAD_ENTER,
    SWR_SERVER_OP_WORKER_START, SWR_SERVER_OP_WORKER_STOP, SWR_SERVER_OP_REACTOR_THREAD_TYPE,
    SWR_SERVER_OP_THREAD_INIT, SWR_SERVER_OP_THREAD_CLEAN, SWR_SERVER_OP_QUEUE_FULL, SWR_SERVER_OP_WAITING_TIME,
    SWR_SERVER_OP_SET_WAITING_TIME, SWR_SERVER_OP_RESUME_TIMER, SWR_SERVER_OP_RESUME_READ, SWR_SERVER_OP_TASK_INIT,
    SWR_SERVER_OP_RD_UNREAD, SWR_SERVER_OP_QUEUE_SUB, SWR_SERVER_OP_FACTORY_DISPATCH, SWR_SERVER_OP_COUNT_DISPATCH, SWR_SERVER_OP_JOIN_HEARTBEAT,
    SWR_SERVER_OP_THREAD_JOINABLE, SWR_SERVER_OP_THREAD_NOTIFY, SWR_SERVER_OP_THREAD_JOIN,
    SWR_SERVER_OP_SEND_CLOSE_FORCE,

    SWR_SERVER_LOG_RECVFROM = 350, SWR_SERVER_LOG_NO_SESSION, SWR_SERVER_TRACE_HEARTBEAT_JOIN,

    SWR_SERVER_OP_HAS_DISPATCH_FUNC = 400, SWR_SERVER_OP_DISPATCH_FUNC, SWR_SERVER_OP_STATE,
    SWR_SERVER_OP_IP_KEY, SWR_SERVER_OP_FD_CONN, SWR_SERVER_OP_CONN_UID, SWR_SERVER_OP_CONN_WORKER,
    SWR_SERVER_OP_PIN_WORKER, SWR_SERVER_OP_WORKER_COROUTINES, SWR_SERVER_OP_WORKER_CONCURRENCY, SWR_SERVER_OP_SET_WARNING,
    SWR_SERVER_OP_FEEDBACK_THREAD, SWR_SERVER_OP_FEEDBACK_SEND, SWR_SERVER_OP_VERIFY_CONN, SWR_SERVER_OP_SEND_SYNC,
    SWR_SERVER_OP_SOCKET_CONN, SWR_SERVER_OP_PIPE_NOBUFS, SWR_SERVER_OP_SOCKET_NOBUFS, SWR_SERVER_OP_WORKER_IDLE,
    SWR_SERVER_OP_TASKING, SWR_SERVER_OP_TASKING_RESET, SWR_SERVER_OP_IS_MASTER, SWR_SERVER_OP_SENDFILE_NAME_MAX,
    SWR_SERVER_OP_SENDFILE_NAME, SWR_SERVER_OP_SENDFILE_FINISH, SWR_SERVER_OP_IS_CREATED, SWR_SERVER_OP_PORT_TYPE,
    SWR_SERVER_OP_PORT_NUMBER, SWR_SERVER_OP_HOST_SIZE, SWR_SERVER_OP_HOST_MAXSIZE, SWR_SERVER_OP_PORT_NEW,
    SWR_SERVER_OP_PORT_SOCKET, SWR_SERVER_OP_NEW_PORT_TYPE, SWR_SERVER_OP_DGRAM_PORT, SWR_SERVER_OP_STREAM_PORT,
    SWR_SERVER_OP_PORT_PUSH, SWR_SERVER_OP_ENV_LISTEN_PID, SWR_SERVER_OP_ENV_FDS_START,
    SWR_SERVER_OP_SENDFILE_NAME_LEN, SWR_SERVER_OP_SYSTEMD_FDS, SWR_SERVER_OP_PORT_IMPORT,
    SWR_SERVER_OP_SENDFILE_OFFSET,

    SWR_SERVER_LOG_INVALID_SESSION = 450, SWR_SERVER_LOG_SEND_IN_MASTER, SWR_SERVER_LOG_SENDFILE_NAME,
    SWR_SERVER_LOG_STAT, SWR_SERVER_LOG_NOT_REGULAR, SWR_SERVER_LOG_FILE_EMPTY, SWR_SERVER_LOG_SENDWAIT,
    SWR_SERVER_LOG_PORT_CREATED, SWR_SERVER_LOG_TOO_MANY_PORTS, SWR_SERVER_LOG_INVALID_PORT,
    SWR_SERVER_LOG_HOST_TOO_LONG, SWR_SERVER_LOG_LISTEN_PID, SWR_SERVER_LOG_FDS_START,

    SWR_SERVER_OP_BASE_REACTORS = 500, SWR_SERVER_OP_ALLOC_CONNECTIONS, SWR_SERVER_OP_ALLOC_CONNECTION_NUMS,
    SWR_SERVER_OP_ALLOC_PORT_CONNECTION_NUMS, SWR_SERVER_OP_NEW_BASE_FACTORY, SWR_SERVER_OP_INFO_CONN,
    SWR_SERVER_OP_CONN_ACTIVE, SWR_SERVER_OP_CONN_CLOSED, SWR_SERVER_OP_NOTIFY_ACCEPT, SWR_SERVER_OP_SESSION_FD,
    SWR_SERVER_OP_SESSION_FOREIGN, SWR_SERVER_OP_FORWARD_CLOSE, SWR_SERVER_OP_NOT_EXIST, SWR_SERVER_OP_CLOSE_RESET,
    SWR_SERVER_OP_CLOSE_ACTIVELY, SWR_SERVER_OP_CONN_CLOSING_FLAG, SWR_SERVER_OP_SET_CLOSING,
    SWR_SERVER_OP_CLOSE_CALLBACK, SWR_SERVER_OP_CLOSE_DONE, SWR_SERVER_OP_CONN_RESETTING,
    SWR_SERVER_OP_SEND_KIND_OK, SWR_SERVER_OP_FORWARD_SEND, SWR_SERVER_OP_SEND_HERE,

    SWR_SERVER_LOG_BASE_ALLOC_CONNECTIONS = 530, SWR_SERVER_LOG_BASE_ALLOC_NUMS, SWR_SERVER_LOG_BASE_ALLOC_PORT_NUMS,
    SWR_SERVER_LOG_NOT_ACTIVE, SWR_SERVER_LOG_CLOSED_BY_SERVER, SWR_SERVER_LOG_END_NO_SESSION,
    SWR_SERVER_LOG_SESSION_CLOSING, SWR_SERVER_LOG_SESSION_CLOSED, SWR_SERVER_LOG_UNKNOWN_EVENT,
    SWR_SERVER_TRACE_FORWARD,
};

/* Reactor threads' close, peer-close and pipe-write paths and setup (crates/server/src/reactor_io.rs). */
enum {
    SWR_SERVER_RUN_RT_PIPE_WRITE = 200, SWR_SERVER_RUN_RT_INIT,
    SWR_SERVER_OP_RT_BUF_EMPTY = 900,
    SWR_SERVER_OP_RT_FRONT,
    SWR_SERVER_OP_RT_FRONT_STREAM,
    SWR_SERVER_OP_RT_FRONT_CONN,
    SWR_SERVER_OP_RT_TOUCH_SEND,
    SWR_SERVER_OP_RT_FRONT_CONN_CLOSED,
    SWR_SERVER_LOG_RT_CLOSED_BY_SERVER,
    SWR_SERVER_OP_RT_DISCARD_TIMEOUT,
    SWR_SERVER_LOG_RT_DISCARD,
    SWR_SERVER_OP_RT_POP,
    SWR_SERVER_OP_RT_SEND_FRONT,
    SWR_SERVER_OP_RT_CATCH_WAIT,
    SWR_SERVER_OP_RT_REMOVE_WRITE,
    SWR_SERVER_LOG_RT_SET_WRITE,
    SWR_SERVER_OP_RT_SETUP,
    SWR_SERVER_OP_RT_DGRAM_BIND,
    SWR_SERVER_OP_RT_INIT_REACTOR,
    SWR_SERVER_OP_RT_INIT_PIPE_SOCKETS,
    SWR_SERVER_OP_RT_THREAD_WORKER,
    SWR_SERVER_OP_RT_HEARTBEAT_TIMER,
    SWR_SERVER_OP_RT_HAS_PIPE_COMMAND,
    SWR_SERVER_OP_RT_PIPE_COMMAND,
    SWR_SERVER_OP_RT_BUS_SETUP,
    SWR_SERVER_OP_RT_ADD_WORKER_PIPE,
};

/* A connection's read, write and close paths: ReactorThread_onRead/onWrite/onClose and
 * Server::close_connection (crates/server/src/conn_io.rs). RUN_RT_READ is 20 and its
 * port read OP_RD_PORT_READ 251, reusing the numbers of the TLS check they replace. */
enum {
    SWR_SERVER_RUN_RT_WRITE = 211, SWR_SERVER_RUN_PEER_CLOSE, SWR_SERVER_RUN_CLOSE_CONNECTION,
    SWR_SERVER_OP_RD_DTLS = 940, SWR_SERVER_OP_RD_ABORT, SWR_SERVER_OP_RD_WRITE_FACTS,
    SWR_SERVER_OP_WR_NOTIFY_CLOSE, SWR_SERVER_OP_WR_FRONT, SWR_SERVER_OP_WR_CLOSE, SWR_SERVER_OP_WR_SENDFILE,
    SWR_SERVER_OP_WR_SEND, SWR_SERVER_OP_WR_QUEUED, SWR_SERVER_OP_WR_WAIT, SWR_SERVER_OP_WR_CLOSE_ERRNO,
    SWR_SERVER_OP_WR_STATE, SWR_SERVER_OP_WR_CLEAR_OVERFLOW, SWR_SERVER_OP_WR_BUFFER_EMPTY,
    SWR_SERVER_OP_WR_TIMER_DEL, SWR_SERVER_OP_WR_TIMER_DELAY, SWR_SERVER_OP_WR_READ_ONLY,
    SWR_SERVER_OP_PC_NOTIFY, SWR_SERVER_OP_CC_FACTS, SWR_SERVER_OP_CC_TIMER_DEL, SWR_SERVER_OP_CC_SSL_SHUTDOWN,
    SWR_SERVER_OP_CC_SSL_WANT, SWR_SERVER_OP_CC_QUEUE_CLOSE, SWR_SERVER_OP_CC_WATCH_WRITE,
    SWR_SERVER_OP_CC_WEBSOCKET_FREE, SWR_SERVER_OP_CC_COUNT, SWR_SERVER_OP_CC_SSL_CLOSE, SWR_SERVER_OP_CC_DTLS_FREE,
    SWR_SERVER_OP_CC_RECV_FREE, SWR_SERVER_OP_CC_HTTP_OBJECT, SWR_SERVER_OP_CC_HTTP_DESTROY,
    SWR_SERVER_OP_CC_REDIS_OBJECT, SWR_SERVER_OP_CC_REDIS_FREE, SWR_SERVER_OP_CC_LINGER,
    SWR_SERVER_OP_CC_SESSION_CLEAR, SWR_SERVER_OP_CC_VALID, SWR_SERVER_OP_CC_FINISH,
    SWR_SERVER_LOG_CC_LINGER = 990, SWR_SERVER_TRACE_CC_MAXFD,
};
/* Their fact bits (crates/server/src/conn_io.rs). */
#define SWR_CN_FOUND (1ull << 0)
#define SWR_CN_CLOSE_NOTIFIED (1ull << 1)
#define SWR_CN_CLOSE_QUEUED (1ull << 2)
#define SWR_CN_SSL_WANT_READ (1ull << 3)
#define SWR_CN_SSL_PENDING (1ull << 4)
#define SWR_CN_DTLS_PORT (1ull << 5)
#define SWR_CN_DTLS_SOCKET (1ull << 6)
#define SWR_CN_PREQUEUE (1ull << 7)
#define SWR_CN_PROCESS (1ull << 8)
#define SWR_CN_CLOSE_NOTIFY (1ull << 9)
#define SWR_CN_DISABLE_NOTIFY (1ull << 10)
#define SWR_CN_CLOSE_FORCE (1ull << 11)
#define SWR_CN_CLOSE_RESET (1ull << 12)
#define SWR_CN_PEER_CLOSED (1ull << 13)
#define SWR_CN_HASH_DISPATCH (1ull << 14)
#define SWR_CN_DISPATCH_FUNC (1ull << 15)
#define SWR_CN_COROUTINE (1ull << 16)
#define SWR_CN_CONN_SSL (1ull << 17)
#define SWR_CN_SOCKET_SSL (1ull << 18)
#define SWR_CN_CLOSE_ERRNO (1ull << 19)
#define SWR_CN_TIMER (1ull << 20)
#define SWR_CN_WEBSOCKET_BUFFER (1ull << 21)
#define SWR_CN_REMOVED (1ull << 22)
#define SWR_CN_BASE (1ull << 23)
#define SWR_CN_RECV_BUFFER (1ull << 24)
#define SWR_CN_LINGER (1ull << 25)
#define SWR_CN_TLS_READY_STREAM (1ull << 26)
#define SWR_CN_SSL_CLOSED (1ull << 27)
#define SWR_CN_OVERFLOW_DRAINED (1ull << 28)
#define SWR_CN_LOW_WATERMARK (1ull << 29)
#define SWR_CN_SEND_TIMER (1ull << 30)
#define SWR_CN_OUT_EMPTY (1ull << 31)
#define SWR_CN_ACTIVE (1ull << 32)
#define SWR_CN_QUEUE_OVER (1ull << 33)
#define SWR_CN_READ_NOTIFIED (1ull << 34)
#define SWR_CN_CLOSE_WAIT (1ull << 35)
#define SWR_CN_SEND_WAIT (1ull << 36)

/* Rust-owned server state and the rest of master.cc (crates/server/src/state.rs). */
enum {
    SWR_SERVER_RUN_ST_ADD_COMMAND = 300, SWR_SERVER_RUN_ST_DTLS_ACCEPT, SWR_SERVER_RUN_ST_DOCUMENT_ROOT,
    SWR_SERVER_RUN_ST_STORE_PIPE_FD, SWR_SERVER_RUN_ST_INIT_PIPE_SOCKETS, SWR_SERVER_RUN_ST_RESET_COUNTER,
    SWR_SERVER_RUN_ST_ABORT, SWR_SERVER_RUN_ST_FOREACH, SWR_SERVER_RUN_ST_SEND, SWR_SERVER_RUN_ST_CLEAR_TIMER,
    SWR_SERVER_OP_ST_CMD_EXISTS = 1000, SWR_SERVER_OP_ST_PIPE_COMMAND_NEW, SWR_SERVER_OP_ST_CMD_ADD,
    SWR_SERVER_OP_ST_DTLS_SOCKET, SWR_SERVER_OP_ST_DTLS_TYPE, SWR_SERVER_OP_ST_DTLS_ASSERT,
    SWR_SERVER_OP_ST_DTLS_BIND, SWR_SERVER_OP_ST_DTLS_V6ONLY, SWR_SERVER_OP_ST_DTLS_CONNECT,
    SWR_SERVER_OP_ST_DTLS_PEER, SWR_SERVER_OP_ST_DTLS_ADD, SWR_SERVER_OP_ST_DTLS_SESSION,
    SWR_SERVER_OP_ST_DTLS_CLEANUP, SWR_SERVER_OP_ST_ROOT_PATH, SWR_SERVER_OP_ST_SET_ROOT,
    SWR_SERVER_OP_ST_PIPE_FD, SWR_SERVER_OP_ST_PIPE_BIND, SWR_SERVER_OP_ST_INIT_PIPE,
    SWR_SERVER_OP_ST_W_CONCURRENCY, SWR_SERVER_OP_ST_W_CONCURRENCY_CAS, SWR_SERVER_OP_ST_GS_CONCURRENCY_SUB,
    SWR_SERVER_OP_ST_GS_CONCURRENCY, SWR_SERVER_OP_ST_GS_CONCURRENCY_ZERO, SWR_SERVER_OP_ST_W_RESET_COUNTS,
    SWR_SERVER_OP_ST_ABORT_COUNT, SWR_SERVER_OP_ST_SOCK_OBJECT, SWR_SERVER_OP_ST_REACTOR_CLOSE,
    SWR_SERVER_OP_ST_SOCK_FREE, SWR_SERVER_OP_ST_VALID_CONN, SWR_SERVER_OP_ST_VISIT,
    SWR_SERVER_OP_ST_FINISH, SWR_SERVER_OP_ST_COUNT_SEND, SWR_SERVER_OP_ST_TIMER, SWR_SERVER_OP_ST_TIMER_DEL,
    SWR_SERVER_LOG_ST_DTLS_BIND = 1100, SWR_SERVER_LOG_ST_DTLS_CONNECT, SWR_SERVER_LOG_ST_ROOT_TOO_LONG,
    SWR_SERVER_LOG_ST_ROOT_MISSING, SWR_SERVER_TRACE_ST_SCHEDULE,
};

/* Server::send_to_connection (crates/server/src/send.rs). */
enum {
    SWR_SERVER_RUN_SC_SEND = 310,
    SWR_SERVER_OP_SC_VERIFY = 1040, SWR_SERVER_OP_SC_FLAGS, SWR_SERVER_OP_SC_YIELD_ERROR, SWR_SERVER_OP_SC_CLOSE,
    SWR_SERVER_OP_SC_SET, SWR_SERVER_OP_SC_DEL, SWR_SERVER_OP_SC_ADD, SWR_SERVER_OP_SC_REMOVE_READ,
    SWR_SERVER_OP_SC_ADD_READ, SWR_SERVER_OP_SC_SEND, SWR_SERVER_OP_SC_ADVANCE, SWR_SERVER_OP_SC_TOUCH_SEND, SWR_SERVER_OP_SC_ENSURE_BUFFER,
    SWR_SERVER_OP_SC_QUEUE_CLOSE, SWR_SERVER_OP_SC_SENDFILE, SWR_SERVER_OP_SC_OUT_FULL, SWR_SERVER_OP_SC_OVERFLOW,
    SWR_SERVER_OP_SC_HIGH_WATERMARK, SWR_SERVER_OP_SC_APPEND, SWR_SERVER_OP_SC_OUT_HIGH,
    SWR_SERVER_OP_SC_NOTIFY_FULL, SWR_SERVER_OP_SC_HAS_SEND_TIMER, SWR_SERVER_OP_SC_IDLE_TIMER,
    SWR_SERVER_OP_SC_WRITABLE, SWR_SERVER_OP_SC_ADD_WRITE,
    SWR_SERVER_LOG_SC_NO_SESSION_DATA = 1110, SWR_SERVER_LOG_SC_NO_SESSION_EVENT, SWR_SERVER_LOG_SC_SOCKET_OVERFLOW,
    SWR_SERVER_LOG_SC_CLOSED_BY_CLIENT, SWR_SERVER_LOG_SC_CONN_OVERFLOW,
};
/* send_to_connection's OP_SC_FLAGS bits (crates/server/src/send.rs), OP_SC_VERIFY's (those, the
 * SWR_SC_FOUND to SWR_SC_DIRECT_SEND bits and the send's length above bit 32) and OP_SC_SET/OP_SC_ADD events. */
#define SWR_SC_OVERFLOW (1u << 0)
#define SWR_SC_CLOSE_RESET (1u << 1)
#define SWR_SC_CLOSE_FORCE (1u << 2)
#define SWR_SC_PEER_CLOSED (1u << 3)
#define SWR_SC_CLOSED (1u << 4)
#define SWR_SC_CLOSE_QUEUED (1u << 5)
#define SWR_SC_HIGH_WATERMARK (1u << 6)
#define SWR_SC_REMOVED (1u << 7)
#define SWR_SC_CLOSE_NOTIFIED (1u << 8)
#define SWR_SC_EVENT_READ (1u << 9)
#define SWR_SC_EVENT_WRITE (1u << 10)
#define SWR_SC_ON_BUFFER_FULL (1u << 11)
#define SWR_SC_ON_BUFFER_EMPTY (1u << 12)
#define SWR_SC_FOUND (1u << 13)
#define SWR_SC_SEND_YIELD (1u << 14)
#define SWR_SC_HALFCLOSE (1u << 15)
#define SWR_SC_OUT_EMPTY (1u << 16)
#define SWR_SC_DIRECT_SEND (1u << 17)
#define SWR_SC_READ 1u
#define SWR_SC_WRITE 2u

/* Server::state_: the round-robin and command id counters. */
typedef struct swr_v1_server_state swr_v1_server_state;
swr_v1_server_state *swr_v1_server_state_new(void);
void swr_v1_server_state_free(swr_v1_server_state *state);
/* Server::get_startup_error_message: message, or code as text when it is empty and code > 0. */
const char *swr_v1_server_startup_error(const char *message, int code);

/* The server's local interface addresses (Server::local_addrs). */
typedef struct swr_v1_server_local_addrs swr_v1_server_local_addrs;
swr_v1_server_local_addrs *swr_v1_server_local_addrs_new(void);
void swr_v1_server_local_addrs_free(swr_v1_server_local_addrs *addrs);
uint16_t swr_v1_server_local_addr_index(const swr_v1_server_local_addrs *addrs, bool v6, const uint8_t *addr);
const char *swr_v1_server_local_addr(const swr_v1_server_local_addrs *addrs, bool v6, uint16_t index);

/*
 * The admin server's commands (crates/server/src/admin.rs). register_admin_server_commands
 * adds command i under swr_v1_server_admin_command(i) until it returns null; its handler
 * runs swr_v1_server_admin_run, which reads each fact through call(context, op, arg, out)
 * and hands back the JSON answer with SWR_SERVER_ADMIN_ANSWER. Text fields are lent
 * until the next call.
 */
typedef uint64_t (*swr_v1_server_admin_call)(void *context, uint32_t op, uint64_t arg, void *out);
const char *swr_v1_server_admin_command(uint32_t index, bool event_workers, int *accepted_process_types);
void swr_v1_server_admin_run(uint32_t index, const char *msg, size_t len, void *context, swr_v1_server_admin_call call);

enum {
    SWR_SERVER_ADMIN_ANSWER, SWR_SERVER_ADMIN_PIPE_WORKERS, SWR_SERVER_ADMIN_PIPE_SOCKET,
    SWR_SERVER_ADMIN_HAS_REACTOR, SWR_SERVER_ADMIN_SOCKETS, SWR_SERVER_ADMIN_SOCKET,
    SWR_SERVER_ADMIN_SOCKET_NAME, SWR_SERVER_ADMIN_COMMANDS, SWR_SERVER_ADMIN_COMMAND,
    SWR_SERVER_ADMIN_CONNECTIONS, SWR_SERVER_ADMIN_CONNECTION, SWR_SERVER_ADMIN_SESSION,
    SWR_SERVER_ADMIN_THREAD, SWR_SERVER_ADMIN_MANAGER, SWR_SERVER_ADMIN_MEMORY, SWR_SERVER_ADMIN_PORTS,
    SWR_SERVER_ADMIN_PORT,
};

typedef struct { const char *name; uint64_t value; } swr_v1_server_fact;
typedef struct { const char *data; size_t len; } swr_v1_server_text;
typedef struct {
    int64_t fd, events, socket_type, fd_type, port;
    uint64_t total_recv_bytes, total_send_bytes, out_buffer_size;
    swr_v1_server_text address;
} swr_v1_server_admin_socket;
typedef struct {
    int64_t session_id, reactor_id, fd, server_port, port;
    uint64_t overflow, recv_queued_bytes, send_queued_bytes, total_recv_bytes, total_send_bytes, uid;
    double connect_time, last_recv_time, last_send_time, last_dispatch_time;
    swr_v1_server_text server_address, address;
} swr_v1_server_admin_connection;
typedef struct {
    swr_v1_server_text host, protocols;
    int64_t port, backlog, type, ssl, connection_num;
} swr_v1_server_admin_port;
typedef struct { swr_v1_server_text name; int64_t id, accepted_process_types; } swr_v1_server_admin_command_info;
typedef struct { int64_t id; uint64_t dispatch_count, event_num, timer_num; } swr_v1_server_admin_thread;
typedef struct { uint64_t reload_count; int64_t reload_last_time; } swr_v1_server_admin_manager;

/* The TCP_INFO fields of *info (struct tcp_info; tcp_connection_info on Apple) in
 * upstream's order, at most cap of them into out; returns how many. Linux and Apple. */
size_t swr_v1_server_tcp_info(const void *info, swr_v1_server_fact *out, size_t cap);

/*
 * The module host (crates/phpswoole): php_swoole.cc's MINIT sets up the module
 * globals (swr_v1_phpswoole_init_globals) and ini entries, then
 * swr_v1_phpswoole_minit registers the global constants, Swoole\Exception/Swoole\Error
 * and the short-name aliases and runs Swoole's core and every family's MINIT. Rust
 * defines MSHUTDOWN, RINIT and RSHUTDOWN (swr_v1_phpswoole_mshutdown, _rinit, _rshutdown), and
 * swoole_module_entry's function table is swr_v1_phpswoole_functions: Rust's rows (its
 * zif_swoole_* exports and C++'s) around the stdext/ssh2/ftp rows only C++'s #ifdefs
 * decide. C++ lends what only it can reach: Swoole's core, error, logger and MIME state.
 */
#define SWR_PHPSWOOLE_DEBUG (1u << 0)
#define SWR_PHPSWOOLE_SHORTNAME (1u << 1)
#define SWR_PHPSWOOLE_COMPRESSION (1u << 2)
#define SWR_PHPSWOOLE_ZLIB (1u << 3)
#define SWR_PHPSWOOLE_BROTLI (1u << 4)
#define SWR_PHPSWOOLE_TLS1_1 (1u << 5)
#define SWR_PHPSWOOLE_TLS1_2 (1u << 6)
#define SWR_PHPSWOOLE_TLS1_3 (1u << 7)
#define SWR_PHPSWOOLE_DTLS (1u << 8)
#define SWR_PHPSWOOLE_SSL3 (1u << 9)
#define SWR_PHPSWOOLE_IOURING (1u << 10)
#define SWR_PHPSWOOLE_STDEXT (1u << 11)
struct _zval_struct;
struct _zend_array;
struct _zend_class_entry;
struct _zend_object_handlers;
typedef struct swr_v1_phpswoole_ops {
    int32_t (*last_error)(void);
    void (*set_last_error)(int32_t code);
    /* ZEND_MODULE_GLOBALS_BULK(swoole): this thread's zend_swoole_globals. */
    void *(*globals)(void);
    /* MINIT: swoole_init(), the after-fork hook, the bug report; and its end: the
     * fatal-error and backtrace hooks, socket buffer size, DNS refresh, pcre.jit, known strings. */
    void (*startup)(void);
    void (*ready)(void);
    /* The hooks only C++'s #ifdefs decide: ssh2/ftp after the clients, the drivers,
     * Swoole\Thread and stdext before the tracer, and their MSHUTDOWN, RINIT, RSHUTDOWN. */
    void (*optional_client_minit)(int module_number);
    void (*optional_minit)(int module_number);
    void (*optional_mshutdown)(void);
    void (*optional_rinit)(void);
    void (*optional_rshutdown)(void);
    void (*known_strings_dtor)(void);
    void (*clean)(void);
    void (*set_task_tmpdir)(const char *dir);
    /* php_swoole_load_library(), a zend_try body. */
    void (*load_library)(void *);
    void (*event_free)(void);
    uint32_t (*cpu_num)(void);
    /* sw_logger()->put */
    void (*log_put)(int32_t level, const char *message, size_t length);
    /* swoole_error_log's tail: set the error, then put if the level passes and it isn't ignored. */
    void (*error_log)(int32_t level, int32_t error, const char *message, size_t length);
    void (*ignore_error)(int32_t code);
    uint8_t (*mime_add)(const char *suffix, const char *type);
    void (*mime_set)(const char *suffix, const char *type);
    uint8_t (*mime_del)(const char *suffix);
    const char *(*mime_get)(const char *filename);
    uint8_t (*mime_exists)(const char *filename);
    void (*mime_list)(struct _zval_struct *array);
    void (*unserialize)(struct _zval_struct *value, const char *buf, size_t length, struct _zend_array *options);
    void (*json_decode)(struct _zval_struct *value, const char *str, size_t length, int64_t options, int64_t depth);
    /* php_swoole_set_global_option's effects, in its order. */
    void (*set_signalfd)(uint8_t on);
    void (*set_kqueue)(uint8_t on);
    void (*set_trace_flags)(int64_t flags);
    void (*set_log_file)(const char *file, size_t length);
    void (*set_log_level)(int64_t level);
    void (*set_log_date_format)(const char *format, size_t length);
    void (*set_log_date_with_microseconds)(uint8_t on);
    void (*set_log_rotation)(int64_t rotation);
    void (*set_print_backtrace)(uint8_t on);
    void (*set_dns_server)(const char *server, size_t length);
    /* kind: 0 dns, 1 connect, 2 read, 3 write */
    void (*set_socket_timeout)(int32_t kind, double seconds);
    void (*set_socket_buffer_size)(int64_t size);
    void (*set_http2_default)(int32_t id, int64_t value);
    /* swoole_async_set() and php_swoole_set_aio_option's SwooleG fields. */
    void (*set_dns_cache_refresh_time)(double seconds);
    double (*dns_cache_refresh_time)(void);
    /* SW_MUST_BE_MAIN_THREAD's test: 1 without SW_THREAD. */
    uint8_t (*main_thread)(void);
    uint8_t (*reactor_created)(void);
    /* Coroutine::get_current_safe(): a fatal error outside a coroutine. */
    void (*current_coroutine_safe)(void);
    void (*check_reactor)(void);
    double (*default_dns_timeout)(void);
    int64_t (*absolute_msec)(void);
    /* swoole::coroutine::dns_lookup: each address pushed onto list in order. */
    void (*dns_lookup)(const char *domain, int32_t family, double timeout, void (*push)(void *list, const char *address),
                       void *list);
} swr_v1_phpswoole_ops;
void swr_v1_phpswoole_minit(int32_t module_number, uint32_t flags, const swr_v1_phpswoole_ops *ops,
                            struct _zend_class_entry **exception, struct _zend_object_handlers *exception_handlers,
                            struct _zend_class_entry **error);
struct _zend_function_entry;
struct _zend_execute_data;
struct _zval_struct;
typedef void (*swr_php_function)(struct _zend_execute_data *, struct _zval_struct *);
/* Null disables a family; arrays have 23/35/36 entries. FTP SSL alone may be null. */
const struct _zend_function_entry *swr_v1_phpswoole_functions(const swr_php_function *stdext,
                                                          const swr_php_function *ssh2,
                                                          const swr_php_function *ftp);
/* The module's MSHUTDOWN, RINIT and RSHUTDOWN: 0 on success. */
int swr_v1_phpswoole_mshutdown(int _kind, int module_number);
int swr_v1_phpswoole_rinit(int _kind, int module_number);
int swr_v1_phpswoole_rshutdown(int _kind, int module_number);
/* ZEND_INIT_MODULE_GLOBALS' constructor of zend_swoole_globals. */
void swr_v1_phpswoole_init_globals(void *globals);
/* sw_php_allocator() (emalloc) and sw_zend_string_allocator() (zend_string values):
 * swoole::Allocators. */
typedef struct swr_v1_phpswoole_allocator {
    void *(*malloc)(size_t size);
    void *(*calloc)(size_t nmemb, size_t size);
    void *(*realloc)(void *ptr, size_t size);
    void (*free)(void *ptr);
} swr_v1_phpswoole_allocator;
extern const swr_v1_phpswoole_allocator swr_v1_phpswoole_php_allocator;
extern const swr_v1_phpswoole_allocator swr_v1_phpswoole_zend_string_allocator;

#ifdef __cplusplus
}
#endif

#endif
