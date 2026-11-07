/* SPDX-License-Identifier: Apache-2.0 */
/* Swoole's include/swoole_reactor.h, owned here: the Reactor class over Rust's reactor
 * state (swoole_rs_reactor.h). Upstream's file is deleted, so `#include "swoole_reactor.h"`
 * finds this copy on the Rust prefix's include path. */
#pragma once

#include "swoole_socket.h"
#include "swoole_rs_reactor.h"
static_assert(SW_EVENT_READ == (1 << 9) && SW_EVENT_WRITE == (1 << 10) && SW_EVENT_DEAULT == 256 && SW_EVENT_ERROR == (1 << 11), "Reactor event ABI");

#include <list>
#include <map>
#include <unordered_map>

namespace swoole {

struct DeferCallback {
    Callback callback;
    void *data;
};

class Reactor;

class ReactorStateOwner {
  protected:
    swr_v1_reactor_prepared state_ = swr_v1_reactor_state_new();
    ReactorStateOwner() { if (!state_.owner) throw std::bad_alloc(); }
    ReactorStateOwner(const ReactorStateOwner &) = delete;
    ReactorStateOwner &operator=(const ReactorStateOwner &) = delete;
    ~ReactorStateOwner() { swr_v1_reactor_state_free(state_.owner); }
};

class Reactor : private ReactorStateOwner {
  public:
    enum Type {
        TYPE_AUTO,
        TYPE_EPOLL,
        TYPE_KQUEUE,
        TYPE_POLL,
    };

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

    enum ExitCondition {
        EXIT_CONDITION_TIMER = 0,
        EXIT_CONDITION_DEFER_TASK,
        EXIT_CONDITION_WAIT_PID,
        EXIT_CONDITION_CO_SIGNAL_LISTENER,
        EXIT_CONDITION_SIGNAL_LISTENER,
        EXIT_CONDITION_AIO_TASK,
        EXIT_CONDITION_SIGNALFD,
        EXIT_CONDITION_USER_BEFORE_DEFAULT,
        EXIT_CONDITION_FORCED_TERMINATION,
        EXIT_CONDITION_IOURING,
        EXIT_CONDITION_DEFAULT = 999,
        EXIT_CONDITION_USER_AFTER_DEFAULT,
    };

    int &type_ = state_.fields->kind;
    void *&ptr = state_.fields->ptr;
    int &native_handle = state_.fields->native_handle;
    uint32_t &max_event_num = state_.fields->max_event_num;
    bool &ready_ = state_.fields->ready;
    bool &running = state_.fields->running;
    bool &start = state_.fields->start;
    bool &once = state_.fields->once;
    bool &wait_exit = state_.fields->wait_exit;
    bool &destroyed = state_.fields->destroyed;
    bool &bailout = state_.fields->bailout;
    bool &timed_out = state_.fields->timed_out;
    int32_t &timeout_msec = state_.fields->timeout_msec;
    uint16_t &id = state_.fields->id;
    uint32_t &max_socket = state_.fields->max_socket;
#ifdef HAVE_MALLOC_TRIM
    time_t &last_malloc_trim_time = state_.fields->last_malloc_trim_time;
#endif
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

    int wait();

    swr_v1_reactor_deferred *&defer_tasks = state_.fields->defer_tasks;
    swr_v1_reactor_deferred *&destroy_callbacks = state_.fields->destroy_callbacks;

    DeferCallback idle_task;
    DeferCallback future_task;

#ifdef SW_USE_IOURING
    std::function<void(Reactor *)> iouring_interrupt_handler;
#endif

    using Write = ssize_t (*)(Reactor *, network::Socket *, const void *, size_t);
    using Writev = ssize_t (*)(Reactor *, network::Socket *, const iovec *, size_t);
    using Close = int (*)(Reactor *, network::Socket *);
    Write &write = state_.fields->write;
    Writev &writev = state_.fields->writev;
    Close &close = state_.fields->close;

  private:
    ReactorCallbacks callbacks_;
    static_assert(SW_MAX_FDTYPE == 32, "the Rust handler tables hold 32 fd types");

  public:
    // Read-only views of the Rust defaults, set through set_default_*_handler.
    const ReactorHandler &default_write_handler =
        *reinterpret_cast<const ReactorHandler *>(swr_v1_reactor_handlers_default(state_.handlers, 1));
    const ReactorHandler &default_error_handler =
        *reinterpret_cast<const ReactorHandler *>(swr_v1_reactor_handlers_default(state_.handlers, 2));

