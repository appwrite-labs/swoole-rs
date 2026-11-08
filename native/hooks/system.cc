// SPDX-License-Identifier: Apache-2.0
// Compatibility derived from Swoole v6.2.4 src/coroutine/system.cc (Swoole contributors).
// What crates/hooks can't express of System and coroutine::Socket (upstream's
// src/coroutine/system.cc is deleted): the std::string, std::shared_ptr and
// std::function signatures, the host tables whose entries call C++ members, and
// ~Socket, the vtable's key function. System::sleep is Rust (crates/hooks/src/sleep.rs).
#include "swoole_coroutine_system.h"
#include "swoole_rs_hooks.h"
#include "swoole_rs_phpruntime.h"
#include "swoole_util.h"

#include "swoole_signal.h"
#include "swoole_rs_uring.h"
#include "swoole_rs_coroutine_socket.h"

namespace swoole {
namespace coroutine {

bool async_job(void (*job)(void *), void *context);

static void file_warning(int error, const char *origin, const char *message) {
    errno = error;
    swoole_log_as(origin, swoole_sys_warning("%s", message));
}

static char *file_buffer(void *out, size_t length, size_t size) {
    auto &content = *static_cast<std::shared_ptr<String> *>(out);
    if (!content) {
        content = std::make_shared<String>(size);
    } else if (content->size < size) {
        content->reserve(size);
    }
    content->length = length;
    return content->str;
}

static const swr_v1_hook_file_ops file_ops = {file_warning, file_buffer};

ssize_t System::write_file(const char *file, const char *buf, size_t length, bool lock, int flags) {
    return swr_v1_hook_write_file(file, buf, length, lock, flags | O_CREAT | O_WRONLY, &file_ops, async_job);
}

std::string gethostbyname_impl_with_async(const std::string &hostname, int domain, double timeout) {
    AsyncEvent ev{};
    auto req = new GethostbynameRequest(hostname, domain);
    ev.data = std::shared_ptr<AsyncRequest>(req);
    ev.retval = 1;

    coroutine::async(async::handler_gethostbyname, ev, timeout);

    const auto completion = swr_v1_core_resolver_finish(ev.retval, ev.error, nullptr, SW_ERROR_AIO_TIMEOUT, SW_ERROR_DNSLOOKUP_RESOLVE_TIMEOUT);
    if (completion.failed) {
        swoole_set_last_error(completion.error);
        return "";
    }
    return std::string(req->addr.data(), req->addr.size());
}

static void dns_assign(void *out, const char *address, size_t length) {
    static_cast<std::string *>(out)->assign(address, length);
}

std::string System::gethostbyname(const std::string &hostname, int domain, double timeout) {
    std::string result;
    if (swr_v1_hook_dns_cache_get(domain, hostname.data(), hostname.size(), &result, dns_assign)) {
        return result;
    }

#ifdef SW_USE_CARES
    auto result_list = dns_lookup_impl_with_cares(hostname.c_str(), domain, timeout);
    if (!result_list.empty()) {
        if (swr_v1_core_dns_lookup_random()) {
            result = result_list[swoole_rand() % result_list.size()];
        } else {
            result = result_list[0];
        }
    }
#else
    result = gethostbyname_impl_with_async(hostname, domain, timeout);
#endif

    swr_v1_hook_dns_cache_set(domain, hostname.data(), hostname.size(), result.data(), result.size());
    return result;
}

static void *co_current() {
    return Coroutine::get_current_safe();
}

static void co_park(void *co) {
    static_cast<Coroutine *>(co)->yield();
}

static void co_resume(void *co) {
    static_cast<Coroutine *>(co)->resume();
}

static bool signal_busy() {
    return SwooleG.signal_listener_num > 0;
}

static void signal_listen(int signo, bool on) {
    swoole::SignalHandler handler = on ? swr_v1_hook_signal_wake : nullptr;
#ifdef SW_USE_THREAD_CONTEXT
    swoole_event_defer([signo, handler](void *) { swoole_signal_set(signo, handler); }, nullptr);
#else
    swoole_signal_set(signo, handler);
#endif
}

static void signal_waiting(bool on) {
    if (!on) {
        SwooleG.signal_async_listener_num--;
        return;
    }
    if (!sw_reactor()->isset_exit_condition(Reactor::EXIT_CONDITION_CO_SIGNAL_LISTENER)) {
        sw_reactor()->set_exit_condition(
            Reactor::EXIT_CONDITION_CO_SIGNAL_LISTENER,
            [](Reactor *reactor, size_t &event_num) -> bool { return SwooleG.signal_async_listener_num == 0; });
    }
    SwooleG.signal_async_listener_num++;
}

static bool co_park_for(void *co, double timeout) {
    return static_cast<Coroutine *>(co)->yield_ex(timeout);
}

static void *poll_add(int fd, int events, void *task) {
    auto *socket = make_socket(fd, SW_FD_CO_POLL);
    socket->object = task;
    if (swoole_event_add(socket, events) < 0) {
        // socket_poll() is not the owner of the fd: release the socket, not the fd.
        socket->move_fd();
        socket->free();
        return nullptr;
    }
    return socket;
}

static void poll_remove(void *socket) {
    auto *_socket = static_cast<network::Socket *>(socket);
    swoole_event_del(_socket);
    _socket->move_fd();
    _socket->free();
}

static void poll_timer_expired(Timer *timer, TimerNode *tnode) {
    swr_v1_hook_poll_timeout(tnode->data);
}

static void *poll_timer_add(double timeout, void *task) {
    return swoole_timer_add(timeout, false, poll_timer_expired, task);
}

static void poll_timer_del(void *timer) {
    swoole_timer_del(static_cast<TimerNode *>(timer));
}

static void poll_defer(void *task) {
    sw_reactor()->defer(swr_v1_hook_poll_completed, task);
}

extern const swr_v1_hook_poll_ops poll_ops = {
    poll_add, poll_remove, poll_timer_add, poll_timer_del, poll_defer, co_current, co_park, co_resume};

// Swoole\Coroutine\System (crates/phpcoroutines) calls these: each result C++ holds
// is copied out through `put`, one call per string.
typedef void (*swr_v1_system_put)(void *out, const char *data, size_t length);

extern "C" bool swr_v1_system_read_file(const char *file, bool lock, swr_v1_system_put put, void *out) {
    std::shared_ptr<String> content;
    swr_v1_hook_read_file(file, lock, &content, &file_ops, async_job);
    if (content) {
        put(out, content->str, content->length);
    }
    return content != nullptr;
}

extern "C" bool swr_v1_system_gethostbyname(
    const char *host, size_t length, long family, double timeout, swr_v1_system_put put, void *out) {
    auto address = System::gethostbyname(std::string(host, length), family, timeout);
    if (!address.empty()) {
        put(out, address.data(), address.size());
    }
    return !address.empty();
}

extern "C" bool swr_v1_system_getaddrinfo(const char *host,
                                          long family,
                                          long socktype,
                                          long protocol,
                                          const char *service,
                                          double timeout,
                                          swr_v1_system_put put,
                                          void *out) {
    AsyncEvent ev{};
    auto req = new GetaddrinfoRequest(host, family, socktype, protocol, service);
    ev.data = std::shared_ptr<AsyncRequest>(req);
    coroutine::async(async::handler_getaddrinfo, ev, timeout);
    std::vector<std::string> addresses;
    const auto completion = swr_v1_core_resolver_finish(ev.retval, ev.error, &req->error, SW_ERROR_AIO_TIMEOUT, SW_ERROR_DNSLOOKUP_RESOLVE_TIMEOUT);
    if (completion.failed) {
        swoole_set_last_error(completion.error);
    } else {
        req->parse_result(addresses);
    }
    for (auto &address : addresses) {
        put(out, address.data(), address.size());
    }
    return !addresses.empty();
}

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

static_assert(sizeof(AsyncEvent) == 80 && offsetof(AsyncEvent, error) == 12 && offsetof(AsyncEvent, retval) == 32 &&
                  offsetof(AsyncEvent, object) == 56 && offsetof(AsyncEvent, callback) == 72,
              "crates/hooks/src/offload.rs mirrors AsyncEvent");

static void *async_dispatch(const void *event) {
    return async::dispatch(static_cast<const AsyncEvent *>(event));
}

static const swr_v1_hook_async_ops async_ops = {
    co_current, async_dispatch, co_park, co_park_for, co_resume, swoole_get_last_error};

bool async(async::Handler handler, AsyncEvent &event, double timeout) {
    return swr_v1_hook_async(&event, reinterpret_cast<void (*)(void *)>(handler), timeout, &async_ops);
}

bool async_job(void (*job)(void *), void *context) {
    return swr_v1_hook_async_job(job, context, &async_ops);
}

bool async(const std::function<void()> &fn) {
    return async_job([](void *fn) { (*static_cast<const std::function<void()> *>(fn))(); },
                     const_cast<std::function<void()> *>(&fn));
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
    [](void *context, int type, double timeout, const char *host, size_t length, void *out,
       void (*put)(void *, const char *, size_t)) {
        auto *ctx = static_cast<NameResolver::Context *>(context);
        ctx->type = type;
        ctx->timeout = timeout;
        std::string address = swoole_name_resolver_lookup(std::string(host, length), ctx);
        put(out, address.data(), address.size());
        return ctx->with_port;
    },
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
    swoole::coroutine::async_job,
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
    &swoole::coroutine::poll_ops,
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
    [](const int *fds, const int *events, size_t count, double timeout, int *ready, int *revents, size_t *size) {
        // Merged by fd, in the map's order, as upstream's socket_poll map was.
        std::unordered_map<int, int> polled;
        for (size_t i = 0; i < count; i++) {
            polled[fds[i]] |= events[i];
        }
        std::vector<swr_v1_hook_poll_fd> list;
        for (auto &entry : polled) {
            list.push_back({entry.first, entry.second, 0});
        }
        if (!swr_v1_hook_socket_poll(list.data(), list.size(), timeout, &swoole::coroutine::poll_ops)) {
            *size = 0;
            return false;
        }
        for (size_t i = 0; i < list.size(); i++) {
            ready[i] = list[i].fd;
            revents[i] = list[i].revents;
        }
        *size = list.size();
        return true;
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
