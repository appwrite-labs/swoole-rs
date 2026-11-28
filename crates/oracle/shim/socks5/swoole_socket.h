// What socks5.cc uses from swoole.h, swoole_error.h, swoole_log.h (logging off)
// and swoole_socket.h: the error codes, socket types and an Address whose assign
// is upstream's inet4/inet6 branch without name resolution.
#pragma once
#include <arpa/inet.h>
#include <cstring>
#include <netinet/in.h>
#include <string>
#include <sys/types.h>

#define SW_ERR -1
#define SW_LOG_NOTICE 3
#define swoole_error_log(...) ((void) 0)
typedef unsigned char uchar;

namespace swoole {
enum {
    SW_ERROR_SOCKS5_UNSUPPORT_VERSION = 7001,
    SW_ERROR_SOCKS5_UNSUPPORT_METHOD,
    SW_ERROR_SOCKS5_AUTH_FAILED,
    SW_ERROR_SOCKS5_SERVER_ERROR,
    SW_ERROR_SOCKS5_HANDSHAKE_FAILED,
};
enum SocketType { SW_SOCK_TCP = 1, SW_SOCK_UDP = 2, SW_SOCK_TCP6 = 3, SW_SOCK_UDP6 = 4, SW_SOCK_UNIX_STREAM = 5 };

namespace network {
struct Socket {
    static bool is_inet4(SocketType type) {
        return type == SW_SOCK_TCP || type == SW_SOCK_UDP;
    }
    static bool is_inet6(SocketType type) {
        return type == SW_SOCK_TCP6 || type == SW_SOCK_UDP6;
    }
};
struct Address {
    union {
        sockaddr_in inet_v4;
        sockaddr_in6 inet_v6;
    } addr;
    bool assign(SocketType type, const std::string &host, int port, bool) {
        if (port < 0 || port > 65535) {
            return false;
        }
        memset(&addr, 0, sizeof(addr));
        if (Socket::is_inet4(type)) {
            return inet_pton(AF_INET, host.c_str(), &addr.inet_v4.sin_addr.s_addr) == 1;
        }
        if (Socket::is_inet6(type)) {
            return inet_pton(AF_INET6, host.c_str(), addr.inet_v6.sin6_addr.s6_addr) == 1;
        }
        return false;
    }
};
}  // namespace network
}  // namespace swoole
