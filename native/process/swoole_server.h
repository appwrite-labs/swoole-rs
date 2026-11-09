/* SPDX-License-Identifier: Apache-2.0 */
/* swoole::Server and its factories: the C++ declarations over the Rust records (crates/server,
 * crates/process) whose methods Rust exports under their upstream symbols, keeping only what
 * native/ and the test fixtures call. Was include/swoole_server.h. */
#pragma once

#include "swoole_rs_process_pool.h"
#include "swoole_rs_server_records.h"
#include "swoole_rs_server_shared.h"
#include "swoole_rs_server_strings.h"
#include "swoole_rs_server_view.h"
#include "swoole_rs_corebase_worker_registry_view.h"
#include "swoole_rs_port_state.h"
#include "swoole_rs.h"

#include <string>
#include <queue>
#include <thread>
#include <mutex>
#include <atomic>
#include <unordered_map>
#include <condition_variable>
#include <cstdlib>

namespace swoole {

class Server;

struct ReactorThread {
    int id;
    network::Socket *notify_pipe = nullptr;
    uint64_t dispatch_count = 0;
    network::Socket *pipe_command = nullptr;
    TimerNode *heartbeat_timer = nullptr;
    MessageBus message_bus;

    void shutdown(Reactor *reactor);
    int close_connection(Reactor *reactor, SessionId session_id);
};

struct ServerGS {
#define SWR_COUNTER_DECLARE(type,name) type name;
    SWR_SERVER_COUNTER_FIELDS(SWR_COUNTER_DECLARE)
#undef SWR_COUNTER_DECLARE
    // crates/server's view finds the pools right after the counters.
    ProcessPool task_workers;
    ProcessPool event_workers;
    Barrier manager_barrier;
};
static_assert(offsetof(ServerGS, task_workers) ==
                      (sizeof(ServerCounters) + alignof(ProcessPool) - 1) / alignof(ProcessPool) * alignof(ProcessPool) &&
                  offsetof(ServerGS, event_workers) == offsetof(ServerGS, task_workers) + sizeof(ProcessPool),
              "the pools crates/server's view reads");

// serv->factory_: the record crates/server/src/factory.rs makes, whose members (upstream's
// BaseFactory, ProcessFactory and ThreadFactory virtuals) dispatch on the server's mode.
class Factory {
  public:
    Server *server_;
    // Thread mode's manager state (crates/process/src/workers/thread.rs); null otherwise.
    void *manager_;
};

// Thread mode's factory, as crates/server's thread ops see it.
class ThreadFactory : public Factory {
  public:
    WorkerId get_master_thread_id() const;
};

// The numbering crates/server's events use.
enum ServerEventType {
    SW_SERVER_EVENT_RECV_DATA,
    SW_SERVER_EVENT_RECV_DGRAM,
    SW_SERVER_EVENT_SEND_DATA,
    SW_SERVER_EVENT_SEND_FILE,
    SW_SERVER_EVENT_CLOSE,
    SW_SERVER_EVENT_CONNECT,
    SW_SERVER_EVENT_CLOSE_FORCE,
    SW_SERVER_EVENT_CLOSE_FORWARD,
    SW_SERVER_EVENT_TASK,
    SW_SERVER_EVENT_FINISH,
    SW_SERVER_EVENT_PIPE_MESSAGE,
    SW_SERVER_EVENT_PAUSE_RECV,
    SW_SERVER_EVENT_RESUME_RECV,
    SW_SERVER_EVENT_BUFFER_FULL,
    SW_SERVER_EVENT_BUFFER_EMPTY,
    SW_SERVER_EVENT_INCOMING,
    SW_SERVER_EVENT_SHUTDOWN,
    SW_SERVER_EVENT_COMMAND_REQUEST,
    SW_SERVER_EVENT_COMMAND_RESPONSE,
    SW_SERVER_EVENT_SHUTDOWN_SIGNAL,
};

// The record crates/server allocates and frees (swr_v1_server_new and swr_v1_server_delete, in
// crates/server/src/server_object.rs), which C++ only views: these four pointers head it. Its
// methods are Rust's under their upstream symbols; the inline ones are what native/ and the test
// fixtures still call.
class Server {
    swr_v1_server_state *state_;
    swr_v1_server_config *config_;
    swr_v1_server_tables *tables_;
    swr_v1_server_callback *callbacks_;

