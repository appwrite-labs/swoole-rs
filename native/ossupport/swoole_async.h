/* SPDX-License-Identifier: Apache-2.0 */
/* Native view of the Rust async event and its resolver record. */
#pragma once

#include "swoole_socket.h"

#include <vector>

struct swr_v1_core_resolver;

namespace swoole {

struct AsyncEvent {
    size_t task_id;
    uint8_t canceled;
    int error;
    // The request's resolver record: each event, and each pool copy, holds its own
    // reference, so a request may go before the pool finishes.
    swr_v1_core_resolver *data;
    ssize_t retval;
    network::Socket *pipe_socket;
    double timestamp;
    void *object;
    void (*handler)(AsyncEvent *event);
    void (*callback)(AsyncEvent *event);
};
static_assert(sizeof(AsyncEvent) == 72 && offsetof(AsyncEvent, error) == 12 && offsetof(AsyncEvent, retval) == 24 &&
                  offsetof(AsyncEvent, object) == 48 && offsetof(AsyncEvent, callback) == 64,
              "crates/corebase/src/offload.rs mirrors AsyncEvent");

namespace async {

typedef void (*Handler)(AsyncEvent *event);


}  // namespace async
}  // namespace swoole
