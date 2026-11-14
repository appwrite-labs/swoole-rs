// Drives the pinned Socks5Proxy through a handshake, or one member at a time, behind C
// entry points: build.rs compiles socks5.cc with `swoole` renamed, over the facade in
// shim/socks5.
#include <cstdint>
#include <cstring>
#include <string>
#include "swoole_proxy.h"
#include "swoole_socket.h"

// One fuzz case: the proxy's settings and the server's replies, each `reply_sizes`
// entry the length of the next reply in `replies`. `send_short` makes every send
// write one byte less than asked.
extern "C" struct SwrOracleSocks5 {
    const uint8_t *user;
    size_t user_length;
    const uint8_t *password;
    size_t password_length;
    const uint8_t *target_host;
    size_t target_host_length;
    uint16_t target_port;
    uint8_t dns_tunnel;
    uint8_t socket_type;
    uint8_t send_short;
    const uint8_t *replies;
    const uint16_t *reply_sizes;
    size_t reply_count;
};

// Logs create's result (N on failure), the negotiate request, then per reply the
// bytes sent (S<len>:bytes), the result, state and method (H<ok>,<state>,<method>;)
// and the reply code's strerror, until a reply fails or the proxy is ready.
// Returns the log's full length; at most capacity bytes are written to out.
extern "C" size_t swr_oracle_socks5_upstream(const SwrOracleSocks5 *c, char *out, size_t capacity) {
    using swoole::Socks5Proxy;
    std::string log;
    auto *proxy = Socks5Proxy::create(c->socket_type,
                                      "proxy",
                                      1080,
                                      std::string(reinterpret_cast<const char *>(c->user), c->user_length),
                                      std::string(reinterpret_cast<const char *>(c->password), c->password_length));
    if (!proxy) {
        log += "N";
    } else {
        proxy->target_host.assign(reinterpret_cast<const char *>(c->target_host), c->target_host_length);
        proxy->target_port = c->target_port;
        proxy->dns_tunnel = c->dns_tunnel;
        const auto send = [&](const char *data, size_t length) -> ssize_t {
            log += "S" + std::to_string(length) + ":";
            log.append(data, length);
            return c->send_short ? static_cast<ssize_t>(length) - 1 : static_cast<ssize_t>(length);
        };
        const ssize_t length = proxy->pack_negotiate_request();
        send(proxy->buf, length);
        proxy->state = SW_SOCKS5_STATE_HANDSHAKE;
        const auto *reply = reinterpret_cast<const char *>(c->replies);
        for (size_t i = 0; i < c->reply_count && proxy->state != SW_SOCKS5_STATE_READY; i++) {
            const bool ok = proxy->handshake(reply, c->reply_sizes[i], send);
            log += "H" + std::to_string(ok) + "," + std::to_string(proxy->state) + "," +
                   std::to_string(proxy->method) + ";";
            if (c->reply_sizes[i] >= 2) {
                log += Socks5Proxy::strerror(static_cast<uint8_t>(reply[1]));
            }
            reply += c->reply_sizes[i];
            if (!ok) {
                break;
            }
        }
        delete proxy;
    }
    size_t n = log.size() < capacity ? log.size() : capacity;
    memcpy(out, log.data(), n);
    return log.size();
}

// One Socks5Proxy's fields, copied in before and out after each call below, so a test
// can set any state upstream's members allow and compare every field afterwards.
extern "C" struct SwrOracleSocks5Proxy {
    char host[512];
    size_t host_length;
    char username[512];
    size_t username_length;
    char password[512];
    size_t password_length;
    char target_host[512];
    size_t target_host_length;
    int port;
    int target_port;
    int socket_type;
    uint8_t state;
    uint8_t version;
    uint8_t method;
    uint8_t dns_tunnel;
    char buf[512];
};

static void load(const SwrOracleSocks5Proxy *c, swoole::Socks5Proxy *proxy) {
    proxy->host.assign(c->host, c->host_length);
    proxy->username.assign(c->username, c->username_length);
    proxy->password.assign(c->password, c->password_length);
    proxy->target_host.assign(c->target_host, c->target_host_length);
    proxy->port = c->port;
    proxy->target_port = c->target_port;
    proxy->socket_type = c->socket_type;
    proxy->state = c->state;
    proxy->version = c->version;
    proxy->method = c->method;
    proxy->dns_tunnel = c->dns_tunnel;
    memcpy(proxy->buf, c->buf, sizeof(proxy->buf));
}

static void store_string(const std::string &value, char *out, size_t *length) {
    *length = value.size() < 512 ? value.size() : 512;
    memcpy(out, value.data(), *length);
}

static void store(const swoole::Socks5Proxy *proxy, SwrOracleSocks5Proxy *c) {
    store_string(proxy->host, c->host, &c->host_length);
    store_string(proxy->username, c->username, &c->username_length);
    store_string(proxy->password, c->password, &c->password_length);
    store_string(proxy->target_host, c->target_host, &c->target_host_length);
    c->port = proxy->port;
    c->target_port = proxy->target_port;
    c->socket_type = proxy->socket_type;
    c->state = proxy->state;
    c->version = proxy->version;
    c->method = proxy->method;
    c->dns_tunnel = proxy->dns_tunnel;
    memcpy(c->buf, proxy->buf, sizeof(c->buf));
}

// pack_negotiate_request (0), pack_auth_request (1) or pack_connect_request (2).
extern "C" ssize_t swr_oracle_socks5_pack(SwrOracleSocks5Proxy *c, int request) {
    swoole::Socks5Proxy proxy{};
    load(c, &proxy);
    const ssize_t n = request == 0   ? proxy.pack_negotiate_request()
                      : request == 1 ? proxy.pack_auth_request()
                                     : proxy.pack_connect_request();
    store(&proxy, c);
    return n;
}

// handshake over `reply`, with a send that copies what it is given to `wire` (its
// length to `*wire_length`) and returns its length when `sent` is 1, else `sent`.
extern "C" bool swr_oracle_socks5_handshake(SwrOracleSocks5Proxy *c,
                                            const char *reply,
                                            size_t reply_length,
                                            ssize_t sent,
                                            char *wire,
                                            size_t *wire_length) {
    swoole::Socks5Proxy proxy{};
    load(c, &proxy);
    const bool ok = proxy.handshake(reply, reply_length, [&](const char *data, size_t length) -> ssize_t {
        *wire_length = length < 512 ? length : 512;
        memcpy(wire, data, *wire_length);
        return sent == 1 ? static_cast<ssize_t>(length) : sent;
    });
    store(&proxy, c);
    return ok;
}

extern "C" const char *swr_oracle_socks5_strerror(int code) {
    return swoole::Socks5Proxy::strerror(code);
}

// create's proxy, its fields in `*c`; false when create refuses.
extern "C" bool swr_oracle_socks5_create(int socket_type,
                                         const char *host,
                                         size_t host_length,
                                         int port,
                                         const char *user,
                                         size_t user_length,
                                         const char *password,
                                         size_t password_length,
                                         SwrOracleSocks5Proxy *c) {
    auto *proxy = swoole::Socks5Proxy::create(socket_type,
                                              std::string(host, host_length),
                                              port,
                                              std::string(user, user_length),
                                              std::string(password, password_length));
    if (!proxy) {
        return false;
    }
    store(proxy, c);
    delete proxy;
    return true;
}
