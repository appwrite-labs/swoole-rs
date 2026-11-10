/* SPDX-License-Identifier: Apache-2.0 */
/* Workers and process pools: the C++ declarations over the Rust records (crates/process,
 * swoole_rs_process_views.h) whose methods Rust exports under their upstream symbols.
 * Was include/swoole_process_pool.h, then the process half of swoole_server.h. */
#pragma once

#include "swoole_string.h"
#include "swoole_socket.h"
#include "swoole_timer.h"
#include "swoole_reactor.h"
#include "swoole_signal.h"
#include "swoole_protocol.h"
#include "swoole_lock.h"
#include "swoole_pipe.h"
#include "swoole_rs_ossupport.h"
#include "swoole_message_bus.h"
#include "swoole_rs_corebase_worker_registry.h"
// Worker's scalar process fields are owned by the Rust record (crates/process); its
// methods read/write them through swr_v1_worker_* accessors over the byte-exact
// swr_v1_worker_record mirror. #629.
#include "swoole_rs_process.h"

#include <sys/wait.h>

#include <csignal>
#include <unordered_set>

enum swWorkerStatus {
    SW_WORKER_BUSY = 1,
    SW_WORKER_IDLE = 2,
    SW_WORKER_EXIT = 3,
};

enum swWorkerType {
    SW_MASTER = 1,
    SW_WORKER = 2,
    SW_MANAGER = 3,
    SW_EVENT_WORKER = 2,
    SW_TASK_WORKER = 4,
    SW_USER_WORKER = 5,
};

enum swIPCMode {
    SW_IPC_NONE = 0,
    SW_IPC_UNIXSOCK = 1,
    SW_IPC_MSGQUEUE = 2,
    SW_IPC_SOCKET = 3,
};

SW_API void swoole_set_worker_id(swoole::WorkerId worker_id);
SW_API void swoole_set_worker_type(int type);

namespace swoole {
enum WorkerMessageType {
    SW_WORKER_MESSAGE_STOP = 1,
};

enum ProtocolType {
    SW_PROTOCOL_TASK = 1,
    SW_PROTOCOL_STREAM,
    SW_PROTOCOL_MESSAGE,
};

struct WorkerStopMessage {
    pid_t pid;
    uint16_t worker_id;
};

class ExitStatus {
    pid_t pid_;
    int status_;

  public:
    ExitStatus(pid_t _pid, int _status) : pid_(_pid), status_(_status) {}

    pid_t get_pid() const {
        return pid_;
    }

    int get_status() const {
        return status_;
    }

    int get_code() const {
        return WEXITSTATUS(status_);
    }

    int get_signal() const {
        return WTERMSIG(status_);
    }
};

struct ProcessPool;
struct Worker;

#include "swoole_rs_process_views.h"

struct Worker {
    SWR_WORKER_RECORD_FIELDS
    void set_max_request(uint32_t max_request, uint32_t max_request_grace);
    /**
     * Init global state for worker process.
     * Must be called after the process is spawned and before the main loop is executed.
     */
    void init();
    void shutdown();
    bool is_shutdown();
    static bool is_running();
};

struct ProcessPool {
    SWR_POOL_RECORD_FIELDS
    Worker *get_worker(WorkerId worker_id) const {
        return &(workers[worker_id - start_id]);
    }

    static TaskId get_task_id(const EventData *task) {
        return task->info.fd;
    }

    void set_max_packet_size(uint32_t _max_packet_size) {
        // SysV message queues have a fixed receive limit and ignore max_package_size.
        max_packet_size_ = use_msgqueue ? SW_MSGMAX : _max_packet_size;
    }

    void set_protocol(ProtocolType _protocol_type);
    void set_type(int _type);
    void set_max_request(uint32_t _max_request, uint32_t _max_request_grace);
    bool detach();
    int wait();
    int start();
    bool shutdown();
    bool reload();
    void reopen_logger();
    // Swoole\Process\Pool's signal handler, in `worker` (null in the master).
    void signal(int signo, Worker *worker);

    pid_t spawn(Worker *worker);
    void stop(Worker *worker);
    swResultCode dispatch(EventData *data, int *worker_id);
    int response(const char *data, uint32_t length) const;
    swResultCode dispatch_sync(EventData *data, int *dst_worker_id);
    void add_worker(Worker *worker) const;
    bool del_worker(const Worker *worker) const;
    Worker *get_worker_by_pid(pid_t pid) const;
    void destroy();
    int create(uint32_t worker_num, key_t msgqueue_key = 0, swIPCMode ipc_mode = SW_IPC_NONE);
    int create_message_bus();
    int push_message(uint8_t _type, const void *data, size_t length) const;
    int push_message(const EventData *msg) const;
    bool send_message(WorkerId worker_id, const char *message, size_t l_message) const;
    int listen(const char *socket_file, int backlog) const;
    int listen(const char *host, int port, int backlog) const;
};
};  // namespace swoole

static sw_inline int swoole_kill(pid_t _pid, int _sig) {
    return kill(_pid, _sig);
}

