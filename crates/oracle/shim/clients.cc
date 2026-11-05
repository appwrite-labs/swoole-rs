// The pinned Address::assign(url), Set-Cookie split and DNS name decode behind C
// entry points: build.rs copies each body unchanged into an .inc.
#include <cstdint>
#include <cstring>
#include <regex>
#include <stdexcept>
#include <string>

namespace upstream_clients {
// As upstream swoole_socket.h (values are this shim's own), logging off.
enum SocketType {
    SW_SOCK_TCP = 1,
    SW_SOCK_UDP = 2,
    SW_SOCK_TCP6 = 3,
    SW_SOCK_UDP6 = 4,
    SW_SOCK_UNIX_STREAM = 5,
    SW_SOCK_UNIX_DGRAM = 6,
};
#define swoole_error_log(level, error, str, ...)
// Records what the URL assigns instead of resolving it.
struct Address {
    SocketType type{};
    std::string host;
    int port = 0;
    bool assign(SocketType _type, const std::string &_host, int _port) {
        type = _type;
        host = _host;
        port = _port;
        return true;
    }
    bool assign(const std::string &url);
};
#include "clients_url.inc"
#undef swoole_error_log
#include "clients_dns.inc"
}  // namespace upstream_clients

extern "C" {
// assign(url) over length bytes: 1 assigned, 0 rejected, -1 std::stoi threw.
// The host goes to host (length bytes) and its length to *host_length.
int swr_oracle_clients_url(const char *data, size_t length, int *type, int *port, char *host, size_t *host_length) {
    upstream_clients::Address address;
    bool assigned;
    try {
        assigned = address.assign(std::string(data, length));
    } catch (const std::exception &) {
        return -1;
    }
    *type = address.type;
    *port = address.port;
    *host_length = address.host.size();
    memcpy(host, address.host.data(), address.host.size());
    return assigned;
}

// php_swoole_http_parse_set_cookies's split of length bytes into key, value offset and value.
void swr_oracle_clients_cookie(const char *at, size_t length, size_t *key, size_t *offset, size_t *value) {
    struct zval { int unused; };
#include "clients_cookie.inc"
    (void) zvalue;
    *key = key_len;
    *offset = p - at;
    *value = value_len;
}

// domain_decode in place over a NUL-terminated name.
void swr_oracle_clients_domain_decode(char *str) {
    upstream_clients::domain_decode(str);
}
}
