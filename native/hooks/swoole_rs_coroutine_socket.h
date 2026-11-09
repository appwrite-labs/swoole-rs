#pragma once
/* coroutine::Socket, the C++ face of Rust's coroutine socket (upstream's
 * include/swoole_coroutine_socket.h and swoole_socket_impl.h). It is Rust's record
 * (swoole_rs_co_socket.h) and nothing more: no vtable, no C++ members. Rust makes,
 * accepts and frees the sockets it hands out (crates/hooks/src/co_socket.rs); a C++
 * `new` or a local one is released by ~Socket. The out-of-line members are Rust under
 * their upstream symbols (crates/hooks/src/socket_members.rs). */
#include "swoole_socket.h"
#include "swoole_rs_hooks.h"

#include "swoole_coroutine.h"
#include "swoole_protocol.h"
#include "swoole_rs_co_socket.h"

#include <string>

namespace swoole {
namespace coroutine {
class Socket : public swr_v1_co_socket {
  public:
    // Rust makes the socket (crates/hooks/src/co_socket.rs).
    Socket(int domain, int type, int protocol) {
        created(swr_v1_hook_co_socket_new(this, this, SWR_CO_SOCKET_BY_FAMILY, -1, 0, domain, type, protocol));
    }
    Socket(int fd, int domain, int type, int protocol) {
        created(swr_v1_hook_co_socket_new(this, this, SWR_CO_SOCKET_FD_BY_FAMILY, fd, 0, domain, type, protocol));
    }
    explicit Socket(SocketType type = SW_SOCK_TCP) {
        created(swr_v1_hook_co_socket_new(this, this, SWR_CO_SOCKET_BY_TYPE, -1, type, 0, 0, 0));
    }
    Socket(int fd, SocketType type) {
        created(swr_v1_hook_co_socket_new(this, this, SWR_CO_SOCKET_FD_BY_TYPE, fd, type, 0, 0, 0));
    }
    /**
     * Warn:
     * the destructor should only be called in following two cases:
     * 1. construct failed
     * 2. called close() and it returns true
     * 3. called close() and it returns false, but it will not be accessed anywhere else
     */
    ~Socket() {
#ifdef SW_DEBUG
        if (SwooleG.running) {
            SW_ASSERT(!has_bound() && socket->removed);
        }
#endif
        swr_v1_hook_co_socket_release(this);
    }
    Socket(const Socket &) = delete;
    Socket &operator=(const Socket &) = delete;
    /**
     * If SSL is enabled, an SSL handshake will automatically take place during the connect() method.
     * When connect() returns true, it indicates that the TCP connection has been successfully
     * established and the SSL handshake has also succeeded.
     */
    // Rust's (crates/hooks/src/connect.rs), through the socket's proxies.
    bool connect(const std::string &host, int port = 0, int flags = 0) {
        return swr_v1_hook_co_socket_connect_host(this, host.data(), host.size(), port);
    }
    // The lifecycle is Rust's (crates/hooks/src/lifecycle.rs).
    bool connect(const sockaddr *addr, socklen_t addrlen) {
        return swr_v1_hook_co_socket_connect(this, addr, addrlen);
    }
    bool shutdown(int how = SHUT_RDWR) {
        return swr_v1_hook_co_socket_shutdown(this, how);
    }
    bool cancel(EventType event) {
        return swr_v1_hook_co_socket_cancel(this, event);
    }
    /**
     * If true is returned, the related resources of this socket can be released.
     * If false is returned, other coroutines are still bound to this socket: they
     * close it, and release it once nothing is bound.
     */
    bool close() {
        return swr_v1_hook_co_socket_close(this);
    }

    bool is_closed() const {
        return sock_fd == SW_BAD_SOCKET;
    }

    bool check_liveness() {
        return swr_v1_hook_co_socket_check_liveness(this);
    }
    ssize_t peek(void *_buf, size_t _n);
    ssize_t recv(void *_buf, size_t _n);
    ssize_t send(const void *_buf, size_t _n);

    ssize_t recv_once(void *_buf, size_t _n);
    ssize_t send_once(const void *_buf, size_t _n);

    ssize_t send(const std::string &buf) {
        return send(buf.c_str(), buf.length());
    }

    /**
     * The read()/write()/recvmsg()/sendmsg() functions currently does not support SSL
     */
    ssize_t read(void *_buf, size_t _n);
    ssize_t write(const void *_buf, size_t _n);
    ssize_t recvmsg(msghdr *msg, int flags);
    ssize_t sendmsg(const msghdr *msg, int flags);

    ssize_t readv(network::IOVector *io_vector);
    ssize_t readv_all(network::IOVector *io_vector);
    ssize_t writev(network::IOVector *io_vector);
    ssize_t writev_all(network::IOVector *io_vector);
    ssize_t recv_all(void *_buf, size_t _n);
    ssize_t send_all(const void *_buf, size_t _n);
    ssize_t recv_packet(double timeout = 0);
    ssize_t recv_line(void *_buf, size_t maxlen);

