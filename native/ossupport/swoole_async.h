/* SPDX-License-Identifier: Apache-2.0 */
/* Swoole's include/swoole_async.h, owned here: what C++ and core-tests still use of it.
 * AsyncEvent is a plain record whose layout crates/hooks/src/offload.rs mirrors (asserted
 * below); the DNS requests are views of the Rust resolver record. The
 * pool (AsyncThreads, async::dispatch) is crates/hooks/src/async_threads.rs. Upstream's
 * file is deleted, so `#include "swoole_async.h"` finds this copy on the Rust prefix's
 * include path. */
#pragma once

#include "swoole_socket.h"

#include <vector>
#include "swoole_rs_corebase_resolver_view.h"
#include "swoole_rs_corebase_string_view.h"

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

struct GethostbynameRequest : private ResolverOwner {
    RustStringView name{state_.name};
    int &family = state_.fields->family;
    RustStringView addr{state_.address};
    GethostbynameRequest(std::string name, int family) : ResolverOwner(family, 0, 0, name, "") {}
    GethostbynameRequest(const GethostbynameRequest &source)
        : GethostbynameRequest(std::string(source.name.data(), source.name.size()), source.family) { addr = source.addr; }
    GethostbynameRequest &operator=(const GethostbynameRequest &source) {
        if (this != &source) { name = source.name; family = source.family; addr = source.addr; }
        return *this;
    }
    swr_v1_core_resolver *state_handle() const { return state_.owner; }
};

struct GetaddrinfoRequest : private ResolverOwner {
    RustStringView hostname{state_.name};
    RustStringView service{state_.service};
    int &family = state_.fields->family;
    int &socktype = state_.fields->socktype;
    int &protocol = state_.fields->protocol;
    int &error = state_.fields->error;
    ResolverAddresses results{state_.owner};
    int &count = state_.fields->count;
    GetaddrinfoRequest(std::string hostname, int family, int socktype, int protocol, std::string service)
        : ResolverOwner(family, socktype, protocol, hostname, service) {}
    GetaddrinfoRequest(const GetaddrinfoRequest &source)
        : GetaddrinfoRequest(std::string(source.hostname.data(), source.hostname.size()), source.family,
                             source.socktype, source.protocol, std::string(source.service.data(), source.service.size())) {
        error = source.error; results = source.results; count = source.count;
    }
    GetaddrinfoRequest &operator=(const GetaddrinfoRequest &source) {
        if (this != &source) {
            hostname = source.hostname; service = source.service; family = source.family;
            socktype = source.socktype; protocol = source.protocol; error = source.error;
            results = source.results; count = source.count;
        }
        return *this;
    }
    swr_v1_core_resolver *state_handle() const { return state_.owner; }
};

namespace async {

typedef void (*Handler)(AsyncEvent *event);

AsyncEvent *dispatch(const AsyncEvent *request);

// network::gethostbyname over the record alone: a pool request has no address cache.
inline void handler_gethostbyname(AsyncEvent *event) {
    event->retval = swr_v1_core_resolver_host(event->data);
    if (event->retval < -1) ResolverOwner::check(event->retval);
    event->error = event->retval < 0 ? SW_ERROR_DNSLOOKUP_RESOLVE_FAILED : 0;
}

}  // namespace async
}  // namespace swoole
