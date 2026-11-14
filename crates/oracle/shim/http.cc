// The pinned HTTP cookie parser and URL codec behind C entry points. build.rs
// copies their bodies from upstream/src/protocol/http.cc unchanged into http.inc.
// Adapted here: the thread-global buffer parse_cookie copies into, and one spare
// byte per sw_malloc, so upstream completes on every input.
#include <cctype>
#include <cstdint>
#include <cstdlib>
#include <cstring>
#include <functional>
#include <vector>

namespace oracle_http {
using uchar = unsigned char;
typedef std::function<bool(char *key, size_t key_len, char *value, size_t value_len)> ParseCookieCallback;
struct Buffer {
    char *str;
};
static thread_local std::vector<char> cookie_storage;
static Buffer *sw_tg_buffer() {
    static thread_local Buffer buffer;
    buffer.str = cookie_storage.data();
    return &buffer;
}
#define sw_malloc(size) malloc((size) + 1)
#define sw_free(pointer) free(pointer)
#include "http.inc"
}  // namespace oracle_http

extern "C" {
size_t swr_oracle_http_url_decode(char *data, size_t length) {
    return oracle_http::url_decode(data, length);
}

// Writes the encoding and its NUL into output (capacity length * 3 + 1); returns its length.
size_t swr_oracle_http_url_encode(const char *data, size_t length, char *output) {
    char *encoded = oracle_http::url_encode(data, length);
    size_t size = strlen(encoded);
    memcpy(output, encoded, size + 1);
    free(encoded);
    return size;
}

// Serializes each callback as u32 key length, key, u32 value length, value into
// output (capacity 8 * length + 16 suffices); returns the bytes written. The callback
// returns false after the first record when `first_only`. When `scratch` is not
// null, the length + 1 bytes parse_cookie tokenized in place are copied into it.
size_t swr_oracle_http_cookies(const char *data, size_t length, char *output, bool first_only, char *scratch) {
    oracle_http::cookie_storage.assign(length + 1, 0);
    size_t at = 0;
    auto put = [&](const char *bytes, size_t size) {
        uint32_t prefix = (uint32_t) size;
        memcpy(output + at, &prefix, 4);
        memcpy(output + at + 4, bytes, size);
        at += 4 + size;
    };
    oracle_http::parse_cookie(data, length, [&](char *key, size_t key_len, char *value, size_t value_len) {
        put(key, key_len);
        put(value, value_len);
        return !first_only;
    });
    if (scratch) {
        memcpy(scratch, oracle_http::cookie_storage.data(), length + 1);
    }
    return at;
}
}
