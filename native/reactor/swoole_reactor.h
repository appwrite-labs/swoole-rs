/* SPDX-License-Identifier: Apache-2.0 */
/* Swoole's include/swoole_reactor.h, owned here: the Reactor class over Rust's reactor
 * state (swoole_rs_reactor.h). Upstream's file is deleted, so `#include "swoole_reactor.h"`
 * finds this copy on the Rust prefix's include path. */
#pragma once

#include "swoole_socket.h"
#include "swoole_rs_reactor.h"

namespace swoole {

// Methods over Rust's reactor state, which a Reactor pointer is: its fields are
// swr_v1_reactor_head's. swoole_event_init/free (crates/reactor) make and free it.
class Reactor : public swr_v1_reactor_head {
  public:
    Reactor() = delete;
    ~Reactor() = delete;
    swr_v1_reactor_state *handle() const { return state_.owner; }

    // wait and the end callbacks are Rust's (crates/reactor's callbacks.rs), keyed by
    // upstream's PRIORITY_* values (Rust's copies) and holding plain functions.
    int wait();
    void set_end_callback(int _id, void (*fn)(void)) {
        swr_v1_reactor_set_end(handle(), _id, fn);
    }
    void erase_end_callback(int _id) {
        swr_v1_reactor_set_end(handle(), _id, nullptr);
    }
    bool isset_handler(int fd_type, int event) const;

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
SW_API swoole::network::Socket *swoole_event_get_socket(int fd);
SW_API int swoole_event_wait();
SW_API bool swoole_event_is_available();
