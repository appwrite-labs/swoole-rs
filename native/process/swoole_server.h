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

// Thread mode's factory, as workers.cc's thread ops see it.
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

}  // namespace swoole

static inline swoole::Server *sw_server() {
    return SwooleG.server;
}
