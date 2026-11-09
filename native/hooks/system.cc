// SPDX-License-Identifier: Apache-2.0
// Compatibility derived from Swoole v6.2.4 src/coroutine/system.cc (Swoole contributors).
// What crates/hooks can't express of System and coroutine::Socket (upstream's
// src/coroutine/system.cc is deleted): the host tables whose entries call C++ members,
// and ~Socket, the vtable's key function. System::sleep, read_file, write_file and
// coroutine::async are Rust (crates/hooks/src/sleep.rs, file_io.rs, offload.rs).
#include "swoole_coroutine_system.h"
#include "swoole_rs_hooks.h"
#include "swoole_rs_phpruntime.h"
#include "swoole_util.h"

#include "swoole_signal.h"
#include "swoole_rs_uring.h"
#include "swoole_rs_coroutine_socket.h"

namespace swoole {
namespace coroutine {

static void *co_current() {
    return Coroutine::get_current_safe();
}

static void co_resume(void *co) {
    static_cast<Coroutine *>(co)->resume();
}

static bool signal_busy() {
    return SwooleG.signal_listener_num > 0;
}

// Records the handler when the registration runs, so a deferred one sees those pending.
static void signal_register(int signo, swr_v1_hook_previous_signal *previous) {
    if (!previous->registered) {
        previous->handler = reinterpret_cast<uintptr_t>(swoole_signal_get_handler(signo));
        previous->registered = true;
        swoole_signal_set(signo, swr_v1_hook_signal_wake);
    }
}

#ifdef SW_USE_THREAD_CONTEXT
// A deferred signal change: restore `handler`, or register `previous`.
struct SignalChange {
    int signo;
    SignalHandler handler;
    swr_v1_hook_previous_signal *previous;
};

static void signal_change(void *data) {
    std::unique_ptr<SignalChange> change(static_cast<SignalChange *>(data));
    if (change->previous) {
        signal_register(change->signo, change->previous);
    } else {
        swoole_signal_set(change->signo, change->handler);
    }
}

static void signal_change_drop(void *data) {
    delete static_cast<SignalChange *>(data);
}
#endif

static void signal_listen(int signo, swr_v1_hook_previous_signal *previous, bool on) {
    auto handler = reinterpret_cast<SignalHandler>(previous->handler);
#ifdef SW_USE_THREAD_CONTEXT
    auto *change = new SignalChange{signo, handler, on ? previous : nullptr};
    if (swr_v1_reactor_deferred_push(&sw_reactor()->defer_tasks, signal_change, change, signal_change_drop) != 0) {
        delete change;
        throw std::bad_alloc();
    }
#else
    if (on) {
        signal_register(signo, previous);
    } else {
        swoole_signal_set(signo, handler);
    }
#endif
}

static bool signal_listeners_done(size_t *) {
    return SwooleG.signal_async_listener_num == 0;
}

static void signal_waiting(bool on) {
    if (!on) {
        SwooleG.signal_async_listener_num--;
        return;
    }
    if (!sw_reactor()->isset_exit_condition(Reactor::EXIT_CONDITION_CO_SIGNAL_LISTENER)) {
        swr_v1_event_set_exit_condition(Reactor::EXIT_CONDITION_CO_SIGNAL_LISTENER, signal_listeners_done);
    }
    SwooleG.signal_async_listener_num++;
}

static bool co_park_for(void *co, double timeout) {
    return static_cast<Coroutine *>(co)->yield_ex(timeout);
}

// Swoole\Coroutine\System (crates/phpcoroutines) calls these: each result C++ holds
// is copied out through `put`, one call per string.
typedef void (*swr_v1_system_put)(void *out, const char *data, size_t length);

extern "C" int swr_v1_system_wait_signal(const long *signals, size_t count, double timeout) {
    static const swr_v1_hook_signal_ops ops = {
        co_current, signal_busy, signal_listen, signal_waiting, co_park_for, co_resume};
    std::vector<int32_t> list(signals, signals + count);
    const auto signalled = swr_v1_hook_wait_signal(list.data(), list.size(), timeout, &ops);
    if (signalled.last_error != 0) {
        swoole_set_last_error(signalled.last_error);
    }
    return signalled.signo;
}

// coroutine::Socket's native side (src/coroutine/socket.cc is gone): the effects its
// Rust operations call back into, the C++-typed members (std::string, std::function,
// the proxies, File and String objects) and the destructor, which is the
// vtable's key function.
static_assert(SW_EVENT_READ == 1 << 9 && SW_EVENT_WRITE == 1 << 10, "swr_v1_hook_socket_* event bits");
// Rust defines the reactor callbacks and the members it runs whole under their
// upstream symbols (crates/hooks/src/socket_members.rs); it finds the record one pointer
// into a Socket.
static_assert(std::is_polymorphic<Socket>::value && !std::is_polymorphic<swr_v1_co_socket>::value &&
                  alignof(swr_v1_co_socket) <= sizeof(void *),
              "coroutine::Socket's layout for crates/hooks/src/socket_members.rs");

// What the Rust operations need from C++ (crates/hooks/src/co_io.rs), installed once.
const swr_v1_hook_co_socket_host_t Socket::host_{
    swoole_set_last_error,
    swoole_strerror,
    [] { return static_cast<void *>(Coroutine::get_current_safe()); },
    [](void *co) { return static_cast<Coroutine *>(co)->get_cid(); },
    Coroutine::print_socket_bound_error,
    [](swr_v1_co_socket *s, double timeout) -> void * {
        return swoole_timer_add(timeout, false, [](Timer *, TimerNode *tnode) {
            swr_v1_hook_co_socket_timer_fired(static_cast<Socket *>(tnode->data), tnode);
        }, static_cast<Socket *>(s));
    },
    [](void *node) { swoole_timer_del(static_cast<TimerNode *>(node)); },
    [] { System::sleep(0.01); },
    [](swr_v1_co_socket *s, uint32_t events, bool add) -> bool {
        auto *sock = s->socket;
        return (add ? swoole_event_add(sock, events) : swoole_event_set(sock, events)) == SW_OK;
    },
    [](swr_v1_co_socket *s, void *co, uint32_t event) {
        auto *sock = static_cast<Socket *>(s);
        Coroutine::CancelFunc cancel_fn = [sock, event](Coroutine *) {
            return sock->cancel(static_cast<EventType>(event));
        };
        static_cast<Coroutine *>(co)->yield(&cancel_fn);
    },
    [](swr_v1_co_socket *s, char **buf, size_t n) {
        auto *sock = static_cast<Socket *>(s);
        if (!sock->zero_copy && n > 0 && *buf != sock->get_write_buffer()->str) {
            sock->write_buffer->clear();
            sock->write_buffer->append(*buf, n);
            *buf = sock->write_buffer->str;
        }
    },
    [](swr_v1_co_socket *s, uint32_t event) {
        if (event == SW_EVENT_READ) {
            SwooleTG.reactor->remove_read_event(s->socket);
        } else {
            SwooleTG.reactor->remove_write_event(s->socket);
        }
    },
    [](void *co) { static_cast<Coroutine *>(co)->resume(); },
    [](swr_v1_co_socket *s) { s->socket->ssl_shutdown(); },
    [](swr_v1_co_socket *s) { return s->socket->check_liveness(); },
    [](swr_v1_co_socket *s, swr_v1_hook_read_buffer *view, uint32_t action, size_t value) {
        auto *sock = static_cast<Socket *>(s);
        String *buffer = action == 0 ? sock->get_read_buffer() : sock->read_buffer;
        if (action != 0) {
            buffer->length = view->length;
            buffer->offset = view->offset;
            if (action == 2) { buffer->extend(value); }
            if (action == 3) { buffer->reduce(value); }
        }
        *view = {buffer->length, buffer->size, static_cast<int64_t>(buffer->offset), buffer->str};
    },
    [](swr_v1_co_socket *s, char *at, size_t n, bool all) {
        auto *sock = static_cast<Socket *>(s);
        return all ? sock->recv_all(at, n) : sock->recv(at, n);
    },
    [](swr_v1_co_socket *s, uint32_t size, uint32_t *header_len) {
        PacketLength pl;
        pl.header_len = 0;
        pl.buf = s->read_buffer->str;
        pl.buf_size = size;
        ssize_t length = s->protocol.get_package_length(&s->protocol, s->socket, &pl);
        swoole_trace_log(SW_TRACE_SOCKET, "packet_len=%ld, length=%u", length, size);
        *header_len = pl.header_len;
        return length;
    },
    [](swr_v1_co_socket *s, uint32_t kind, ssize_t length) {
        auto *sock = static_cast<Socket *>(s);
        if (kind == 0) {
            sock->set_err(SW_ERROR_PACKAGE_LENGTH_NOT_FOUND, "get package length failed");
        } else if (kind == 1) {
            swoole_log_as("Socket::recv_packet_with_length_protocol",
                          swoole_error_log(SW_LOG_WARNING, SW_ERROR_PACKAGE_LENGTH_TOO_LARGE,
                                           "packet length is too big, remote_addr=%s:%d, length=%zu",
                                           s->socket->get_addr(), s->socket->get_port(), length));
            sock->set_err(SW_ERROR_PACKAGE_LENGTH_TOO_LARGE, sw_error);
        } else {
            sock->set_err(SW_ERROR_PACKAGE_LENGTH_TOO_LARGE, "no package eof, package_max_length exceeded");
        }
    },
    [](swr_v1_co_socket *server, swr_v1_socket *conn) -> void * {
        return new Socket(static_cast<network::Socket *>(conn), static_cast<Socket *>(server));
    },
    [](void *s) { delete static_cast<Socket *>(s); },
};
const bool Socket::host_installed_ = (swr_v1_hook_co_socket_host(&Socket::host_), true);

static_assert(SW_FD_CO_SOCKET == 6 && SW_SOCK_RAW == 7 && SW_BUFFER_SIZE_BIG == 65536 &&
                  SW_INPUT_BUFFER_SIZE == 2 * 1024 * 1024,
              "swr_v1_hook_co_socket constants");

// Rust makes the socket and sets its options (crates/hooks/src/co_socket.rs); these
// are the error and the logs.
void Socket::created(uint32_t result) {
    extra_ = extra_owner_.value.owner;
    if (!(result & SWR_CO_SOCKET_OPENED)) {
        swoole_set_last_error(errno);
        return;
    }
    if ((result >> 8) & 1) {
        swoole_sys_warning("fcntl(%d, GETFL) failed", sock_fd);
    }
    if ((result >> 8) & 2) {
        swoole_sys_warning("fcntl(%d, SETFL, opts) failed", sock_fd);
    }
    if (result & SWR_CO_SOCKET_NODELAY_FAILED) {
        swoole_sys_warning(
            "setsockopt(%d, %d, %d, %u) failed", sock_fd, IPPROTO_TCP, TCP_NODELAY, (unsigned) sizeof(int));
    }
}

// The transfers, peek and poll are Rust under their own symbols (socket_members.rs).
// Rust receives into the network::Socket's peer address (crates/hooks/src/transfer.rs).
ssize_t Socket::recvfrom(void *_buf, size_t _n) {
    return swr_v1_hook_co_socket_recvfrom(this, _buf, _n, [](swr_v1_co_socket *s, void *buf, size_t n, sockaddr *addr, socklen_t *len) {
        return static_cast<Socket *>(s)->recvfrom(buf, n, addr, len);
    });
}

/**
 * Warn:
 * the destructor should only be called in following two cases:
 * 1. construct failed
 * 2. called close() and it returns true
 * 3. called close() and it returns false, but it will not be accessed anywhere else
 */
Socket::~Socket() {
#ifdef SW_DEBUG
    if (SwooleG.running) {
        SW_ASSERT(!has_bound() && socket->removed);
    }
#endif
    // The destructor callback, the buffers and the network::Socket are Rust's to release
    // (crates/hooks/src/co_socket.rs).
    swr_v1_hook_co_socket_release(this);
}
}  // namespace coroutine
}  // namespace swoole

