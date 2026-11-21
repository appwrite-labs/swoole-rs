// The pinned HttpProxy behind C entry points. build.rs copies create, get_auth_str,
// pack and handshake from upstream/src/protocol/http.cc unchanged into
// http_proxy.inc; it compiles with `swoole` renamed so the class comes from the
// pinned swoole_proxy.h. The facade is what those bodies use from swoole.h,
// swoole_string.h, swoole_error.h, base.cc (sw_snprintf) and base64.cc.
#include <cctype>
#include <cerrno>
#include <cstdarg>
#include <cstdint>
#include <cstdio>
#include <cstring>
#include <strings.h>
#include <sys/types.h>
#include "swoole_proxy.h"

#define SWOOLE_VERSION "6.2.5"
#define SW_STRL(s) s, sizeof(s) - 1
#define SW_STR_ISTARTS_WITH(str, len, const_sub_str) swoole_str_istarts_with(str, len, SW_STRL(const_sub_str))
#define sw_unlikely(x) __builtin_expect(!!(x), 0)

namespace oracle {
size_t base64_encode(const unsigned char *in, size_t inlen, char *out);
}

namespace swoole {
class String {
  public:
    size_t length, size;
    off_t offset;
    char *str;
};
enum { SW_ERROR_HTTP_PROXY_HANDSHAKE_FAILED = 7103 };
static thread_local int last_error = 0;
static void swoole_set_last_error(int error) {
    last_error = error;
}
static inline unsigned int swoole_str_istarts_with(const char *pstr, size_t plen, const char *sstr, size_t slen) {
    return (plen >= slen) && (strncasecmp(pstr, sstr, slen) == 0);
}
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
using oracle::base64_encode;
#include "http_proxy.inc"
}  // namespace swoole

using swoole::HttpProxy;

static std::string text(const uint8_t *data, size_t length) {
    return {reinterpret_cast<const char *>(data), length};
}

static HttpProxy proxy(const uint8_t *user, size_t user_length, const uint8_t *password, size_t password_length,
                       const uint8_t *target, size_t target_length, int target_port) {
    HttpProxy proxy{};
    proxy.username = text(user, user_length);
    proxy.password = text(password, password_length);
    proxy.target_host = text(target, target_length);
    proxy.target_port = target_port;
    return proxy;
}

extern "C" {
// HttpProxy::create's fields: the port, then host, username and password copied
// into output (capacity host + user + password lengths) with their lengths in
// lengths[3].
int swr_oracle_http_proxy_create(const uint8_t *host, size_t host_length, int port, const uint8_t *user,
                                 size_t user_length, const uint8_t *password, size_t password_length, uint8_t *output,
                                 size_t *lengths) {
    HttpProxy *created = HttpProxy::create(text(host, host_length), port, text(user, user_length),
                                           text(password, password_length));
    size_t at = 0, index = 0;
    for (const std::string *field : {&created->host, &created->username, &created->password}) {
        memcpy(output + at, field->data(), field->size());
        at += field->size();
        lengths[index++] = field->size();
    }
    int result = created->port;
    delete created;
    return result;
}

// get_auth_str over these credentials, copied into output (capacity 512), with
// errno set to EINTR before and stored in *error after; returns its length.
size_t swr_oracle_http_proxy_auth(const uint8_t *user, size_t user_length, const uint8_t *password,
                                  size_t password_length, uint8_t *output, int *error) {
    HttpProxy subject = proxy(user, user_length, password, password_length, nullptr, 0, 0);
    errno = EINTR;
    std::string auth = subject.get_auth_str();
    *error = errno;
    memcpy(output, auth.data(), auth.size());
    return auth.size();
}

// pack into output (capacity bytes; a null output with capacity 0 asks the
// length), with errno set to EINTR before and stored in *error after.
size_t swr_oracle_http_proxy_pack(const uint8_t *user, size_t user_length, const uint8_t *password,
                                  size_t password_length, const uint8_t *target, size_t target_length,
                                  int target_port, const uint8_t *host, size_t host_length, uint8_t *output,
                                  size_t capacity, int *error) {
    HttpProxy subject = proxy(user, user_length, password, password_length, target, target_length, target_port);
    swoole::String buffer{0, capacity, 0, reinterpret_cast<char *>(output)};
    errno = EINTR;
    size_t written = subject.pack(&buffer, text(host, host_length));
    *error = errno;
    return written;
}

// handshake over length bytes, with the last error (0 before) in *error; errno
// is EINTR before it.
bool swr_oracle_http_proxy_handshake(const uint8_t *data, size_t length, int *error) {
    swoole::String buffer{length, length, 0, reinterpret_cast<char *>(const_cast<uint8_t *>(data))};
    swoole::last_error = 0;
    errno = EINTR;
    bool accepted = HttpProxy::handshake(&buffer);
    *error = swoole::last_error;
    return accepted;
}
}
