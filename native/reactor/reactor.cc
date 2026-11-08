// SPDX-License-Identifier: Apache-2.0
// Compatibility derived from Swoole v6.2.4 src/wrapper/event.cc and src/reactor/base.cc
// (Swoole contributors).
// What stable Rust can't express of the event loop: the C++ Reactor object's lifetime
// (swoole_event_init/free, constructor, destructor) and the std::function callbacks,
// tasks and deferrals it holds. Rust decides each step (crates/reactor: initialize.rs,
// builtins.rs, callbacks.rs, deferred.rs, wait_loop.rs); this file runs its effects.
#include "swoole_reactor.h"
#include "swoole_rs_wrappers.h"
#include "swoole_client.h"
#include "swoole_coroutine_system.h"
#include "swoole_signal.h"
#include "swoole_api.h"

#include <cstddef>
#include <memory>
#ifdef HAVE_MALLOC_TRIM
#ifdef __APPLE__
#include <sys/malloc.h>
#else
#include <malloc.h>
#endif
#endif

using swoole::Callback;
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
        case SWR_WRAPPER_INIT_CLIENT: swoole::network::Client::init_reactor(reactor); break;
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

void swoole_event_defer(const Callback &cb, void *private_data) {
    SwooleTG.reactor->defer(cb, private_data);
}

void swr_v1_event_set_exit_condition(int id, bool (*condition)(size_t *event_num)) {
    SwooleTG.reactor->set_exit_condition(static_cast<Reactor::ExitCondition>(id),
                                         [condition](Reactor *, size_t &event_num) { return condition(&event_num); });
}

