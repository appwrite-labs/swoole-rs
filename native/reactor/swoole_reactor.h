/* SPDX-License-Identifier: Apache-2.0 */
/* Swoole's include/swoole_reactor.h, owned here: the Reactor class over Rust's reactor
 * state (swoole_rs_reactor.h). Upstream's file is deleted, so `#include "swoole_reactor.h"`
 * finds this copy on the Rust prefix's include path. */
#pragma once

#include "swoole_socket.h"
#include "swoole_rs_reactor.h"
static_assert(SW_EVENT_READ == (1 << 9) && SW_EVENT_WRITE == (1 << 10) && SW_EVENT_DEAULT == 256 && SW_EVENT_ERROR == (1 << 11), "Reactor event ABI");

namespace swoole {

// Methods over Rust's reactor state, which a Reactor pointer is: its fields are
// swr_v1_reactor_head's. swoole_event_init/free (crates/reactor) make and free it.
class Reactor : public swr_v1_reactor_head {
  public:
    enum EndCallback {
        PRIORITY_TIMER = 0,
        PRIORITY_DEFER_TASK,
        PRIORITY_IDLE_TASK,
        PRIORITY_SIGNAL_CALLBACK,
        PRIORITY_TRY_EXIT,
        PRIORITY_MALLOC_TRIM,
        PRIORITY_WORKER_CALLBACK,
        /**
         * PRIORITY_IOURING_SUBMIT must be the last one, as other callback functions might allocate new SQEs.
         * It is essential to ensure that the SQE is submitted before the next event loop iteration and before the
         * epoll_wait() call.
         */
        PRIORITY_IOURING_SUBMIT,
    };

    Reactor() = delete;
    ~Reactor() = delete;
    swr_v1_reactor_state *handle() const { return state_.owner; }

    // ReactorEpoll/ReactorKqueue/ReactorPoll add, set and del are Rust's (swr_v1_reactor_change).
    int change(network::Socket *socket, uint32_t op, int events) const;

    int add(network::Socket *socket, int events) const {
        return change(socket, 0, events);
    }

    int set(network::Socket *socket, int events) const {
        return change(socket, 1, events);
    }

    int del(network::Socket *socket) const {
        return change(socket, 2, 0);
    }

    // wait and the end callbacks are Rust's (crates/reactor's callbacks.rs), keyed as
    // upstream's map was and holding plain functions.
    int wait();
    void set_end_callback(EndCallback _id, void (*fn)(void)) {
        swr_v1_reactor_set_end(handle(), _id, fn);
    }
    void erase_end_callback(EndCallback _id) {
        swr_v1_reactor_set_end(handle(), _id, nullptr);
    }
    void set_handler(int fd_type, int event, ReactorHandler handler);
    bool isset_handler(int fd_type, int event) const;
    void drain_write_buffer(network::Socket *socket);

    bool ready() const {
        return ready_;
    }

    bool exists(const int fd) const {
        return get_socket(fd) != nullptr;
    }

    void set_timeout_msec(int mesc) {
        timeout_msec = mesc;
    }

    size_t get_event_num() const {
        return swr_v1_reactor_registry_count(state_.registry);
    }

    network::Socket *get_socket(const int fd) const {
        return static_cast<network::Socket *>(swr_v1_reactor_registry_get(state_.registry, fd));
    }
};

}  // namespace swoole

SW_API int swoole_event_init(int flags);
SW_API int swoole_event_add(swoole::network::Socket *socket, int events);
SW_API int swoole_event_del(swoole::network::Socket *socket);
SW_API void swoole_event_defer(void (*fn)(void *), void *private_data);
SW_API ssize_t swoole_event_write(swoole::network::Socket *socket, const void *data, size_t len);
SW_API ssize_t swoole_event_writev(swoole::network::Socket *socket, const iovec *iov, size_t iovcnt);
SW_API swoole::network::Socket *swoole_event_get_socket(int fd);
SW_API int swoole_event_wait();
SW_API int swoole_event_free();
SW_API void swoole_event_set_handler(int fd_type, int event, swoole::ReactorHandler handler);
SW_API bool swoole_event_is_available();