// hook.cc's socket functions are Rust (crates/hooks/src/hooked.rs), as are the hooked
// file, sleep and resolver functions (stdio.rs); this is what they need of native.

#define SOCK(socket) static_cast<SocketImpl *>(socket)
static const swr_v1_hook_host hook_host = {
    [] { return SwooleTG.reactor != nullptr && swoole::Coroutine::get_current() != nullptr; },
    swr_v1_hook_coroutine_async_job,
    swoole::coroutine::System::sleep,
#ifdef SW_USE_IOURING
    &swr_v1_uring_native,
#else
    nullptr,
#endif
#ifdef HAVE_IOURING_STATX
    true,
#else
    false,
#endif
#ifdef HAVE_IOURING_FTRUNCATE
    true,
#else
    false,
#endif
#ifdef HAVE_IOURING_FUTEX
    true,
#else
    false,
#endif
    swoole_set_last_error,
    {
        [](int domain, int type, int protocol) -> void * { return new SocketImpl(domain, type, protocol); },
        [](int fd) -> void * { return new SocketImpl(fd, SW_SOCK_RAW); },
        [](void *socket) { delete SOCK(socket); },
    },
};

// What the Rust coroutine streams (crates/phpruntime/src/socket.rs) need of SocketImpl.
using RuntimeCall = swr_v1_runtime_socket_call;
// Declared extern "C" in swoole_rs_phpruntime.h, so the definition has C linkage.
const swr_v1_runtime_socket_ops swr_v1_runtime_socket = {
    [](void *arg) {
        auto *call = static_cast<RuntimeCall *>(arg);
        if (!call->family) {
            call->socket = call->fd < 0 ? new SocketImpl((swSocketType) call->type)
                                        : new SocketImpl(call->fd, (swSocketType) call->type);
        } else {
            call->socket = call->fd < 0 ? new SocketImpl(call->domain, call->type, call->protocol)
                                        : new SocketImpl(call->fd, call->domain, call->type, call->protocol);
        }
    },
    [](void *arg) {
        auto *call = static_cast<RuntimeCall *>(arg);
        call->result = SOCK(call->socket)->close();
    },
    [](void *socket) { delete SOCK(socket); },
    [](void *socket, int level, int name, int value, bool logged) {
        if (logged) {
            SOCK(socket)->set_option(level, name, value);
        } else {
            SOCK(socket)->get_socket()->set_option(level, name, value);
        }
    },
    [](void *socket, const char *host, size_t length, int port, int flags) {
        return SOCK(socket)->connect(std::string(host, length), port, flags);
    },
    [](void *socket, void (*each)(void *, const char *, size_t), void *arg) {
        for (auto &cert : SOCK(socket)->get_socket()->ssl_get_peer_cert_chain(INT_MAX)) {
            each(arg, cert.c_str(), cert.length());
        }
    },
    [](void *socket, int op, const iovec *vectors, int count, int *index, size_t *offset) {
        swoole::network::IOVector io_vector(vectors, count);
        auto *sock = SOCK(socket);
        ssize_t retval = op == 0 ? sock->readv(&io_vector)
                         : op == 1 ? sock->readv_all(&io_vector)
                         : op == 2 ? sock->writev(&io_vector)
                                   : sock->writev_all(&io_vector);
        *index = io_vector.get_index();
        *offset = io_vector.get_offset_bytes();
        return retval;
    },
    [](void *socket, bool http, const char *host, size_t host_length, int port, const char *user,
       size_t user_length, const char *password, size_t password_length) {
        std::string h(host, host_length), u(user, user_length), p(password, password_length);
        if (http) {
            SOCK(socket)->set_http_proxy(h, port, u, p);
        } else {
            SOCK(socket)->set_socks5_proxy(h, port, u, p);
        }
    },
    [](void *socket) -> swr_v1_core_proxy * {
        auto &proxy = SOCK(socket)->http_proxy;
        return proxy ? proxy->handle() : nullptr;
    },
};
#undef SOCK
[[maybe_unused]] static const bool hook_host_set = (swr_v1_hook_set_host(&hook_host), true);