    template <class Sig> detail::Callback<Sig> &callback(int index) const {
        return *reinterpret_cast<detail::Callback<Sig> *>(callbacks_ + index);
    }

  public:
    enum Mode {
        MODE_BASE = 1,
        MODE_PROCESS = 2,
        MODE_THREAD = 3,
    };

    enum TaskIpcMode {
        TASK_IPC_UNIXSOCK = 1,
        TASK_IPC_MSGQUEUE = 2,
        TASK_IPC_PREEMPTIVE = 3,
        TASK_IPC_STREAM = 4,
    };

    enum ThreadType {
        THREAD_REACTOR = 2,
        THREAD_WORKER = 4,
    };

    enum DispatchMode {
        DISPATCH_ROUND = 1,
        DISPATCH_FDMOD = 2,
        DISPATCH_IDLE_WORKER = 3,
        DISPATCH_IPMOD = 4,
        DISPATCH_UIDMOD = 5,
        DISPATCH_USERFUNC = 6,
        DISPATCH_CO_CONN_LB = 8,
        DISPATCH_CO_REQ_LB = 9,
        DISPATCH_CONCURRENT_LB = 10,
    };

    enum FactoryDispatchResult {
        DISPATCH_RESULT_DISCARD_PACKET = -1,
        DISPATCH_RESULT_CLOSE_CONNECTION = -2,
        DISPATCH_RESULT_USERFUNC_FALLBACK = -3,
    };

    enum CloseFlag {
        CLOSE_RESET = 1u << 1,
        CLOSE_ACTIVELY = 1u << 2,
    };

    Server() = delete;
    Server(const Server &) = delete;
    ~Server() = delete;

    swr_v1_server_config *config() const {
        return config_;
    }

    swr_v1_server_state *state_handle() const {
        return state_;
    }

    // The static handler's locations and the compressed MIME types.
    detail::ServerStrings locations() const {
        return {state_, 0};
    }

    detail::ServerStrings http_compression_types() const {
        return {state_, 2};
    }

    detail::Callback<void(Server *)> &onStart() const {
        return callback<void(Server *)>(SWR_SERVER_CB_START);
    }
    detail::Callback<void(Server *, Worker *)> &onWorkerStart() const {
        return callback<void(Server *, Worker *)>(SWR_SERVER_CB_WORKER_START);
    }
    detail::Callback<int(Server *, RecvData *)> &onReceive() const {
        return callback<int(Server *, RecvData *)>(SWR_SERVER_CB_RECEIVE);
    }
    detail::Callback<void(Server *, DataHead *)> &onClose() const {
        return callback<void(Server *, DataHead *)>(SWR_SERVER_CB_CLOSE);
    }
    detail::Callback<void(Server *, DataHead *)> &onConnect() const {
        return callback<void(Server *, DataHead *)>(SWR_SERVER_CB_CONNECT);
    }

    ListenPort *get_primary_port() const {
        return tables_->ports.front();
    }

    int create();
    int start();
    bool shutdown();
    void destroy();

    int add_worker(Worker *worker);
    ListenPort *add_port(SocketType type, const char *host, int port);
    void abort_worker(Worker *worker) const;
    void reset_worker_counter(Worker *worker) const;

    uint32_t get_idle_worker_num() const;

    pid_t get_master_pid() const {
        return config_->gs->master_pid;
    }

    ProcessPool *get_event_worker_pool() const {
        return &config_->gs->event_workers;
    }

    bool is_enable_coroutine() const {
        if (is_task_worker()) {
            return config_->task_enable_coroutine;
        } else if (is_manager()) {
            return false;
        } else {
            return config_->enable_coroutine;
        }
    }

    Worker *get_worker(uint16_t worker_id) const;
    Worker *get_worker_by_pid(pid_t worker_pid) const;

    size_t get_core_worker_num() const {
        return config_->worker_num + config_->task_worker_num;
    }

    bool is_task_worker() const {
        return swoole_get_worker_type() == SW_TASK_WORKER;
    }