namespace swoole {

static void swr_reactor_run_task(const DeferCallback &task) {
    auto reply = swr_v1_reactor_builtin_step(2, 0, bool(task.callback), 0);
    switch (reply.action) {
    case 0: return;
    case 6: task.callback(task.data); return;
    default: std::abort();
    }
}
static void swr_reactor_try_exit(Reactor *reactor) {
    auto reply = swr_v1_reactor_builtin_step(3, 0, reactor->wait_exit, 0);
    if (reply.action == 7) {
        bool accepted = reactor->if_exit();
        reply = swr_v1_reactor_builtin_step(3, reply.phase, accepted ? 2 : 0, 0);
    }
    switch (reply.action) {
    case 0: return;
    case 8: reactor->running = false; return;
    default: std::abort();
    }
}
static bool swr_reactor_exit_predicate(uint32_t role, uint32_t flags, size_t event_num) {
    static_assert(sizeof(size_t) <= sizeof(uint64_t), "reactor count ABI");
    switch (swr_v1_reactor_builtin_step(role, 0, flags, event_num).action) {
    case 0: return false;
    case 9: return true;
    default: std::abort();
    }
}

template<class Function>
Function *ReactorCallbacks::ensure(uint32_t kind, int32_t key) {
    std::unique_ptr<Function> candidate;
    for (;;) {
        uintptr_t token = 0;
        switch (swr_v1_reactor_callbacks_ensure(&owner_, kind, key,
                    reinterpret_cast<uintptr_t>(candidate.get()), &token)) {
        case 0: candidate.reset(); break;
        case 1: return reinterpret_cast<Function *>(token);
        case 2: candidate.reset(new Function); break;
        case 3: return candidate.release();
        case -2: throw std::bad_alloc();
        default: std::abort();
        }
    }
}
void ReactorCallbacks::set_end(int32_t key, const End &function) {
    *ensure<End>(0, key) = function;
}
void ReactorCallbacks::set_exit(int32_t key, const Exit &function) {
    *ensure<Exit>(1, key) = function;
}
void ReactorCallbacks::erase_end(int32_t key) {
    uintptr_t token = 0;
    int32_t result = swr_v1_reactor_callbacks_erase(owner_, 0, key, &token);
    if (result == 1) delete reinterpret_cast<End *>(token);
    else if (result != 0) std::abort();
}
size_t ReactorCallbacks::erase_exit(int32_t key) {
    uintptr_t token = 0;
    int32_t result = swr_v1_reactor_callbacks_erase(owner_, 1, key, &token);
    if (result == 1) delete reinterpret_cast<Exit *>(token);
    else if (result != 0) std::abort();
    return result;
}
bool ReactorCallbacks::has_exit(int32_t key) const {
    uintptr_t token = 0;
    int32_t result = swr_v1_reactor_callbacks_lookup(owner_, 1, key, &token);
    if (result < 0) std::abort();
    return result == 1;
}
// Walks kind's entries in key order through Rust's view, re-read after each call:
// entries a callback adds after the current key run, ones it erases don't. Stops
// at the first call that returns false.
template<class Call>
static bool swr_reactor_callbacks_walk(swr_v1_reactor_callbacks *const &owner, uint32_t kind, Call &&call) {
    for (size_t i = 0; owner && i < owner->views[kind].len;) {
        const swr_v1_reactor_callback_entry entry = owner->views[kind].entries[i];
        if (!call(entry)) return false;
        const swr_v1_reactor_callback_view &view = owner->views[kind];
        i = i < view.len && view.entries[i].key == entry.key ? i + 1 : swr_v1_reactor_callbacks_after(owner, kind, entry.key);
    }
    return true;
}
void ReactorCallbacks::execute_end(Reactor *reactor) const {
    swr_reactor_callbacks_walk(owner_, 0, [&](const swr_v1_reactor_callback_entry &entry) {
        (*reinterpret_cast<End *>(entry.token))(reactor);
        return true;
    });
}
bool ReactorCallbacks::if_exit(Reactor *reactor, size_t &event_num) const {
    return swr_reactor_callbacks_walk(owner_, 1, [&](const swr_v1_reactor_callback_entry &entry) {
        return (*reinterpret_cast<Exit *>(entry.token))(reactor, event_num);
    });
}
void ReactorCallbacks::clear() noexcept {
    for (;;) {
        uintptr_t token = 0;
        switch (swr_v1_reactor_callbacks_clear_next(&owner_, &token)) {
        case 0: return;
        case 1: delete reinterpret_cast<End *>(token); break;
        case 2: delete reinterpret_cast<Exit *>(token); break;
        default: std::abort();
        }
    }
}
ReactorCallbacks::~ReactorCallbacks() noexcept {
    clear();
}
void ReactorCallbacks::copy_from(const ReactorCallbacks &source) {
    swr_reactor_callbacks_walk(source.owner_, 0, [&](const swr_v1_reactor_callback_entry &entry) {
        set_end(entry.key, *reinterpret_cast<const End *>(entry.token));
        return true;
    });
    swr_reactor_callbacks_walk(source.owner_, 1, [&](const swr_v1_reactor_callback_entry &entry) {
        set_exit(entry.key, *reinterpret_cast<const Exit *>(entry.token));
        return true;
    });
}
ReactorCallbacks::ReactorCallbacks(const ReactorCallbacks &source) {
    try { copy_from(source); }
    catch (...) { clear(); throw; }
}
ReactorCallbacks &ReactorCallbacks::operator=(const ReactorCallbacks &source) {
    if (this != &source) {
        ReactorCallbacks copied(source);
        std::swap(owner_, copied.owner_);
    }
    return *this;
}

Reactor::Reactor(int max_event, Type _type) {
    static_assert(TYPE_AUTO == 0 && TYPE_EPOLL == 1 && TYPE_KQUEUE == 2 && TYPE_POLL == 3, "backend type ABI");
    uint32_t caps = 0, trim_compiled = 0;
#ifdef HAVE_EPOLL
    caps |= 1;
#endif
#ifdef HAVE_KQUEUE
    caps |= 2;
#endif
#ifdef HAVE_MALLOC_TRIM
    trim_compiled = 1;
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

    swr_v1_reactor_init_state state{};
    uint32_t hook = 0, main_thread = 0;
    for (;;) {
        switch (swr_v1_reactor_init_next(&state, ready_, hook, main_thread, trim_compiled)) {
        case 0: return;
        case 1: break;  // the backend set ready_ when it opened
        case 2:
            timeout_msec = -1;
            idle_task = {};
            future_task = {};
            write = _write;
            writev = _writev;
            close = _close;
            set_default_write_handler(_writable_callback);
            break;
        case 3: hook = swoole_isset_hook(SW_GLOBAL_HOOK_ON_REACTOR_CREATE); break;
        case 4: swoole_call_hook(SW_GLOBAL_HOOK_ON_REACTOR_CREATE, this); break;
        case 5: set_end_callback(PRIORITY_DEFER_TASK, [](Reactor *reactor) { swr_v1_reactor_run_defer(&reactor->defer_tasks); }); break;
        case 6: set_exit_condition(EXIT_CONDITION_DEFER_TASK,
                       [](Reactor *reactor, size_t &event_num) -> bool { return swr_reactor_exit_predicate(4, reactor->defer_tasks != nullptr, 0); }); break;
        case 7: set_end_callback(PRIORITY_IDLE_TASK, [](Reactor *reactor) { swr_reactor_run_task(reactor->idle_task); }); break;
        case 8: main_thread = swoole_is_main_thread(); break;
        case 9: set_end_callback(PRIORITY_SIGNAL_CALLBACK, [](Reactor *) { swoole_signal_dispatch(); }); break;
        case 10: set_end_callback(PRIORITY_TRY_EXIT, [](Reactor *reactor) { swr_reactor_try_exit(reactor); }); break;
        case 11: main_thread = swoole_is_main_thread(); break;
#ifdef HAVE_MALLOC_TRIM
        case 12: set_end_callback(PRIORITY_MALLOC_TRIM, [](Reactor *reactor) {
            time_t now = ::time(nullptr);
            if (reactor->last_malloc_trim_time < now - SW_MALLOC_TRIM_INTERVAL) {
                malloc_trim(SW_MALLOC_TRIM_PAD);
                reactor->last_malloc_trim_time = now;
            }
        }); break;
#endif
        case 13: set_exit_condition(EXIT_CONDITION_DEFAULT, [](Reactor *, size_t &event_num) -> bool { return swr_reactor_exit_predicate(5, 0, event_num); }); break;
        default: std::abort();
        }
    }
}

static_assert(sizeof(Event) == sizeof(swr_v1_reactor_event) && offsetof(Event, fd) == offsetof(swr_v1_reactor_event, fd) &&
                  offsetof(Event, reactor_id) == offsetof(swr_v1_reactor_event, reactor_id) &&
                  offsetof(Event, type) == offsetof(swr_v1_reactor_event, type) &&
                  offsetof(Event, socket) == offsetof(swr_v1_reactor_event, socket) && sizeof(FdType) == sizeof(int),
              "the Rust wait loop fills swoole::Event");

// Rust runs ReactorEpoll/ReactorKqueue/ReactorPoll::wait over the state's backend, with
// its warnings (swr_v1_reactor_wait); these are the effects over C++ callbacks.
int Reactor::wait() {
    const swr_v1_reactor_wait_ops ops{
        this,
        [](void *r) { static_cast<Reactor *>(r)->execute_begin_callback(); },
        [](void *r, int timed_out) { static_cast<Reactor *>(r)->execute_end_callbacks(timed_out != 0); },
#ifdef SW_USE_IOURING
        [](void *r) { return static_cast<int>(errno == EINTR && static_cast<Reactor *>(r)->iouring_interrupt_handler); },
        [](void *r) {
            auto reactor = static_cast<Reactor *>(r);
            reactor->iouring_interrupt_handler(reactor);
        },
#else
        nullptr,
        nullptr,
#endif
    };
    return swr_v1_reactor_wait(handle(), &ops) < 0 ? SW_ERR : SW_OK;
}

bool Reactor::if_exit() {
    size_t event_num = get_event_num();
    return callbacks_.if_exit(this, event_num);
}

// A Callback enters a Rust deferred queue boxed with its data, freed once run or by the queue.
static void defer_into(swr_v1_reactor_deferred **queue, const Callback &cb, void *data) {
    using Task = std::pair<Callback, void *>;
    std::unique_ptr<Task> task(new Task(cb, data));
    auto run = [](void *t) {
        std::unique_ptr<Task> task(static_cast<Task *>(t));
        task->first(task->second);
    };
    if (swr_v1_reactor_deferred_push(queue, run, task.get(), [](void *t) { delete static_cast<Task *>(t); }) != 0) {
        throw std::bad_alloc();
    }
    task.release();
}

void Reactor::add_destroy_callback(const Callback &cb, void *data) {
    defer_into(&destroy_callbacks, cb, data);
}

void Reactor::set_end_callback(const EndCallback _id, const std::function<void(Reactor *)> &fn) {
    callbacks_.set_end(_id, fn);
}

void Reactor::erase_end_callback(const EndCallback _id) {
    callbacks_.erase_end(_id);
}

/**
 * Returns false, the reactor cannot be exited, the next condition is skipped
 * Returns true, the reactor can exit and will continue to execute the next conditional function
 */
void Reactor::set_exit_condition(const ExitCondition _id, const std::function<bool(Reactor *, size_t &)> &fn) {
    callbacks_.set_exit(_id, fn);
}

void Reactor::defer(const Callback &cb, void *data) {
    defer_into(&defer_tasks, cb, data);
}

void Reactor::execute_end_callbacks(bool _timed_out) {
    timed_out = _timed_out;
    callbacks_.execute_end(this);
}

void Reactor::execute_begin_callback() const {
    if (future_task.callback) {
        future_task.callback(future_task.data);
    }
}

Reactor::~Reactor() {
    destroyed = true;
    swr_v1_reactor_deferred_run(destroy_callbacks);
    swr_v1_reactor_state_close_backend(handle());
    if (swoole_isset_hook(SW_GLOBAL_HOOK_ON_REACTOR_DESTROY)) {
        swoole_call_hook(SW_GLOBAL_HOOK_ON_REACTOR_DESTROY, this);
    }
}
}  // namespace swoole
