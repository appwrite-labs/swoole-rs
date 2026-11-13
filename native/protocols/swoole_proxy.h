/* SPDX-License-Identifier: Apache-2.0 */
/* Swoole's include/swoole_proxy.h, owned here: HttpProxy and Socks5Proxy as views of
 * the Rust records (crates/protocols-ffi/src/owned_proxy.rs). Upstream's file is deleted,
 * so `#include "swoole_proxy.h"` finds this copy. */

#pragma once
#define SWOOLE_RS_PROXY_OWNER 1

#include <string>
#include <cstdint>
#include <functional>
#include <memory>
#include "swoole_rs_corebase_proxy.h"
#include "swoole_rs_corebase_string_view.h"

enum swHttpProxyState {
    SW_HTTP_PROXY_STATE_WAIT = 0,
    SW_HTTP_PROXY_STATE_HANDSHAKE,
    SW_HTTP_PROXY_STATE_READY,
};

enum swSocks5State {
    SW_SOCKS5_STATE_WAIT = 0,
    SW_SOCKS5_STATE_HANDSHAKE,
    SW_SOCKS5_STATE_AUTH,
    SW_SOCKS5_STATE_CONNECT,
    SW_SOCKS5_STATE_READY,
};

static_assert(SW_SOCKS5_STATE_WAIT == 0 && SW_SOCKS5_STATE_HANDSHAKE == 1 && SW_SOCKS5_STATE_READY == 4 &&
                  SW_HTTP_PROXY_STATE_WAIT == 0 && SW_HTTP_PROXY_STATE_READY == 2,
              "the proxy states crates/clients/src/client.rs and crates/hooks/src/connect.rs copy");

namespace swoole {
static_assert(sizeof(swr_v1_core_proxy_fields) == 20 && offsetof(swr_v1_core_proxy_fields, port) == 8, "proxy scalar ABI");
class ProxyOwner {
  protected:
    swr_v1_core_proxy_prepared prepared_;
    explicit ProxyOwner(bool socks) : prepared_(swr_v1_core_proxy_new(socks)) {}
    ProxyOwner(const ProxyOwner &) = delete;
    ProxyOwner &operator=(const ProxyOwner &) = delete;
    ~ProxyOwner() { swr_v1_core_proxy_free(prepared_.owner); }
};

struct HttpProxy : private ProxyOwner {
    HttpProxy() : ProxyOwner(false) {}
    HttpProxy(const HttpProxy &source) : HttpProxy() { *this = source; }
    HttpProxy &operator=(const HttpProxy &source) {
        auto result = swr_v1_core_proxy_assign(handle(), source.handle());
        RustStringView *views[] = {&host, &username, &password, &target_host};
        if (result.status) views[result.index]->check(result.status);
        return *this;
    }
    swr_v1_core_proxy *handle() const { return prepared_.owner; }
    uint8_t &state = prepared_.fields->state;
    uint8_t &dont_handshake = prepared_.fields->dont_handshake;
    int &port = prepared_.fields->port;
    RustStringView host{prepared_.strings[0]};
    RustStringView username{prepared_.strings[1]};
    RustStringView password{prepared_.strings[2]};
    RustStringView target_host{prepared_.strings[3]};
    int &target_port = prepared_.fields->target_port;
};

struct Socks5Proxy : private ProxyOwner {
    Socks5Proxy() : ProxyOwner(true) {}
    Socks5Proxy(const Socks5Proxy &source) : Socks5Proxy() { *this = source; }
    Socks5Proxy &operator=(const Socks5Proxy &source) {
        auto result = swr_v1_core_proxy_assign(handle(), source.handle());
        RustStringView *views[] = {&host, &username, &password, &target_host};
        if (result.status) views[result.index]->check(result.status);
        return *this;
    }
    swr_v1_core_proxy *handle() const { return prepared_.owner; }
    RustStringView host{prepared_.strings[0]};
    int &port = prepared_.fields->port;
    uint8_t &state = prepared_.fields->state;
    uint8_t &version = prepared_.fields->version;
    uint8_t &method = prepared_.fields->method;
    uint8_t &dns_tunnel = prepared_.fields->dns_tunnel;
    RustStringView username{prepared_.strings[1]};
    RustStringView password{prepared_.strings[2]};
    RustStringView target_host{prepared_.strings[3]};
    int &target_port = prepared_.fields->target_port;
    int &socket_type = prepared_.fields->socket_type;
    char (&buf)[512] = *reinterpret_cast<char (*)[512]>(prepared_.buffer);

    // Rust under these symbols (crates/protocols-ffi/src/owned_proxy.rs).
    ssize_t pack_negotiate_request();
    ssize_t pack_auth_request();
    ssize_t pack_connect_request();
    // Applies a reply, logging a failure, and packs the request that answers it into
    // buf: its size, 0 when none is due, -1 on failure.
    ssize_t handshake_step(const char *rbuf, size_t rlen);
    bool handshake(const char *rbuf, size_t rlen, const std::function<ssize_t(const char *buf, size_t len)> &send_fn) {
        const ssize_t n = handshake_step(rbuf, rlen);
        return n == 0 || (n > 0 && send_fn(buf, n) == n);
    }

    static const char *strerror(int code);
    // 0, logged, when a credential is too long; 2 when both are set; else 1.
    static int admit(size_t user_length, size_t password_length);
    static Socks5Proxy *create(
        int socket_type, const std::string &host, int port, const std::string &user, const std::string &pwd) {
        const int config = admit(user.length(), pwd.length());
        if (config == 0) {
            return nullptr;
        }
        std::unique_ptr<Socks5Proxy> proxy(new Socks5Proxy());
        proxy->host = host;
        proxy->port = port;
        proxy->dns_tunnel = 1;
        proxy->socket_type = socket_type;
        if (config == 2) {
            proxy->username = user;
            proxy->password = pwd;
        }
        return proxy.release();
    }
};
}  // namespace swoole
