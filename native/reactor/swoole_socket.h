/* SPDX-License-Identifier: Apache-2.0 */
/* Swoole's include/swoole_socket.h, owned here and cut to what the build's C uses:
 * network::Address and the Socket methods over Rust's socket record (swoole_rs_socket.h).
 * Rust defines the rest of upstream's Socket methods under their C++ symbols. Upstream's
 * file is deleted, so `#include "swoole_socket.h"` finds this copy on the Rust prefix's
 * include path. */
#pragma once

#include "swoole.h"
#include "swoole_ssl.h"
#include "swoole_buffer.h"
#include "swoole_string.h"

#include "swoole_rs_reactor.h"
#include "swoole_rs_clients.h"

#include <poll.h>
#include <sys/types.h>
#include <sys/socket.h>
#include <sys/un.h>
#include <sys/uio.h>

#include <netinet/in.h>
#include <netinet/ip6.h>
#include <netinet/tcp.h>
#include <netinet/udp.h>
#include <arpa/inet.h>

#include <stdexcept>
#include <string>
#include <vector>

enum {
    SW_BAD_SOCKET = -1,
};

struct swr_v1_core_resolver;

namespace swoole {

namespace network {
struct Address {
    union {
        sockaddr ss;
        sockaddr_in inet_v4;
        sockaddr_in6 inet_v6;
        sockaddr_un un;
    } addr;
    socklen_t len;
    SocketType type;

    static bool verify_ip(int family, const std::string &str) { return swr_v1_clients_address_verify(family, str.c_str()); }
};

}  // namespace network
}  // namespace swoole

#include "swoole_rs_socket.h"

namespace swoole {
namespace network {
// The fields are Rust's record (swoole_rs_socket.h); Rust allocates and frees it.
struct Socket : swr_v1_socket {
    // SocketRecord::new: zeroed, with the default timeouts. Records Rust frees come from Rust.
    Socket() : swr_v1_socket{} {
        *timeouts() = swr_v1_socket_default_timeouts();
    }

    uint32_t get_out_buffer_length() const {
        return out_buffer ? out_buffer->length() : 0;
    }

    int move_fd() {
        int sock_fd = fd;
        fd = SW_BAD_SOCKET;
        return sock_fd;
    }

    int wait_event(int timeout_ms, int _events) const;
    void free();
};

}  // namespace network

network::Socket *make_socket(int fd, FdType fd_type);
}  // namespace swoole