    explicit Reactor(int max_event = SW_REACTOR_MAXEVENTS, Type _type = TYPE_AUTO);
    ~Reactor();
    bool if_exit();
    void defer(const Callback &cb, void *data = nullptr);
    void set_end_callback(EndCallback _id, const std::function<void(Reactor *)> &fn);
    void erase_end_callback(EndCallback _id);
    void set_exit_condition(ExitCondition _id, const std::function<bool(Reactor *, size_t &)> &fn);
    void set_handler(int fd_type, int event, ReactorHandler handler);
    bool isset_handler(int fd_type, int event) const;
    void add_destroy_callback(const Callback &cb, void *data = nullptr);
    void execute_begin_callback() const;
    void execute_end_callbacks(bool _timed_out = false);
    void drain_write_buffer(network::Socket *socket);

    bool ready() const {
        return ready_;
    }

    bool is_running() const {
        return running;
    }

    size_t remove_exit_condition(const ExitCondition _id) {
        return callbacks_.erase_exit(_id);
    }

    bool isset_exit_condition(const ExitCondition _id) {
        return callbacks_.has_exit(_id);
    }

    // The interest helpers' decisions are Rust's too (swr_v1_reactor_interest).
    int interest(network::Socket *socket, uint32_t kind, int event = 0) const;

    int add_event(network::Socket *_socket, EventType event_type) const {
        return interest(_socket, 0, event_type);
    }

    int del_event(network::Socket *_socket, EventType event_type) const {
        return interest(_socket, 1, event_type);
    }

    int remove_read_event(network::Socket *_socket) const {
        return interest(_socket, 4);
    }

    int remove_write_event(network::Socket *_socket) const {
        return interest(_socket, 5);
    }

    int add_read_event(network::Socket *_socket) const {
        return interest(_socket, 2);
    }

    int add_write_event(network::Socket *_socket) const {
        return interest(_socket, 3);
    }

    bool exists(const network::Socket *_socket) const {
        return !_socket->removed && _socket->events;
    }

    bool exists(const int fd) const {
        return get_socket(fd) != nullptr;
    }

    int get_timeout_msec() const {
        return defer_tasks == nullptr ? timeout_msec : 0;
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

    void foreach_socket(const std::function<void(int, network::Socket *)> &callback) const {
        auto visit = [](void *context, int fd, void *socket) {
            (*static_cast<const std::function<void(int, network::Socket *)> *>(context))(
                fd, static_cast<network::Socket *>(socket));
        };
        if (swr_v1_reactor_registry_foreach(state_.registry, const_cast<void *>(static_cast<const void *>(&callback)), visit) < 0) {
            throw std::bad_alloc();
        }
    }

    ReactorHandler get_handler(const FdType fd_type, const EventType event) const {
        if (event != SW_EVENT_READ && event != SW_EVENT_WRITE && event != SW_EVENT_ERROR) {
            abort();
        }
        uint32_t kind = event == SW_EVENT_READ ? 0 : event == SW_EVENT_WRITE ? 1 : 2;
        return reinterpret_cast<ReactorHandler>(swr_v1_reactor_handlers_get(state_.handlers, kind, fd_type, true));
    }

    void set_default_write_handler(ReactorHandler handler) {
        swr_v1_reactor_handlers_set_default(state_.handlers, 1, reinterpret_cast<swr_v1_reactor_handler>(handler));
    }

    void set_default_error_handler(ReactorHandler handler) {
        swr_v1_reactor_handlers_set_default(state_.handlers, 2, reinterpret_cast<swr_v1_reactor_handler>(handler));
    }

    int trigger_close_event(Event *event) {
        return default_error_handler(this, event);
    }

    void set_wait_exit(const bool enable) {
        wait_exit = enable;
    }

    static ssize_t _write(Reactor *reactor, network::Socket *socket, const void *buf, size_t n);
    static ssize_t _writev(Reactor *reactor, network::Socket *socket, const iovec *iov, size_t iovcnt);
    static int _close(Reactor *reactor, network::Socket *socket);
    static int _writable_callback(Reactor *reactor, Event *ev);

    static bool isset_write_event(const int events) {
        return events & SW_EVENT_WRITE;
    }
};

inline int16_t translate_events_to_poll(int events) {
    return swr_v1_reactor_poll_events_to(events);
}
inline int translate_events_from_poll(int16_t events) {
    return swr_v1_reactor_poll_events_from(events);
}
}  // namespace swoole
