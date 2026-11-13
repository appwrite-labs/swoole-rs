// The pinned Address methods, hosts lookup, Set-Cookie split, DNS name codec,
// address text and socket DNS lookup behind C entry points: build.rs copies each body unchanged into an .inc.
#include <arpa/inet.h>
#include <sys/un.h>
#include <algorithm>
#include <array>
#include <cassert>
#include <cstdarg>
#include <cstdio>
#include <cstdlib>
#include <cerrno>
#include <cstdint>
#include <cstring>
#include <fstream>
#include <locale>
#include <regex>
#include <sstream>
#include <stdexcept>
#include <string>
#include <unordered_map>
#include <vector>

namespace upstream_clients {
// As upstream swoole_socket.h (values are this shim's own).
enum SocketType {
    SW_SOCK_TCP = 1,
    SW_SOCK_UDP = 2,
    SW_SOCK_TCP6 = 3,
    SW_SOCK_UDP6 = 4,
    SW_SOCK_UNIX_STREAM = 5,
    SW_SOCK_UNIX_DGRAM = 6,
};
// As upstream swoole_error.h.
#define SW_ERROR_NAME_TOO_LONG 504
#define SW_ERROR_DNSLOOKUP_RESOLVE_FAILED 711
#define SW_ERROR_BAD_HOST_ADDR 722
#define SW_ERROR_BAD_PORT 723
#define SW_ERROR_BAD_SOCKET_TYPE 724
#define SW_PATH_HOSTS "/nonexistent-oracle-hosts"

// Effect doubles: the error log and last error are counted, and gethostbyname
// records its call and fills the address with a family marker.
static thread_local int logs, last_error, dns_calls;
static thread_local std::string dns_host;
#define swoole_error_log(level, error, str, ...) (++logs)
static void swoole_set_last_error(int code) {
    last_error = code;
}
static int gethostbyname(int family, const char *host, char *out) {
    ++dns_calls;
    dns_host = host;
    errno = 31;
    if (dns_host == "resolve-fail") {
        return -1;
    }
    std::memset(out, family == AF_INET ? 0x41 : 0x61, family == AF_INET ? 4 : 16);
    return 0;
}
static size_t swoole_strlcpy(char *dst, const char *src, size_t size) {
    size_t length = std::strlen(src);
    if (size) {
        size_t copy = length < size - 1 ? length : size - 1;
        std::memcpy(dst, src, copy);
        dst[copy] = 0;
    }
    return length;
}
static thread_local char tmp_address[INET6_ADDRSTRLEN];
struct {
    std::string dns_hosts_path;
} SwooleG;
struct Socket {
    static bool is_inet4(int t) {
        return t == 1 || t == 2 || t == 7;
    }
    static bool is_inet6(int t) {
        return t == 3 || t == 4 || t == 8;
    }
    static bool is_local(int t) {
        return t == 5 || t == 6;
    }
};
struct Address {
    union {
        sockaddr_in inet_v4;
        sockaddr_in6 inet_v6;
        sockaddr_un un;
    } addr;
    socklen_t len = 0;
    SocketType type{};
    // assign(url) hands its parse to this recorder instead of resolving it.
    bool accept = true;
    std::string seen_host;
    int seen_port = 0;
    static bool verify_ip(int family, const std::string &str);
    static bool verify_port(const int port, const bool for_connect = false);
    bool assign(SocketType _type, const std::string &_host, int _port, bool _resolve_name);
    bool assign(SocketType _type, const std::string &_host, int _port) {
        type = _type;
        seen_host = _host;
        seen_port = _port;
        return accept;
    }
    bool assign(const std::string &url);
    bool is_loopback_addr() const;
};
#include "clients_address.inc"
#undef swoole_error_log
#include "clients_dns.inc"
}  // namespace upstream_clients

using namespace upstream_clients;

// A C++ locale whose ctype also classes NUL, 0xff and '@' as space.
struct ExtraWhitespace : std::ctype<char> {
    static const mask *classification() {
        static const std::array<mask, table_size> table = [] {
            std::array<mask, table_size> values{};
            std::copy_n(classic_table(), table_size, values.begin());
            values[0] |= space;
            values[255] |= space;
            values['@'] |= space;
            return values;
        }();
        return table.data();
    }
    ExtraWhitespace() : std::ctype<char>(classification()) {}
};

