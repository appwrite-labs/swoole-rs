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
#include "swoole_rs_workers.h"
#include "swoole_rs_port_state.h"

#include <string>
#include <queue>
#include <thread>
#include <mutex>
#include <atomic>
#include <unordered_map>
#include <condition_variable>

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

class Server : public detail::ServerFields {
    // The native halves of crates/process/src/workers, one per source file.
    template <int>
    friend struct WorkersNative;

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

    // Worker (and task worker) chroot, user and group.
    RustStringView chroot_{text(SWR_SERVER_TEXT_CHROOT)};
    RustStringView user_{text(SWR_SERVER_TEXT_USER)};
    RustStringView group_{text(SWR_SERVER_TEXT_GROUP)};

    MessageBus message_bus;

    decltype(swr_v1_server_tables::ports) &ports = tables_->ports;
    std::vector<std::shared_ptr<UnixSocket>> worker_pipes;

    ListenPort *get_primary_port() const {
        return ports.front();
    }

    ListenPort *get_port_by_server_fd(const int server_fd) const {
        return static_cast<ListenPort *>(connection_list[server_fd].object);
    }

    network::Socket *get_command_reply_socket() const {
        return is_base_mode() ? get_worker(0)->pipe_master : pipe_command->get_socket(false);
    }

    network::Socket *get_worker_pipe_master_in_message_bus(const Worker *worker) {
        return is_thread_mode() ? get_worker_message_bus()->get_pipe_socket(worker->pipe_master) : worker->pipe_master;
    }

    network::Socket *get_worker_pipe_worker_in_message_bus(const Worker *worker) {
        return is_thread_mode() ? get_worker_message_bus()->get_pipe_socket(worker->pipe_worker) : worker->pipe_worker;
    }

    network::Socket *get_worker_pipe_master_in_message_bus(WorkerId id) {
        return get_worker_pipe_master_in_message_bus(get_worker(id));
    }

    // [Worker|Master] The pipe of the worker that serves this session's reactor.
    network::Socket *get_reactor_pipe_socket(const SessionId session_id, int reactor_id) const {
        const int pipe_index = session_id % reactor_pipe_num;
        int pipe_worker_id = reactor_id + (pipe_index * reactor_num);
        Worker *worker = get_worker(pipe_worker_id);
        return worker->pipe_worker;
    }


    detail::ServerStrings locations{state_,0};
    detail::ServerStrings http_compression_types{state_,2};

    Barrier reactor_thread_barrier = {};

    // Master process
    detail::Callback<void(Server *)> &onStart = callback(SWR_SERVER_CB_START);
    detail::Callback<void(Server *)> &onShutdown = callback(SWR_SERVER_CB_SHUTDOWN);
    // Manager process
    detail::Callback<void(Server *, Worker *, const ExitStatus &)> &onWorkerError = callback(SWR_SERVER_CB_WORKER_ERROR);
    detail::Callback<void(Server *)> &onBeforeReload = callback(SWR_SERVER_CB_BEFORE_RELOAD);
    detail::Callback<void(Server *)> &onAfterReload = callback(SWR_SERVER_CB_AFTER_RELOAD);
    // Worker process
    detail::Callback<void(Server *, EventData *)> &onPipeMessage = callback(SWR_SERVER_CB_PIPE_MESSAGE);
    detail::Callback<void(Server *, Worker *)> &onWorkerStart = callback(SWR_SERVER_CB_WORKER_START);
    detail::Callback<void(Server *, Worker *)> &onWorkerStop = callback(SWR_SERVER_CB_WORKER_STOP);
    detail::Callback<void(Server *, Worker *)> &onWorkerExit = callback(SWR_SERVER_CB_WORKER_EXIT);
    // Connection
    detail::Callback<int(Server *, RecvData *)> &onReceive = callback(SWR_SERVER_CB_RECEIVE);
    detail::Callback<int(Server *, RecvData *)> &onPacket = callback(SWR_SERVER_CB_PACKET);
    detail::Callback<void(Server *, DataHead *)> &onClose = callback(SWR_SERVER_CB_CLOSE);
    detail::Callback<void(Server *, DataHead *)> &onConnect = callback(SWR_SERVER_CB_CONNECT);
    detail::Callback<void(Server *, DataHead *)> &onBufferFull = callback(SWR_SERVER_CB_BUFFER_FULL);
    detail::Callback<void(Server *, DataHead *)> &onBufferEmpty = callback(SWR_SERVER_CB_BUFFER_EMPTY);
    // Task worker
    detail::Callback<int(Server *, EventData *)> &onTask = callback(SWR_SERVER_CB_TASK);
    detail::Callback<int(Server *, EventData *)> &onFinish = callback(SWR_SERVER_CB_FINISH);

    explicit Server(Mode _mode = MODE_BASE);
    ~Server();

    bool set_document_root(const std::string &path);
    void add_http_compression_type(const std::string &type);

    int create();
    int start();
    bool shutdown();
    void destroy();

    int add_worker(Worker *worker);
    ListenPort *add_port(SocketType type, const char *host, int port);
    void abort_worker(Worker *worker) const;
    void reset_worker_counter(Worker *worker) const;