    bool poll(EventType _type, double timeout = 0);
    /**
     * If the server has SSL enabled, you must explicitly call `ssl_handshake()`,
     * as it will not be automatically executed within the `accept()` function.
     * This behavior is inconsistent with `connect()`, which internally executes `ssl_handshake()` automatically,
     * thus not requiring an explicit call at the application level.
     * The reason for this design is that `ssl_handshake()` can typically be performed concurrently within a separate
     * client coroutine. If `ssl_handshake()` were to be automatically executed inside the `accept()` function,
     * it would block the server's listening coroutine,
     * causing the `ssl_handshake()` processes to execute sequentially rather than in parallel.
     */
    Socket *accept(double timeout = 0);
    bool bind(const char *address, int port = 0);
    bool bind(const sockaddr *sa, socklen_t len);
    bool listen(int backlog = 0);
    bool sendfile(const char *filename, off_t offset, size_t length);
    ssize_t sendto(const char *host, int port, const void *_buf, size_t _n);
    ssize_t recvfrom(void *_buf, size_t _n);
    ssize_t recvfrom(void *_buf, size_t _n, sockaddr *_addr, socklen_t *_socklen);

    SocketType get_type() const {
        return type;
    }

    int get_fd() const {
        return sock_fd;
    }

    network::Socket *get_socket() const {
        return socket;
    }

    bool getsockname() const;

    const char *get_addr() const {
        return socket->get_addr();
    }

    int get_port() const {
        return socket->get_port();
    }

    bool has_bound(const EventType event = SW_EVENT_RDWR) const {
        return get_bound_co(event) != nullptr;
    }

    Coroutine *get_bound_co(const EventType event) const {
        return static_cast<Coroutine *>(swr_v1_hook_socket_bound_co(state_, event));
    }

    // The bound-coroutine checks, set_err(e), check_return_value and is_available are
    // Rust's (crates/hooks/src/co_io.rs).
    void check_bound_co(const EventType event) const {
        swr_v1_hook_co_socket_check_bound(this, event);
    }

    void set_err(const int e) {
        swr_v1_hook_co_socket_set_err(this, e);
    }

    void set_err() { swr_v1_hook_co_socket_default_err(this, swoole_get_last_error()); }
    void set_err(int error, const char *message) {
        swr_v1_hook_co_socket_borrowed_err(this, error, message);
    }
    void set_err(int error, const std::string &message) {
        swr_v1_hook_co_socket_owned_err(this, error, message.data(), message.size());
    }

    /* set connect read write timeout */
    void set_timeout(double timeout, int _type = SW_TIMEOUT_ALL) const;

    void set_timeout(const timeval *timeout, int _type = SW_TIMEOUT_ALL) const {
        set_timeout(swr_v1_hook_timeval((double) timeout->tv_sec, (double) timeout->tv_usec), _type);
    }

    double get_timeout(TimeoutType _type) const;
    bool get_option(int level, int optname, void *optval, socklen_t *optlen) const;
    bool get_option(int level, int optname, int *optval) const;
    bool set_option(int level, int optname, const void *optval, socklen_t optlen) const;
    bool set_option(int level, int optname, int optval) const;
    String *get_read_buffer();
    String *get_write_buffer();
    int move_fd() {
        sock_fd = SW_BAD_SOCKET;
        return socket->move_fd();
    }

  protected:
    void bind_co(uint32_t event, Coroutine *co) { swr_v1_hook_socket_bind(state_, event, co); }

    // Rust reports what the constructor did (crates/hooks/src/co_socket.rs).
    void created(uint32_t result) {
        swr_v1_hook_co_socket_created(this, result);
    }

    void check_return_value(ssize_t retval) {
        swr_v1_hook_co_socket_check(this, retval);
    }

    // Rust's wait (crates/hooks/src/socket_wait.rs) for the current coroutine.
    bool wait_event(EventType event,
                    swr_v1_hook_barrier barrier = nullptr,
                    void *barrier_state = nullptr,
                    const void **_buf = nullptr,
                    size_t _n = 0) {
        return swr_v1_hook_co_socket_wait(this, event, barrier, barrier_state, (char **) _buf, _n);
    }

    static const swr_v1_hook_co_socket_host_t host_;
    static const bool host_installed_;

    bool is_available(const EventType event) {
        return swr_v1_hook_co_socket_available(this, event);
    }
};

static_assert(sizeof(Socket) == sizeof(swr_v1_co_socket) && !std::is_polymorphic<Socket>::value,
              "a Socket is Rust's record (crates/hooks/src/co_socket.rs)");

class ProtocolSwitch {
    swr_v1_co_protocol_scope state_;

  public:
    explicit ProtocolSwitch(Socket *socket) : state_(swr_v1_hook_protocol_scope(socket)) {}
    ~ProtocolSwitch() { swr_v1_hook_protocol_restore(&state_); }
};

//-------------------------------------------------------------------------------
}  // namespace coroutine
}  // namespace swoole
#include <memory>

using CoSocket = swoole::coroutine::Socket;

#ifdef SW_USE_URING_SOCKET
namespace swoole {
namespace coroutine {
// A Socket whose members Rust runs through io_uring (crates/hooks/src/uring_socket.rs).
struct UringSocket : Socket {
    template <class... Args>
    explicit UringSocket(Args... args) : Socket(args...) {
        swr_v1_hook_co_socket_uring(this);
    }
};
}  // namespace coroutine
}  // namespace swoole
using swoole::coroutine::UringSocket;
using SocketImpl = UringSocket;
#else
using SocketImpl = CoSocket;
#endif

/* Lent until the fd's hooked close or unwrap. */
SocketImpl *swoole_coroutine_get_socket_object(int sockfd);
SocketImpl *swoole_coroutine_get_socket_object_ex(int sockfd);
