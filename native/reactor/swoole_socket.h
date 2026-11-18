/* SPDX-License-Identifier: Apache-2.0 */
/* Swoole's include/swoole_socket.h, owned here and cut to what the build's C uses:
 * the SSH2 hook reads the descriptor prefix and checks IPv6 host strings.
 * Rust defines the rest of upstream's Socket methods under their C++ symbols. Upstream's
 * file is deleted, so `#include "swoole_socket.h"` finds this copy on the Rust prefix's
 * include path. */
#pragma once

#include "swoole.h"
#include "swoole_ssl.h"
#include "swoole_buffer.h"
#include "swoole_string.h"

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

namespace swoole {
namespace network {

struct Address {
    static bool verify_ip(int family, const std::string &str) { return swr_v1_clients_address_verify(family, str.c_str()); }
};

// Rust owns the record. SSH2's coroutine socket reads only its descriptor prefix.
struct Socket {
    Socket() = delete;
    int fd;
};
static_assert(offsetof(Socket, fd) == 0, "SocketRecord starts with the descriptor");

}  // namespace network
}  // namespace swoole
