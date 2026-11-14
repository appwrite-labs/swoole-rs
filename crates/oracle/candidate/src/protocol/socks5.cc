/*
 +----------------------------------------------------------------------+
 | Swoole                                                               |
 +----------------------------------------------------------------------+
 | Copyright (c) 2012-2017 The Swoole Group                             |
 +----------------------------------------------------------------------+
 | This source file is subject to version 2.0 of the Apache license,    |
 | that is bundled with this package in the file LICENSE, and is        |
 | available through the world-wide-web at the following url:           |
 | http://www.apache.org/licenses/LICENSE-2.0.html                      |
 | If you did not receive a copy of the Apache2.0 license and are unable|
 | to obtain it through the world-wide-web, please send a note to       |
 | license@swoole.com so we can mail you a copy immediately.            |
 +----------------------------------------------------------------------+
 | Author: Tianfeng Han  <rango@swoole.com>                             |
 +----------------------------------------------------------------------+
 */

#include "swoole_proxy.h"
#include "swoole_socket.h"
#include "swoole_rs_protocols.h"

namespace swoole {
const char *Socks5Proxy::strerror(int code) {
    return swr_v1_socks_strerror(code);
}

Socks5Proxy *Socks5Proxy::create(
    int socket_type, const std::string &host, int port, const std::string &user, const std::string &pwd) {
    const int config = swr_v1_socks_config(user.length(), pwd.length());
    if (config == 0) {
        swoole_error_log(SW_LOG_NOTICE,
                         SW_ERROR_SOCKS5_AUTH_FAILED,
                         "SOCKS5 username or password is too long, max length is 250 bytes");
        return nullptr;
    }
    auto socks5_proxy = new Socks5Proxy();
    socks5_proxy->host = host;
    socks5_proxy->port = port;
    socks5_proxy->dns_tunnel = 1;
    socks5_proxy->socket_type = socket_type;
    if (config == 2) {
        socks5_proxy->username = user;
        socks5_proxy->password = pwd;
    }
    return socks5_proxy;
}

ssize_t Socks5Proxy::pack_negotiate_request() {
    return swr_v1_socks_negotiate(!username.empty(), &method, reinterpret_cast<uint8_t *>(buf), sizeof(buf));
}

ssize_t Socks5Proxy::pack_auth_request() {
    return swr_v1_socks_auth(reinterpret_cast<const uint8_t *>(username.data()), username.size(),
                             reinterpret_cast<const uint8_t *>(password.data()), password.size(),
                             reinterpret_cast<uint8_t *>(buf), sizeof(buf));
}

bool Socks5Proxy::handshake(const char *data, size_t length,
                          const std::function<ssize_t(const char *, size_t)> &send_fn) {
    swr_socks_transition decision{};
    if (swr_v1_socks_transition(state, method, reinterpret_cast<const uint8_t *>(data), length, &decision) < 0) {
        return false;
    }
    switch (decision.error) {
    case 1:
        swoole_error_log(SW_LOG_NOTICE, SW_ERROR_SOCKS5_HANDSHAKE_FAILED, "SOCKS5 handshake failed, data length is too short");
        break;
    case 2:
        swoole_error_log(SW_LOG_NOTICE, SW_ERROR_SOCKS5_UNSUPPORT_VERSION, "SOCKS version is not supported");
        break;
    case 3:
        swoole_error_log(SW_LOG_NOTICE, SW_ERROR_SOCKS5_UNSUPPORT_METHOD, "SOCKS authentication method is not supported");
        break;
    case 4:
        swoole_error_log(SW_LOG_NOTICE, SW_ERROR_SOCKS5_AUTH_FAILED, "SOCKS username/password authentication failed");
        break;
    case 5:
        swoole_error_log(SW_LOG_NOTICE, SW_ERROR_SOCKS5_SERVER_ERROR,
                         "Socks5 server error, reason :%s", Socks5Proxy::strerror(decision.reply));
        break;
    }
    if (decision.error) return false;
    state = decision.state;
    if (decision.action == 1 || decision.action == 2) {
        const auto size = decision.action == 1 ? pack_auth_request() : pack_connect_request();
        return size >= 0 && send_fn(buf, size) == size;
    }
    return true;
}

ssize_t Socks5Proxy::pack_connect_request() {
    // Preserve the observable prefix even when native address validation fails.
    memcpy(buf, "\x05\x01", 3);
    const uint8_t *address;
    size_t address_length;
    uint8_t address_type;
    network::Address target_addr;
    if (dns_tunnel) {
        address_type = 3;
        address = reinterpret_cast<const uint8_t *>(target_host.data());
        address_length = target_host.size();
    } else {
        if (!target_addr.assign(static_cast<SocketType>(socket_type), target_host, target_port, false)) {
            swoole_error_log(SW_LOG_NOTICE, SW_ERROR_SOCKS5_HANDSHAKE_FAILED,
                             "When disable SOCKS5 proxy DNS tunnel connection, the destination host must be an IP address.");
            return SW_ERR;
        }
        if (network::Socket::is_inet4(static_cast<SocketType>(socket_type))) {
            address_type = 1;
            address = reinterpret_cast<const uint8_t *>(&target_addr.addr.inet_v4.sin_addr);
            address_length = sizeof(target_addr.addr.inet_v4.sin_addr);
        } else if (network::Socket::is_inet6(static_cast<SocketType>(socket_type))) {
            address_type = 4;
            address = reinterpret_cast<const uint8_t *>(&target_addr.addr.inet_v6.sin6_addr);
            address_length = sizeof(target_addr.addr.inet_v6.sin6_addr);
        } else {
            swoole_error_log(SW_LOG_NOTICE, SW_ERROR_SOCKS5_HANDSHAKE_FAILED, "Unsupported socket type for SOCKS5");
            return SW_ERR;
        }
    }
    const auto size = swr_v1_socks_connect(address_type, address, address_length, target_port,
                                           reinterpret_cast<uint8_t *>(buf), sizeof(buf));
    if (size == -2) {
        swoole_error_log(SW_LOG_NOTICE,
                         SW_ERROR_SOCKS5_HANDSHAKE_FAILED,
                         "SOCKS5 target host is too long, max length is 255 bytes");
        return SW_ERR;
    }
    return size;
}
}  // namespace swoole
