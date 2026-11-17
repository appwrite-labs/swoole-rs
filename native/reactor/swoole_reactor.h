/* SPDX-License-Identifier: Apache-2.0 */
/* Swoole's include/swoole_reactor.h, owned here: the Reactor class over Rust's reactor
 * state (swoole_rs_reactor.h). Upstream's file is deleted, so `#include "swoole_reactor.h"`
 * finds this copy on the Rust prefix's include path. */
#pragma once

#include "swoole_socket.h"
#include "swoole_rs_reactor.h"

namespace swoole {

// Methods over Rust's reactor state, which a Reactor pointer is: its fields are
// swr_v1_reactor_head's. swoole_event_init/free (`corebase::reactor`) make and free it.
class Reactor : public swr_v1_reactor_head {
  public:
    Reactor() = delete;
    ~Reactor() = delete;
    swr_v1_reactor_state *handle() const { return state_.owner; }

    // wait and the end callbacks are Rust's (`corebase::reactor`'s callbacks.rs).
    int wait();
    bool isset_handler(int fd_type, int event) const;

    bool ready() const {
        return ready_;
    }

    void set_timeout_msec(int mesc) {
        timeout_msec = mesc;
    }
};

}  // namespace swoole
