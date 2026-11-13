/* SPDX-License-Identifier: Apache-2.0 */
/* Workers and process pools: the C++ declarations over the Rust records (crates/process,
 * swoole_rs_process_views.h), whose methods are Rust functions.
 * Was include/swoole_process_pool.h, then the process half of swoole_server.h. */
#pragma once

#include "swoole_string.h"
#include "swoole_socket.h"
#include "swoole_reactor.h"
#include "swoole_protocol.h"
#include "swoole_lock.h"
#include "swoole_pipe.h"
#include "swoole_message_bus.h"
// Worker and ProcessPool's fields are the Rust records' (crates/process), which
// tools/process_records.py writes into swoole_rs_process_views.h; tools/headers.sh
// --check fails when that header is stale. #629.

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

SW_API int swoole_get_worker_type();
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
};
};  // namespace swoole

static sw_inline int swoole_kill(pid_t _pid, int _sig) {
    return kill(_pid, _sig);
}

