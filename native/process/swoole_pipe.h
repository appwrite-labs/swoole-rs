/* SPDX-License-Identifier: Apache-2.0 */
/* Swoole's include/swoole_pipe.h, owned here: SocketPair, Pipe and UnixSocket over the
 * Rust record (SWR_SOCKET_PAIR_RECORD_FIELDS); their methods are Rust under upstream's
 * symbols. */
#pragma once

#include "swoole_socket.h"
/* The Rust records the views below point at, opaque here (crates/corebase, crates/process, crates/ossupport). */
typedef struct swr_v1_process_worker_registry swr_v1_process_worker_registry;
typedef struct swr_v1_process_pool_ipc swr_v1_process_pool_ipc;
typedef struct swr_v1_process_reload_task swr_v1_process_reload_task;
typedef struct swr_v1_os_msg_queue swr_v1_os_msg_queue;
#include "swoole_rs_process_views.h"

enum swPipe_close_which {
    SW_PIPE_CLOSE_MASTER = 1,
    SW_PIPE_CLOSE_WORKER = 2,
    SW_PIPE_CLOSE_READ = 3,
    SW_PIPE_CLOSE_WRITE = 4,
    SW_PIPE_CLOSE_BOTH = 0,
};

namespace swoole {
class SocketPair {
  protected:
    SWR_SOCKET_PAIR_RECORD_FIELDS

  public:
    ~SocketPair();

    ssize_t read(void *_buf, size_t length) const;
    ssize_t write(const void *_buf, size_t length) const;
    void clean() const;
    bool close(int which = 0);

    network::Socket *get_socket(bool _master) const {
        return _master ? master_socket : worker_socket;
    }

    bool ready() const {
        return master_socket != nullptr && worker_socket != nullptr;
    }

    void set_timeout(double _timeout);
    void set_blocking(bool _blocking);
};

class Pipe : public SocketPair {
  public:
    explicit Pipe(bool blocking);
};

class UnixSocket : public SocketPair {
  public:
    UnixSocket(bool blocking, int _protocol);
    bool set_buffer_size(size_t _size) const;
};
}  // namespace swoole
