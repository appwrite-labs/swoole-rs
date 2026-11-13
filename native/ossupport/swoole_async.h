/* SPDX-License-Identifier: Apache-2.0 */
/* Swoole's include/swoole_async.h, owned here: what C++ still uses of it.
 * AsyncEvent is a plain record whose layout crates/hooks/src/offload.rs mirrors (asserted
 * below). The DNS requests are Rust's resolver record (crates/corebase/src/
 * resolver_request.rs), which C++ only carries; the
 * pool (AsyncThreads, async::dispatch) is crates/hooks/src/async_threads.rs. Upstream's
 * file is deleted, so `#include "swoole_async.h"` finds this copy on the Rust prefix's
 * include path. */
#pragma once

#include "swoole_socket.h"

#include <vector>
#include "swoole_rs_corebase_string_view.h"

struct swr_v1_core_resolver;

namespace swoole {

struct AsyncEvent {
    size_t task_id;
    uint8_t canceled;
    int error;
    // The request's resolver record. A stack event borrows its request's; each pool
    // copy holds its own reference, so a request may go before the pool finishes.
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
              "crates/hooks/src/offload.rs mirrors AsyncEvent");

namespace async {

typedef void (*Handler)(AsyncEvent *event);

AsyncEvent *dispatch(const AsyncEvent *request);

}  // namespace async
}  // namespace swoole