    bool is_manager() const {
        return swoole_get_worker_type() == SW_MANAGER;
    }

    bool is_user_worker() const {
        return swoole_get_worker_type() == SW_USER_WORKER;
    }

    bool is_worker_thread() const {
        return config_->mode_ == MODE_THREAD && swoole_get_thread_type() == THREAD_WORKER;
    }

    bool is_healthy_connection(double now, const Connection *conn) const;

    Connection *get_connection_by_session_id(SessionId session_id) const {
        return static_cast<Connection *>(swr_v1_server_connection(state_, session_id, 1));
    }

    Session *get_session(SessionId session_id) const {
        return static_cast<Session *>(swr_v1_server_session(state_, session_id));
    }

    bool send(SessionId session_id, const void *data, uint32_t length) const;
    bool sendfile(SessionId session_id, const char *file, uint32_t l_file, off_t offset, size_t length) const;
    bool close(SessionId session_id, bool reset = false) const;

    void set_max_connection(uint32_t _max_connection);

    const char *get_startup_error_message() const;
};

namespace detail {
template <size_t N> void check_record(uint32_t kind, size_t size, size_t alignment, const size_t (&offsets)[N]) {
    if (!swr_v1_server_record_layout(kind, size, alignment, offsets, N)) std::abort();
}
// Checks native's view of each record crates/server lays out (crates/server/src/layout.rs) and
// aborts on a mismatch. php_swoole_server_minit and the C++ test fixtures run it before they
// make a Server.
inline void check_server_layouts() {
#define SWR_CONFIG_OFFSET(type, name) offsetof(swr_v1_server_config, name),
    check_record(0, sizeof(swr_v1_server_config), alignof(swr_v1_server_config),
                 {SWR_SERVER_CONFIG_FIELDS(SWR_CONFIG_OFFSET)});
#undef SWR_CONFIG_OFFSET
    check_record(1, sizeof(swr_v1_server_tables), alignof(swr_v1_server_tables), {offsetof(swr_v1_server_tables, connection_list), offsetof(swr_v1_server_tables, session_list), offsetof(swr_v1_server_tables, port_gs_list), offsetof(swr_v1_server_tables, max_connection), offsetof(swr_v1_server_tables, enable_accept_timer), offsetof(swr_v1_server_tables, reactor_pipe_num), offsetof(swr_v1_server_tables, reactor_threads), offsetof(swr_v1_server_tables, ports), offsetof(swr_v1_server_tables, message_bus)});
    check_record(2, sizeof(Session), alignof(Session), {offsetof(Session, id), offsetof(Session, fd)});
    check_record(3, sizeof(Connection), alignof(Connection), {offsetof(Connection, active), offsetof(Connection, socket_type), offsetof(Connection, fd), offsetof(Connection, worker_id), offsetof(Connection, session_id), offsetof(Connection, ssl), offsetof(Connection, ssl_ready), offsetof(Connection, overflow), offsetof(Connection, high_watermark), offsetof(Connection, http_upgrade), offsetof(Connection, http2_stream), offsetof(Connection, websocket_compression), offsetof(Connection, close_actively), offsetof(Connection, close_queued), offsetof(Connection, closing), offsetof(Connection, close_reset), offsetof(Connection, peer_closed), offsetof(Connection, protect), offsetof(Connection, close_notify), offsetof(Connection, close_force), offsetof(Connection, reactor_id), offsetof(Connection, close_errno), offsetof(Connection, recv_queued_bytes), offsetof(Connection, send_queued_bytes), offsetof(Connection, waiting_time), offsetof(Connection, local_port), offsetof(Connection, local_addr_index), offsetof(Connection, timer), offsetof(Connection, info), offsetof(Connection, object), offsetof(Connection, socket), offsetof(Connection, connect_time), offsetof(Connection, last_recv_time), offsetof(Connection, last_send_time), offsetof(Connection, last_dispatch_time), offsetof(Connection, uid), offsetof(Connection, websocket_status), offsetof(Connection, websocket_buffer), offsetof(Connection, ssl_client_cert), offsetof(Connection, ssl_client_cert_pid), offsetof(Connection, lock), offsetof(Connection, closed), offsetof(Connection, server_fd)});
#define SWR_PORT_OFFSET(type, name) offsetof(swr_v1_port_config, name),
    check_record(4, sizeof(swr_v1_port_config), alignof(swr_v1_port_config), {SWR_PORT_CONFIG_FIELDS(SWR_PORT_OFFSET)});
#undef SWR_PORT_OFFSET
    check_record(5, sizeof(ServerCounters), alignof(ServerCounters), {offsetof(ServerCounters, master_pid), offsetof(ServerCounters, manager_pid), offsetof(ServerCounters, session_round), offsetof(ServerCounters, start), offsetof(ServerCounters, shutdown), offsetof(ServerCounters, max_fd), offsetof(ServerCounters, min_fd), offsetof(ServerCounters, onstart_called), offsetof(ServerCounters, start_time), offsetof(ServerCounters, connection_num), offsetof(ServerCounters, connection_nums), offsetof(ServerCounters, tasking_num), offsetof(ServerCounters, max_concurrency), offsetof(ServerCounters, concurrency), offsetof(ServerCounters, abort_count), offsetof(ServerCounters, accept_count), offsetof(ServerCounters, close_count), offsetof(ServerCounters, dispatch_count), offsetof(ServerCounters, request_count), offsetof(ServerCounters, response_count), offsetof(ServerCounters, total_recv_bytes), offsetof(ServerCounters, total_send_bytes), offsetof(ServerCounters, pipe_packet_msg_id), offsetof(ServerCounters, task_count), offsetof(ServerCounters, spinlock)});
    check_record(6, sizeof(PortCounters), alignof(PortCounters), {offsetof(PortCounters, connection_num), offsetof(PortCounters, connection_nums), offsetof(PortCounters, abort_count), offsetof(PortCounters, accept_count), offsetof(PortCounters, close_count), offsetof(PortCounters, dispatch_count), offsetof(PortCounters, request_count), offsetof(PortCounters, response_count), offsetof(PortCounters, total_recv_bytes), offsetof(PortCounters, total_send_bytes)});
    check_record(7, sizeof(swr_v1_server_callback)*SWR_SERVER_CB_COUNT, alignof(swr_v1_server_callback), {offsetof(swr_v1_server_callback, invoke), offsetof(swr_v1_server_callback, ctx), offsetof(swr_v1_server_callback, drop)});
    check_record(8, sizeof(ServerGS), alignof(ServerGS),
                 {offsetof(ServerGS, task_workers), offsetof(ServerGS, event_workers), offsetof(ServerGS, manager_barrier)});
}
}  // namespace detail

// The layouts and numbering Rust mirrors (crates/process, crates/server): a drift fails
// every build that includes this header.
static_assert(sizeof(Worker) == sizeof(swr_v1_worker_record) &&
                  offsetof(Worker, pool) == offsetof(swr_v1_worker_record, pool) &&
                  offsetof(Worker, msgqueue_mode) == offsetof(swr_v1_worker_record, msgqueue_mode),
              "Worker layout drift");
static_assert(sizeof(Factory) == 2 * sizeof(void *) && offsetof(Factory, manager_) == sizeof(void *), "Factory layout");
#define SWR_POOL_FIELD(field, mirror)                                                                                  \
    static_assert(offsetof(ProcessPool, field) == offsetof(swr_v1_pool_record, mirror), #field " drift")
static_assert(sizeof(ProcessPool) == sizeof(swr_v1_pool_record), "ProcessPool layout drift (size)");
SWR_POOL_FIELD(running, running);
SWR_POOL_FIELD(reload_init, reload_init);
SWR_POOL_FIELD(read_message, read_message);
SWR_POOL_FIELD(started, started);
SWR_POOL_FIELD(schedule_by_sysvmsg, schedule_by_sysvmsg);
SWR_POOL_FIELD(async, async);
SWR_POOL_FIELD(ipc_mode, ipc_mode);
SWR_POOL_FIELD(protocol_type_, protocol_type);
SWR_POOL_FIELD(master_pid, master_pid);
SWR_POOL_FIELD(max_wait_time, max_wait_time);
SWR_POOL_FIELD(reload_count, reload_count);
SWR_POOL_FIELD(reload_last_time, reload_last_time);
SWR_POOL_FIELD(type, type);
SWR_POOL_FIELD(start_id, start_id);
SWR_POOL_FIELD(use_msgqueue, use_msgqueue);
SWR_POOL_FIELD(use_socket, use_socket);
SWR_POOL_FIELD(max_packet_size_, max_packet_size);
SWR_POOL_FIELD(msgqueue_key, msgqueue_key);
SWR_POOL_FIELD(worker_num, worker_num);
SWR_POOL_FIELD(max_request, max_request);
SWR_POOL_FIELD(max_request_grace, max_request_grace);
SWR_POOL_FIELD(scheduler_warning, scheduler_warning);
SWR_POOL_FIELD(warning_time, warning_time);
SWR_POOL_FIELD(onStart, on_start);
SWR_POOL_FIELD(onShutdown, on_shutdown);
SWR_POOL_FIELD(onBeforeReload, on_before_reload);
SWR_POOL_FIELD(onAfterReload, on_after_reload);
SWR_POOL_FIELD(onTask, on_task);
SWR_POOL_FIELD(onWorkerStart, on_worker_start);
SWR_POOL_FIELD(onMessage, on_message);
SWR_POOL_FIELD(onWorkerExit, on_worker_exit);
SWR_POOL_FIELD(onWorkerStop, on_worker_stop);
SWR_POOL_FIELD(onWorkerError, on_worker_error);
SWR_POOL_FIELD(onWorkerMessage, on_worker_message);
SWR_POOL_FIELD(onWorkerNotFound, on_worker_not_found);
SWR_POOL_FIELD(main_loop, main_loop);
SWR_POOL_FIELD(round_id, round_id);
SWR_POOL_FIELD(workers, workers);
SWR_POOL_FIELD(pipes, pipes);
SWR_POOL_FIELD(registry_, registry);
SWR_POOL_FIELD(queue, queue);
SWR_POOL_FIELD(ipc_, ipc);
SWR_POOL_FIELD(message_box, message_box);
SWR_POOL_FIELD(message_bus, message_bus);
SWR_POOL_FIELD(reload_task, reload_task);
SWR_POOL_FIELD(ptr, ptr);
#undef SWR_POOL_FIELD
static_assert(SW_SERVER_EVENT_RECV_DATA == 0 && SW_SERVER_EVENT_RECV_DGRAM == 1 && SW_SERVER_EVENT_SEND_DATA == 2 &&
                  SW_SERVER_EVENT_SEND_FILE == 3 && SW_SERVER_EVENT_CLOSE == 4 && SW_SERVER_EVENT_CONNECT == 5 &&
                  SW_SERVER_EVENT_CLOSE_FORWARD == 7 && SW_SERVER_EVENT_FINISH == 9 &&
                  SW_SERVER_EVENT_PIPE_MESSAGE == 10 && SW_SERVER_EVENT_BUFFER_FULL == 13 &&
                  SW_SERVER_EVENT_BUFFER_EMPTY == 14 && SW_SERVER_EVENT_SHUTDOWN == 16 &&
                  SW_SERVER_EVENT_COMMAND_REQUEST == 17 && SW_SERVER_EVENT_COMMAND_RESPONSE == 18 &&
                  SW_EVENT_WORKER == 2 && SW_WORKER_IDLE == 2 && SW_WORKER_MESSAGE_STOP == 1,
              "crates/process events");
static_assert(SW_MESSAGE_BOX_SIZE == 65536 && SW_INPUT_BUFFER_SIZE == 2 * 1024 * 1024 && SW_WORKER_MAX_WAIT_TIME == 3 &&
                  SW_IPC_BUFFER_MAX_SIZE == 64 * 1024,
              "pool constants drift");
static_assert(sizeof(DataHead) == 40 && offsetof(DataHead, time) == 32 && offsetof(DataHead, len) == 16 && offsetof(DataHead, type) == 22 &&
                  offsetof(EventData, data) == sizeof(DataHead) && sizeof(EventData) == SW_IPC_MAX_SIZE &&
                  sizeof(String) == 40 && offsetof(WorkerStopMessage, pid) == 0 &&
                  offsetof(WorkerStopMessage, worker_id) == 4 && sizeof(RecvData) == 48 && sizeof(ExitStatus) == 8,
              "pool message layout drift");
static_assert(SW_PROTOCOL_TASK == 1 && SW_PROTOCOL_STREAM == 2 && SW_PROTOCOL_MESSAGE == 3 && SW_MASTER == 1 &&
                  SW_MANAGER == 3 && SW_WORKER == 2 && SW_TASK_WORKER == 4 && SW_USER_WORKER == 5,
              "pool enums drift");
// What crates/server, crates/phpserver and crates/phphttp mirror.
static_assert(sizeof(Server) == 4 * sizeof(void *) && sizeof(ListenPort) == sizeof(void *) &&
                  sizeof(ServerPortGS) == 80 && sizeof(Session) == 16 && sizeof(sw_atomic_t) == 4 &&
                  offsetof(ReactorThread, heartbeat_timer) == 32 && offsetof(ReactorThread, message_bus) == 40 &&
                  sizeof(ReactorThread) == 56 && sizeof(EventData) <= 8192 && alignof(EventData) <= 8 &&
                  offsetof(network::SendfileTask, length) == 8 && offsetof(RecvData, data) == sizeof(DataHead) &&
                  offsetof(DgramPacket, socket_addr) == 4 &&
                  offsetof(DgramPacket, length) == 4 + sizeof(network::Address) && offsetof(String, length) == 0 &&
                  offsetof(String, str) == 3 * sizeof(size_t),
              "server record layout drift");
static_assert(SW_ACCEPT_MAX_COUNT == 64 && SW_MAX_CONNECTION == 100000 && SW_SESSION_LIST_SIZE == 1024 * 1024 &&
                  SW_MAX_THREAD_NCPU == 4 && SW_MAX_WORKER_NCPU == 1000 && SW_TIMER_MIN_SEC == 0.001 &&
                  SW_BUFFER_MIN_SIZE == 65536 && SW_IPC_BUFFER_SIZE == SW_IPC_MAX_SIZE - 40 &&
                  SW_MAX_LISTEN_PORT == 60000 && SW_SYSTEMD_FDS_START == 3 && SW_MAX_CONCURRENT_TASK == 1024 &&
                  SW_WORKER_MIN_REQUEST == 10 && SW_HTTP_FORM_KEYLEN == 512 && SW_HTTP_UPLOAD_TMPDIR_SIZE == 256 &&
                  SW_STACK_BUFFER_SIZE == 65536 && SW_HTTP_HEADER_KEY_SIZE == 128 && SW_HTTP_MAX_APPEND_DATA == 16384,
              "server limits drift");
static_assert(Server::MODE_BASE == 1 && Server::MODE_PROCESS == 2 && Server::MODE_THREAD == 3 &&
                  Server::TASK_IPC_UNIXSOCK == 1 && Server::TASK_IPC_MSGQUEUE == 2 &&
                  Server::TASK_IPC_PREEMPTIVE == 3 && Server::TASK_IPC_STREAM == 4 && SW_IPC_UNIXSOCK == 1 &&
                  SW_IPC_MSGQUEUE == 2 && SW_IPC_SOCKET == 3 && Server::DISPATCH_ROUND == 1 &&
                  Server::DISPATCH_FDMOD == 2 && Server::DISPATCH_IDLE_WORKER == 3 && Server::DISPATCH_IPMOD == 4 &&
                  Server::DISPATCH_UIDMOD == 5 && Server::DISPATCH_USERFUNC == 6 && Server::DISPATCH_CO_CONN_LB == 8 &&
                  Server::DISPATCH_CO_REQ_LB == 9 && Server::DISPATCH_CONCURRENT_LB == 10 &&
                  Server::DISPATCH_RESULT_DISCARD_PACKET == -1 && Server::DISPATCH_RESULT_CLOSE_CONNECTION == -2 &&
                  Server::DISPATCH_RESULT_USERFUNC_FALLBACK == -3 && Server::CLOSE_RESET == (1 << 1) &&
                  Server::CLOSE_ACTIVELY == (1 << 2),
              "server modes drift");
static_assert(SW_SOCK_TCP == 1 && SW_SOCK_UDP == 2 && SW_SOCK_UDP6 == 4 && SW_SOCK_UNIX_STREAM == 5 &&
                  SW_SOCK_UNIX_DGRAM == 6 && SW_SOCK_SSL == (1u << 9) && SW_EVENT_DATA_PTR == (1 << 1) &&
                  SW_CONTINUE == 1 && SW_WAIT == 2 && SW_ERROR == 4 && SW_READY == 5 &&
                  BufferChunk::TYPE_DATA == 0 && BufferChunk::TYPE_SENDFILE == 1 && BufferChunk::TYPE_CLOSE == 2 &&
                  SW_LOG_TRACE == 1 && SW_LOG_NOTICE == 3 && SW_LOG_WARNING == 4,
              "socket, return code, output and log enums drift");
static_assert(SW_SERVER_EVENT_PAUSE_RECV == 11 && SW_SERVER_EVENT_RESUME_RECV == 12 &&
                  int(SW_SERVER_EVENT_CLOSE_FORCE) == int(SWR_SERVER_EVENT_CLOSE_FORCE) &&
                  int(SW_SERVER_EVENT_CLOSE_FORWARD) == int(SWR_SERVER_EVENT_CLOSE_FORWARD) &&
                  int(SW_SERVER_EVENT_FINISH) == int(SWR_SERVER_EVENT_FINISH) &&
                  int(SW_SERVER_EVENT_PIPE_MESSAGE) == int(SWR_SERVER_EVENT_PIPE_MESSAGE) &&
                  int(SW_SERVER_EVENT_INCOMING) == int(SWR_SERVER_EVENT_INCOMING) &&
                  int(SW_SERVER_EVENT_SHUTDOWN) == int(SWR_SERVER_EVENT_SHUTDOWN) &&
                  int(SW_SERVER_EVENT_COMMAND_REQUEST) == int(SWR_SERVER_EVENT_COMMAND_REQUEST) &&
                  int(SW_SERVER_EVENT_COMMAND_RESPONSE) == int(SWR_SERVER_EVENT_COMMAND_RESPONSE) &&
                  int(SW_SERVER_EVENT_SHUTDOWN_SIGNAL) == int(SWR_SERVER_EVENT_SHUTDOWN_SIGNAL),
              "server pipe events drift");
static_assert(SW_TASK_TMPFILE == 1 && SW_TASK_SERIALIZE == 1 << 1 && SW_TASK_NONBLOCK == 1 << 2 &&
                  SW_TASK_CALLBACK == 1 << 3 && SW_TASK_WAITALL == 1 << 4 && SW_TASK_COROUTINE == 1 << 5 &&
                  SW_TASK_PEEK == 1 << 6 && SW_TASK_NOREPLY == 1 << 7 && SW_WORKER_BUSY == 1 && SW_WORKER_EXIT == 3,
              "task flags and worker states drift");
static_assert(SW_ERROR_PHP_FATAL_ERROR == 503 && SW_ERROR_INVALID_PARAMS == 505 &&
                  SW_ERROR_OPERATION_NOT_SUPPORT == 507 && SW_ERROR_PHP_RUNTIME_NOTICE == 510 &&
                  SW_ERROR_NO_PAYLOAD == 550 && SW_ERROR_SESSION_CLOSED == 1004 && SW_ERROR_SESSION_NOT_EXIST == 1005 &&
                  SW_ERROR_OUTPUT_SEND_YIELD == 1010 && SW_ERROR_TASK_DISPATCH_FAIL == 2002 &&
                  SW_ERROR_HTTP_CONTEXT_UNAVAILABLE == 7106 && SW_ERROR_SERVER_INVALID_REQUEST == 9010 &&
                  SW_ERROR_SERVER_IS_NOT_REGULAR_FILE == 9013 && SW_ERROR_SERVER_UNRELATED_THREAD == 9016,
              "error codes drift");

}  // namespace swoole

static inline swoole::Server *sw_server() {
    return SwooleG.server;
}