    uint32_t get_idle_worker_num() const;

    TaskId get_task_id(const EventData *task) const {
        return get_task_worker_pool()->get_task_id(task);
    }

    static uint16_t get_command_id(const EventData *cmd) {
        return cmd->info.server_fd;
    }

    pid_t get_master_pid() const {
        return gs->master_pid;
    }

    MessageBus *get_worker_message_bus() {
#ifdef SW_THREAD
        return sw_likely(is_thread_mode()) ? SwooleTG.message_bus : &message_bus;
#else
        return &message_bus;
#endif
    }

    ProcessPool *get_task_worker_pool() const {
        return &gs->task_workers;
    }

    ProcessPool *get_event_worker_pool() const {
        return &gs->event_workers;
    }

    bool is_process_mode() const {
        return mode_ == MODE_PROCESS;
    }

    bool is_base_mode() const {
        return mode_ == MODE_BASE;
    }

    bool is_thread_mode() const {
        return mode_ == MODE_THREAD;
    }

    bool is_enable_coroutine() const {
        if (is_task_worker()) {
            return task_enable_coroutine;
        } else if (is_manager()) {
            return false;
        } else {
            return enable_coroutine;
        }
    }

    Worker *get_worker(uint16_t worker_id) const;
    Worker *get_worker_by_pid(pid_t worker_pid) const;
    void stop_async_worker(Worker *worker);

    size_t get_all_worker_num() const {
        return get_core_worker_num() + user_worker_list.size();
    }

    size_t get_core_worker_num() const {
        return worker_num + task_worker_num;
    }

    ReactorThread *get_thread(int reactor_id) const {
        return &reactor_threads[reactor_id];
    }

    bool is_started() const {
        return gs->start;
    }


    bool is_worker() const {
        return swoole_get_worker_type() == SW_EVENT_WORKER;
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
        return is_thread_mode() && swoole_get_thread_type() == THREAD_WORKER;
    }

    bool is_reactor_thread() {
        return swoole_get_thread_type() == THREAD_REACTOR;
    }


    bool is_healthy_connection(double now, const Connection *conn) const;

    Connection *get_connection_verify_no_ssl(SessionId session_id) const {
        return static_cast<Connection *>(swr_v1_server_connection(state_, session_id, 2));
    }

    Connection *get_connection_verify(SessionId session_id) const {
        return static_cast<Connection *>(swr_v1_server_connection(state_, session_id, 3));
    }

    Connection *get_connection(const int fd) const {
        return static_cast<Connection *>(swr_v1_server_connection(state_, fd, 0));
    }

    Connection *get_connection_by_session_id(SessionId session_id) const {
        return static_cast<Connection *>(swr_v1_server_connection(state_, session_id, 1));
    }

    Session *get_session(SessionId session_id) const {
        return static_cast<Session *>(swr_v1_server_session(state_, session_id));
    }

    void call_worker_error_callback(Worker *worker, const ExitStatus &status);
    void call_command_handler(MessageBus &mb, uint16_t worker_id, network::Socket *sock);
    void foreach_connection(const std::function<void(Connection *)> &callback) const;
    static int accept_connection(Reactor *reactor, Event *event);
    static int close_connection(Reactor *reactor, network::Socket *_socket);

    ssize_t send_to_worker_from_worker(const Worker *dst_worker, const void *buf, size_t len, int flags);

    ssize_t send_to_worker_from_worker(WorkerId id, const EventData *data, int flags) {
        return send_to_worker_from_worker(get_worker(id), data, data->size(), flags);
    }

    bool send(SessionId session_id, const void *data, uint32_t length) const;
    bool sendfile(SessionId session_id, const char *file, uint32_t l_file, off_t offset, size_t length) const;
    bool close(SessionId session_id, bool reset = false) const;

    void init_pipe_sockets(MessageBus *mb) const;

    void set_max_connection(uint32_t _max_connection);

    uint32_t get_max_connection() const {
        return max_connection;
    }

    pid_t restart_worker_process(const ExitStatus &exit_status);



    // [Worker]
    static int reactor_process_main_loop(ProcessPool *pool, Worker *worker);
    static void reactor_thread_main_loop(Server *serv, int reactor_id);
    static bool task_unpack(EventData *task, String *buffer, PacketPtr *packet);

    const char *get_startup_error_message() const;

  private:
    decltype(swr_v1_server_tables::connection_list) &connection_list = tables_->connection_list;
    decltype(swr_v1_server_tables::session_list) &session_list = tables_->session_list;
    decltype(swr_v1_server_tables::max_connection) &max_connection = tables_->max_connection;
    // The number of pipes each reactor maintains.
    decltype(swr_v1_server_tables::reactor_pipe_num) &reactor_pipe_num = tables_->reactor_pipe_num;
    decltype(swr_v1_server_tables::reactor_threads) &reactor_threads = tables_->reactor_threads;
    detail::PendingWorkers<Worker> user_worker_list{worker_registry};

    int create_pipe_buffers();
    int start_master_thread(Reactor *reactor);
};

}  // namespace swoole

static inline swoole::Server *sw_server() {
    return SwooleG.server;
}