extern "C" {
// Address::assign(url) over length bytes, into a recorder that returns accept:
// its result, or -1 for std::out_of_range and -2 for std::invalid_argument.
int swr_oracle_clients_url(const char *data, size_t length, bool accept, int *type, int *port, char *host,
                           size_t *host_length, int *log_count) {
    Address address;
    // A sentinel no SocketType holds, copied as bytes: loading it as the enum is UB.
    const int unset = 87;
    static_assert(sizeof(address.type) == sizeof(unset));
    memcpy(&address.type, &unset, sizeof unset);
    address.accept = accept;
    logs = 0;
    int result;
    try {
        result = address.assign(std::string(data, length));
    } catch (const std::out_of_range &) {
        result = -1;
    } catch (const std::invalid_argument &) {
        result = -2;
    }
    memcpy(type, &address.type, sizeof *type);
    *port = address.seen_port;
    *host_length = address.seen_host.size();
    memcpy(host, address.seen_host.data(), address.seen_host.size());
    *log_count = logs;
    return result;
}

bool swr_oracle_clients_verify_port(int port, bool for_connect) {
    return Address::verify_port(port, for_connect);
}

// Address::verify_ip with errno 17 and its scratch filled with 0x53 first: the
// result, then errno and the scratch after.
bool swr_oracle_clients_verify_ip(int family, const char *host, size_t length, int *error, char *scratch) {
    std::memset(tmp_address, 0x53, sizeof(tmp_address));
    errno = 17;
    bool result = Address::verify_ip(family, std::string(host, length));
    *error = errno;
    std::memcpy(scratch, tmp_address, sizeof(tmp_address));
    return result;
}

// What Address::assign(type, host, port, resolve) leaves, over a record filled
// with 0x73, type -1, length 987, errno 17 and last error 11.
struct swr_oracle_clients_assign {
    unsigned char addr[128];
    socklen_t len;
    int type, result, error, last_error, dns_calls, loopback;
    size_t dns_host_length;
    char dns_host[256];
};

static_assert(sizeof(Address::addr) <= 128, "the trace holds the whole union");

size_t swr_oracle_clients_address_size() {
    return sizeof(Address::addr);
}

void swr_oracle_clients_assign(int type, const char *host, size_t length, int port, bool resolve,
                               swr_oracle_clients_assign *out) {
    Address address;
    std::memset(&address.addr, 0x73, sizeof(address.addr));
    address.type = static_cast<SocketType>(-1);
    address.len = 987;
    errno = 17;
    last_error = 11;
    dns_calls = 0;
    dns_host.clear();
    out->result = address.assign(static_cast<SocketType>(type), std::string(host, length), port, resolve);
    out->error = errno;
    out->last_error = last_error;
    out->dns_calls = dns_calls;
    out->dns_host_length = std::min(dns_host.size(), sizeof(out->dns_host));
    std::memcpy(out->dns_host, dns_host.data(), out->dns_host_length);
    out->type = address.type;
    out->len = address.len;
    out->loopback = address.is_loopback_addr();
    std::memcpy(out->addr, &address.addr, sizeof(address.addr));
}

// Address::is_loopback_addr for type over 16 address bytes (4 for IPv4 types).
bool swr_oracle_clients_loopback(int type, const unsigned char *bytes) {
    Address address{};
    address.type = static_cast<SocketType>(type);
    if (type == 1 || type == 2) {
        std::memcpy(&address.addr.inet_v4.sin_addr, bytes, 4);
    } else {
        std::memcpy(&address.addr.inet_v6.sin6_addr, bytes, 16);
    }
    return address.is_loopback_addr();
}

// Which bytes the C++ locale's ctype classes as space, as the hybrid lookup asks
// it (libc++ answers no for every byte past ASCII).
void swr_oracle_clients_spaces(bool extra_space, uint8_t *spaces) {
    const std::locale locale = extra_space ? std::locale(std::locale(), new ExtraWhitespace) : std::locale();
    const auto &facet = std::use_facet<std::ctype<char>>(locale);
    for (unsigned i = 0; i < 256; ++i) {
        spaces[i] = facet.is(std::ctype_base::space, static_cast<char>(i));
    }
}

// get_ip_by_hosts(name) over the hosts file at path, under a global C++ locale
// that classes extra bytes as space when extra_space is set; the address length.
size_t swr_oracle_clients_hosts(
    const char *path, const char *name, size_t length, bool extra_space, char *address, size_t capacity) {
    const auto previous = std::locale();
    if (extra_space) {
        std::locale::global(std::locale(previous, new ExtraWhitespace));
    }
    SwooleG.dns_hosts_path = path;
    const std::string found = get_ip_by_hosts(std::string(name, length));
    std::locale::global(previous);
    const size_t copied = std::min(found.size(), capacity);
    std::memcpy(address, found.data(), copied);
    return copied;
}

// php_swoole_http_parse_set_cookies's split of length bytes into key, value offset and value.
void swr_oracle_clients_cookie(const char *at, size_t length, size_t *key, size_t *offset, size_t *value) {
    struct zval {
        int unused;
    };
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

// dns_lookup_impl_with_socket and its codec, over a UDP socket double that
// answers the query it was sent with one record of the fixture's type.
namespace upstream_dns {
using uchar = unsigned char;
#define SW_OK 0
#define SW_ERR -1
#define SW_BUFFER_SIZE_STD 8192
#define SW_ERROR_DNSLOOKUP_NO_SERVER 714
#define SW_ERROR_CO_CANCELED 10017
#define sw_memset_zero(p, n) std::memset(p, 0, n)
#define sw_unlikely(x) __builtin_expect(!!(x), 0)
// As upstream src/core/base.cc.
static size_t sw_snprintf(char *buf, size_t size, const char *format, ...) {
    va_list args;
    va_start(args, format);
    int retval = vsnprintf(buf, size, format, args);
    va_end(args);
    if (size == 0) {
        return retval;
    } else if (sw_unlikely(retval < 0)) {
        retval = 0;
        buf[0] = '\0';
    } else if (sw_unlikely(retval >= (int) size)) {
        retval = size - 1;
        buf[retval] = '\0';
    }
    return retval;
}
#define swoole_warning(...) ((void) 0)
static constexpr int SW_SOCK_UDP = 2;
struct Fixture {
    int record_type, receive_mode;
    bool send_success, canceled, bad_id;
};
static thread_local const Fixture *fixture;
static thread_local void *context;
static thread_local void (*effect)(void *, int, const char *, size_t);
static void record(int kind, const std::string &text) {
    effect(context, kind, text.data(), text.size());
}
static void swoole_set_last_error(int code) {
    record(1, "error:" + std::to_string(code));
}
static bool swoole_load_resolv_conf() {
    return false;
}
static struct {
    struct {
        std::string host = "127.0.0.1";
        int port = 53;
    } dns_server;
} SwooleG;
static thread_local std::vector<char> sent_query;
struct Socket {
    int errCode = fixture->canceled ? ECANCELED : EIO;
    explicit Socket(int) {}
    void set_timeout(double) {
        record(1, "timeout");
    }
    bool sendto(const std::string &, int, const char *data, size_t length) {
        sent_query.assign(data, data + length);
        record(1, "send");
        record(2, std::string(data, length));
        return fixture->send_success;
    }
    ssize_t recv(void *buffer, size_t capacity);
};
}  // namespace upstream_dns

extern "C" {
// The socket double's answer to the query it was sent, into buffer: -1 when
// receive_mode is 0, the first 3 bytes when 1, all of it when 2.
ssize_t swr_oracle_clients_dns_answer(int record_type, int receive_mode, bool bad_id, const char *query,
                                      size_t length, char *buffer, size_t capacity) {
    if (receive_mode == 0) return -1;
    std::vector<unsigned char> response(query, query + length);
    response[6] = 0;
    response[7] = 1;
    if (bad_id) response[0] ^= 1;
    std::vector<unsigned char> payload;
    if (record_type == 1) payload = {127, 0, 0, 1};
    else if (record_type == 28) payload = {0x20, 1, 0x0d, 0xb8, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 1};
    else payload = {5, 'a', 'l', 'i', 'a', 's', 4, 't', 'e', 's', 't', 0};
    for (unsigned char byte : std::vector<unsigned char>{
             0xc0, 0x0c, 0, (unsigned char) record_type, 0, 1, 0, 0, 0, 0, 0, (unsigned char) payload.size()})
        response.push_back(byte);
    for (auto byte : payload) response.push_back(byte);
    size_t received = receive_mode == 1 ? 3 : response.size();
    if (received > capacity) std::abort();
    std::memcpy(buffer, response.data(), received);
    return received;
}
}

namespace upstream_dns {
ssize_t Socket::recv(void *buffer, size_t capacity) {
    record(1, "receive");
    return swr_oracle_clients_dns_answer(fixture->record_type, fixture->receive_mode, fixture->bad_id,
                                         sent_query.data(), sent_query.size(), static_cast<char *>(buffer), capacity);
}
static thread_local uint16_t dns_request_id = 1;
#include "clients_dns_lookup.inc"
}  // namespace upstream_dns

extern "C" {
// domain_encode over n bytes of a NUL-terminated src into dest.
int swr_oracle_clients_domain_encode(const char *src, int n, char *dest) {
    return upstream_dns::domain_encode(src, n, dest);
}

// parse_ip_address of a 4- or 16-byte address into text; its length.
size_t swr_oracle_clients_ip_text(void *address, int family, char *text, size_t capacity) {
    const auto result = upstream_dns::parse_ip_address(address, family);
    const size_t length = std::min(result.size(), capacity);
    std::memcpy(text, result.data(), length);
    return length;
}

// dns_lookup_impl_with_socket with request ids from 1 per thread. Each address
// goes to effect(context, 0, ...), each socket call and last error set as
// effect(context, 1, ...) and the query sent as effect(context, 2, ...).
void swr_oracle_clients_dns_lookup(const char *domain, int family, double timeout, int record_type,
                                   int receive_mode, bool send_success, bool canceled, bool bad_id, void *context,
                                   void (*effect)(void *, int, const char *, size_t)) {
    const upstream_dns::Fixture fixture{record_type, receive_mode, send_success, canceled, bad_id};
    upstream_dns::fixture = &fixture;
    upstream_dns::context = context;
    upstream_dns::effect = effect;
    for (const auto &address : upstream_dns::dns_lookup_impl_with_socket(domain, family, timeout)) {
        effect(context, 0, address.data(), address.size());
    }
}
}

// The pinned form formats and sw_snprintf, under the request formatter's calls.
namespace upstream_http_format {
#include "clients_http_format.inc"
// The pinned request formatter calls, by swoole_rs_clients::http_format kind.
static size_t bounded(unsigned kind, const std::string &a, const std::string &b, const std::string &c,
                      const std::string &d, size_t number, char *output, size_t capacity) {
    switch (kind) {
    case 0:
        return sw_snprintf(output, capacity, "Content-Length: %zu\r\n\r\n", number);
    case 1:
        if (std::memchr(a.data(), ':', a.size()))
            return sw_snprintf(output, capacity, "%s%s%s", "http://", a.c_str(), b.c_str());
        else
            return sw_snprintf(output, capacity, "%s%s:%u%s", "http://", a.c_str(), static_cast<unsigned>(number),
                               b.c_str());
    case 2:
        return sw_snprintf(output, capacity, "Content-Type: multipart/form-data; boundary=%.*s\r\n",
                           static_cast<int>(a.size()), a.c_str());
    case 3:
        return sw_snprintf(output, capacity, SW_HTTP_FORM_RAW_DATA_FMT, static_cast<int>(a.size()), a.c_str(),
                           static_cast<int>(b.size()), b.c_str());
    case 4:
        return sw_snprintf(output, capacity, SW_HTTP_FORM_FILE_DATA_FMT, static_cast<int>(a.size()), a.c_str(),
                           static_cast<int>(b.size()), b.c_str(), static_cast<int>(c.size()), c.c_str(),
                           static_cast<int>(d.size()), d.c_str());
    case 5:
        return sw_snprintf(output, capacity, "--%.*s--\r\n", static_cast<int>(a.size()), a.c_str());
    }
    return 0;
}
}  // namespace upstream_http_format

extern "C" {
// The request formatter's sw_snprintf call for `kind` over four texts, from errno 17;
// errno afterwards goes to *error.
size_t swr_oracle_clients_http_format(unsigned kind, const char *const *texts, const size_t *lengths, size_t number,
                                      char *output, size_t capacity, int *error) {
    const std::string a(texts[0], lengths[0]), b(texts[1], lengths[1]), c(texts[2], lengths[2]),
        d(texts[3], lengths[3]);
    errno = 17;
    const size_t written = upstream_http_format::bounded(kind, a, b, c, d, number, output, capacity);
    *error = errno;
    return written;
}
}
