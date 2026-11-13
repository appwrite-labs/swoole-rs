// MQTT's length decoder, RESP's get_number, parse and format, and the PHP redis
// server's request loop behind C entry points: build.rs copies the pinned bodies
// into protocols_upstream.inc and protocols_php_request.inc unchanged.
#include <cerrno>
#include <cinttypes>
#include <cstdarg>
#include <cstdint>
#include <cstdlib>
#include <cstring>
#include <string>
#include <sys/types.h>
#include <utility>
#include <vector>

namespace upstream {
// As upstream include/swoole.h, swoole_log.h (logging off), swoole_mqtt.h and swoole_protocol.h.
enum { SW_OK = 0, SW_ERR = -1 };
#define SW_CRLF "\r\n"
#define SW_CRLF_LEN 2
#define SW_STRL(s) s, sizeof(s) - 1
#define SW_MQTT_MIN_LENGTH_SIZE 1
#define SW_MQTT_MAX_LENGTH_SIZE 4
#define swoole_error_log(level, error, str, ...)
#include "protocols_states.inc"
struct Protocol;
struct Socket;
struct PacketLength {
    const char *buf;
    uint32_t buf_size;
    uint32_t header_len;
};
// What format uses of swoole::String.
struct String {
    std::string value;
    void append(const char *data, size_t length) {
        value.append(data, length);
    }
    void append(const std::string &data) {
        value += data;
    }
    void format(const char *pattern, ...) {
        va_list arguments, copy;
        va_start(arguments, pattern);
        va_copy(copy, arguments);
        const int count = vsnprintf(nullptr, 0, pattern, copy);
        va_end(copy);
        std::vector<char> result(count + 1);
        vsnprintf(result.data(), result.size(), pattern, arguments);
        va_end(arguments);
        value.append(result.data(), count);
    }
};
#include "protocols_upstream.inc"

// The request loop's Zend calls, recording each parameter: 0 string, 1 integer (in
// decimal), 2 null.
using zval = std::vector<std::pair<uint32_t, std::string>>;
static void array_init(zval *) {}
static void add_next_index_null(zval *out) {
    out->emplace_back(2, "");
}
static void add_next_index_long(zval *out, int value) {
    out->emplace_back(1, std::to_string(value));
}
static void add_next_index_stringl(zval *out, const char *data, size_t length) {
    out->emplace_back(0, std::string(data, length));
}

// Each item as u32 kind, u32 length and bytes, in a malloc'd buffer of *size bytes.
static char *pack(const zval &items, size_t *size) {
    *size = 0;
    for (const auto &item : items) *size += 8 + item.second.size();
    char *output = static_cast<char *>(malloc(*size + 1));
    char *at = output;
    for (const auto &item : items) {
        const uint32_t length = static_cast<uint32_t>(item.second.size());
        memcpy(at, &item.first, 4);
        memcpy(at + 4, &length, 4);
        memcpy(at + 8, item.second.data(), length);
        at += 8 + length;
    }
    return output;
}
}  // namespace upstream

extern "C" {
void swr_oracle_free(void *pointer) {
    free(pointer);
}

// get_package_length over size bytes.
int64_t swr_oracle_mqtt_length_upstream(const char *data, uint32_t size) {
    upstream::PacketLength length{data, size, 0};
    return upstream::get_package_length(nullptr, nullptr, &length);
}

// parse over length bytes followed by a NUL, from errno `initial`. Returns each
// string as u32 length and bytes in a buffer of *size bytes to release with
// swr_oracle_free; *error is errno after.
char *swr_oracle_resp_parse_upstream(const char *data, size_t length, int initial, size_t *size, int *error) {
    errno = initial;
    std::vector<std::string> items = upstream::parse(data, length);
    *error = errno;
    *size = 0;
    for (const auto &item : items) *size += 4 + item.size();
    char *output = static_cast<char *>(malloc(*size + 1));
    char *at = output;
    for (const auto &item : items) {
        uint32_t prefix = static_cast<uint32_t>(item.size());
        memcpy(at, &prefix, 4);
        memcpy(at + 4, item.data(), item.size());
        at += 4 + item.size();
    }
    return output;
}

// get_number over a NUL-terminated line, from errno `initial`: whether it returned a
// position; *error is errno after.
bool swr_oracle_resp_get_number(const char *line, int initial, int *error) {
    int value = 0;
    errno = initial;
    const bool found = upstream::get_number(line, &value) != nullptr;
    *error = errno;
    return found;
}

// format(String *, ReplyType, const std::string &) into an empty String: its bytes,
// *size of them, to release with swr_oracle_free.
char *swr_oracle_resp_format_string(int type, const char *value, size_t length, size_t *size) {
    upstream::String buffer;
    upstream::format(&buffer, static_cast<upstream::ReplyType>(type), std::string(value, length));
    *size = buffer.value.size();
    char *output = static_cast<char *>(malloc(*size + 1));
    memcpy(output, buffer.value.data(), *size);
    return output;
}

// format(String *, ReplyType, long), as swr_oracle_resp_format_string.
char *swr_oracle_resp_format_long(int type, long value, size_t *size) {
    upstream::String buffer;
    upstream::format(&buffer, static_cast<upstream::ReplyType>(type), value);
    *size = buffer.value.size();
    char *output = static_cast<char *>(malloc(*size + 1));
    memcpy(output, buffer.value.data(), *size);
    return output;
}

// The PHP redis server's request loop (swoole_redis_server.cc's onReceive) over
// wire_length bytes followed by a NUL, from errno `initial`, with only Zend's array
// modeled. Returns the items as u32 kind, u32 length and bytes in a buffer of *size
// bytes to release with swr_oracle_free: first the command (kind 1 once a string
// was taken as the command, else 0 and empty), then each parameter (0 string,
// 1 integer in decimal, 2 null). *error is errno after.
char *swr_oracle_resp_php_request(const char *wire, size_t wire_length, int initial, size_t *size, int *error) {
    using namespace upstream;
    namespace Redis = upstream;
    errno = initial;
#define Z_STRVAL(unused) wire
#define Z_STRLEN(unused) wire_length
#include "protocols_php_request.inc"
#undef Z_STRVAL
#undef Z_STRLEN
    *error = errno;
    zval items;
    items.emplace_back(add_param ? 1 : 0, command ? std::string(command, command_len) : std::string());
    items.insert(items.end(), zparams.begin(), zparams.end());
    return pack(items, size);
}
}
