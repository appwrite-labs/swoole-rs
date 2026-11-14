#pragma once
/* coroutine::Socket, the C++ face of Rust's coroutine socket (upstream's
 * include/swoole_coroutine_socket.h and swoole_socket_impl.h). It is Rust's record
 * (swoole_rs_co_socket.h) and nothing more: no vtable, no C++ members. Rust makes,
 * accepts and frees the sockets it hands out (crates/hooks/src/co_socket.rs); a C++
 * `new` or a local one is released by ~Socket. Only ext/ssh2 uses it from C++: the
 * other members are Rust's `Co` methods. */
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
    // Only after a failed constructor, or once nothing is bound to the socket.
    ~Socket() {
        swr_v1_hook_co_socket_release(this);
    }
    Socket(const Socket &) = delete;
    Socket &operator=(const Socket &) = delete;
    // Rust's (crates/hooks/src/connect.rs), through the socket's proxies; with SSL
    // enabled, true means the handshake succeeded too.
    bool connect(const std::string &host, int port = 0, int flags = 0) {
        return swr_v1_hook_co_socket_connect_host(this, host.data(), host.size(), port);
    }

    int get_fd() const {
        return sock_fd;
    }

  protected:
    // Rust reports what the constructor did (crates/hooks/src/co_socket.rs).
    void created(uint32_t result) {
        swr_v1_hook_co_socket_created(this, result);
    }
};

static_assert(sizeof(Socket) == sizeof(swr_v1_co_socket) && !std::is_polymorphic<Socket>::value,
              "a Socket is Rust's record (crates/hooks/src/co_socket.rs)");

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
