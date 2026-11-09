// SPDX-License-Identifier: Apache-2.0
// Compatibility derived from Swoole v6.2.4 src/wrapper/event.cc and src/reactor/base.cc
// (Swoole contributors).
// The C++ Reactor object's lifetime: swoole_event_init/free, constructor and destructor.
// Its callbacks, deferrals and wait loop are Rust's (crates/reactor: callbacks.rs,
// deferred.rs, wait_loop.rs).
#include "swoole_reactor.h"
#include "swoole_rs_wrappers.h"
#include "swoole_rs_clients.h"
#include "swoole_coroutine_system.h"
#include "swoole_signal.h"
#include "swoole_api.h"

#include <cstddef>

using swoole::Reactor;

int swoole_event_init(int flags) {
    auto *reactor = new Reactor(SW_REACTOR_MAXEVENTS);
    auto plan = swr_v1_wrappers_event_init(flags, SW_EVENTLOOP_WAIT_EXIT, reactor->ready());
    if (!plan.accepted) {
        delete reactor;
        return SW_ERR;
    }
    if (plan.wait_exit) { reactor->wait_exit = true; }
    for (uint32_t initializer : plan.initializers) {
        switch (initializer) {
        // System::init_reactor registers the coroutine socket's handlers too.
        case SWR_WRAPPER_INIT_SOCKET: break;
        case SWR_WRAPPER_INIT_SYSTEM: swoole::coroutine::System::init_reactor(reactor); break;
        case SWR_WRAPPER_INIT_CLIENT: swr_v1_clients_init_reactor(reactor); break;
        default: assert(false); return SW_ERR;
        }
    }
    SwooleTG.reactor = reactor;
    return SW_OK;
}

int swoole_event_free() {
    if (!SwooleTG.reactor) {
        return SW_ERR;
    }
    delete SwooleTG.reactor;
    SwooleTG.reactor = nullptr;
    return SW_OK;
}

namespace swoole {

Reactor::Reactor(int max_event, Type _type) {
    static_assert(TYPE_AUTO == 0 && TYPE_EPOLL == 1 && TYPE_KQUEUE == 2 && TYPE_POLL == 3, "backend type ABI");
    uint32_t caps = 0;
#ifdef HAVE_EPOLL
    caps |= 1;
#endif
#ifdef HAVE_KQUEUE
    caps |= 2;
#endif
    auto backend = swr_v1_reactor_backend_type(_type, caps, 0, 0);
#ifdef HAVE_KQUEUE
    if (backend.observe_kqueue) {
        backend = swr_v1_reactor_backend_type(_type, caps, 1, swr_v1_core_enable_kqueue());
    }
#endif
    type_ = static_cast<Type>(backend.selected);
    switch (swr_v1_reactor_state_open(handle(), backend.factory, max_event)) {
    case 1:
        swoole_log_as(backend.factory == TYPE_EPOLL ? "ReactorEpoll::ReactorEpoll" : "ReactorKqueue::ReactorKqueue",
                      swoole_sys_warning("%s() failed", backend.factory == TYPE_EPOLL ? "epoll_create" : "kqueue"));
        break;
    case 2: throw std::bad_alloc();
    case 3: throw std::bad_array_new_length();
    }

    if (!ready_) {
        return;
    }
    timeout_msec = -1;
    write = _write;
    writev = _writev;
    close = _close;
    set_default_write_handler(_writable_callback);
    if (swoole_isset_hook(SW_GLOBAL_HOOK_ON_REACTOR_CREATE)) {
        swoole_call_hook(SW_GLOBAL_HOOK_ON_REACTOR_CREATE, this);
    }
    swr_v1_reactor_install_builtins(handle());
}

static_assert(sizeof(Event) == sizeof(swr_v1_reactor_event) && offsetof(Event, fd) == offsetof(swr_v1_reactor_event, fd) &&
                  offsetof(Event, reactor_id) == offsetof(swr_v1_reactor_event, reactor_id) &&
                  offsetof(Event, type) == offsetof(swr_v1_reactor_event, type) &&
                  offsetof(Event, socket) == offsetof(swr_v1_reactor_event, socket) && sizeof(FdType) == sizeof(int),
              "the Rust wait loop fills swoole::Event");

Reactor::~Reactor() {
    destroyed = true;
    swr_v1_reactor_deferred_run(destroy_callbacks);
    swr_v1_reactor_state_close_backend(handle());
    if (swoole_isset_hook(SW_GLOBAL_HOOK_ON_REACTOR_DESTROY)) {
        swoole_call_hook(SW_GLOBAL_HOOK_ON_REACTOR_DESTROY, this);
    }
}
}  // namespace swoole
