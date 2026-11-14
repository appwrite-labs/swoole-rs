// Drives Socks5Proxy through a handshake behind a C entry point, once per engine:
// build.rs compiles the pinned socks5.cc (upstream) or it with the protocols
// overlay patch applied (candidate, which calls the Rust exports), each with
// `swoole` renamed to the engine, over the facade in shim/socks5.
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

#define SWR_CAT2(a, b) a##_##b
#define SWR_CAT(a, b) SWR_CAT2(a, b)

// Logs create's result (N on failure), the negotiate request, then per reply the
// bytes sent (S<len>:bytes), the result, state and method (H<ok>,<state>,<method>;)
// and the reply code's strerror, until a reply fails or the proxy is ready.
// Returns the log's full length; at most capacity bytes are written to out.
extern "C" size_t SWR_CAT(swr_oracle_socks5, SWR_SOCKS5_ENGINE)(const SwrOracleSocks5 *c, char *out, size_t capacity) {
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
