// The pinned WebSocket codec behind C entry points. build.rs copies the function
// bodies and definitions from upstream/src/protocol/websocket.cc and
// upstream/include/swoole_websocket.h unchanged into websocket.inc, and the PHP
// server's pack and message policy from upstream/ext-src/swoole_websocket_server.cc
// into websocket_pack.inc and websocket_drop.inc; only logging, the output String
// and the socket types the length query ignores are stubbed here.
#include <arpa/inet.h>
#include <cinttypes>
#include <cstdint>
#include <cstring>
#include <sys/types.h>
#include <vector>

#define swoole_trace_log(...) ((void) 0)
#define swoole_error_log(...) ((void) 0)
#define swoole_warning(...) ((void) 0)
#define sw_unlikely(value) (value)

namespace oracle_ws {
using uchar = unsigned char;
struct Protocol {};
struct Socket {};
struct PacketLength {
    const char *buf;
    uint32_t buf_size;
    uint32_t header_len;
};
static uint64_t swoole_ntoh64(uint64_t value) {
    unsigned char bytes[8];
    std::memcpy(bytes, &value, 8);
    uint64_t result = 0;
    for (auto byte : bytes) result = (result << 8) | byte;
    return result;
}
static uint64_t swoole_hton64(uint64_t value) {
    return swoole_ntoh64(value);
}
// What encode needs of swoole::String.
struct String {
    std::vector<char> bytes;
    char *str = nullptr;
    size_t length = 0;
    void append(const char *data, size_t size) {
        if (!size) return;
        bytes.insert(bytes.end(), data, data + size);
        str = bytes.data();
        length = bytes.size();
    }
};
// Upstream get_package_length ignores its protocol and socket parameters.
#pragma GCC diagnostic ignored "-Wunused-parameter"
// The pinned codec reads and writes words at byte offsets, which UBSan's alignment check
// reports wherever the sanitizer run builds it (the native gate it replaced built it
// with -fno-sanitize=alignment).
#ifdef __clang__
#pragma clang attribute push(__attribute__((no_sanitize("alignment"))), apply_to = function)
#endif
#include "websocket.inc"
#ifdef __clang__
#pragma clang attribute pop
#endif

namespace WebSocket = oracle_ws;
#include "websocket_bits.inc"
// WebSocketFrame pack's flag normalization: the flags it encodes with in the low
// byte, whether it compresses in bit 8.
static uint16_t pack_policy(uint8_t opcode, uint8_t flags, size_t len) {
#include "websocket_pack.inc"
    return uint16_t(flags) | (uint16_t(need_compress) << 8);
}
// The server's condition for keeping a close, ping or pong frame from onMessage
// (settings bits close=1, ping=2, pong=4): 0 deliver, 1 drop, 2 answer with a pong.
static uint8_t message_policy(uint8_t opcode, uint8_t settings) {
    struct Settings {
        bool open_close_frame, open_ping_frame, open_pong_frame;
    };
    struct Port {
        Settings websocket_settings;
    } value{{bool(settings & 1), bool(settings & 2), bool(settings & 4)}};
    auto *port = &value;
    const bool drop =
#include "websocket_drop.inc"
        ;
    return drop ? (opcode == OPCODE_PING ? 2 : 1) : 0;
}
// `buffer`'s bytes into `out` when they fit; its length. An empty buffer has no `str`.
static size_t copy_out(const String &buffer, char *out, size_t capacity) {
    if (buffer.length && buffer.length <= capacity) std::memcpy(out, buffer.str, buffer.length);
    return buffer.length;
}
}  // namespace oracle_ws

struct swr_oracle_ws_frame {
    uint64_t payload_length;
    uint16_t header_length;
    uint8_t opcode, flags, length_code;
    char mask_key[4];
};

extern "C" {
ssize_t swr_oracle_ws_frame_length(const char *data, size_t length, uint32_t *header_length) {
    oracle_ws::PacketLength pl{data, (uint32_t) length, *header_length};
    ssize_t total = oracle_ws::get_package_length_impl(&pl);
    *header_length = pl.header_len;
    return total;
}

bool swr_oracle_ws_decode(char *data, size_t length, swr_oracle_ws_frame *out) {
    oracle_ws::Frame frame{};
    bool ok = oracle_ws::decode(&frame, data, length);
    out->payload_length = frame.payload_length;
    out->header_length = frame.header_length;
    out->opcode = frame.header.OPCODE;
    out->flags = frame.get_flags();
    out->length_code = frame.header.LENGTH;
    std::memcpy(out->mask_key, frame.mask_key, 4);
    return ok;
}

void swr_oracle_ws_mask(char *data, size_t length, const char *key) {
    oracle_ws::mask(data, length, key);
}

bool swr_oracle_ws_encode(
    const char *data, size_t length, uint8_t opcode, uint8_t flags, char *out, size_t capacity, size_t *written) {
    oracle_ws::String buffer;
    bool ok = oracle_ws::encode(&buffer, data, length, opcode, flags);
    *written = oracle_ws::copy_out(buffer, out, capacity);
    return ok;
}

bool swr_oracle_ws_pack_close(
    int code, const char *reason, size_t length, uint8_t flags, char *out, size_t capacity, size_t *written) {
    oracle_ws::String buffer;
    bool ok = oracle_ws::pack_close_frame(&buffer, code, reason, length, flags);
    *written = oracle_ws::copy_out(buffer, out, capacity);
    return ok;
}

uint16_t swr_oracle_ws_pack_policy(uint8_t opcode, uint8_t flags, size_t length) {
    return oracle_ws::pack_policy(opcode, flags, length);
}

uint8_t swr_oracle_ws_message_policy(uint8_t opcode, uint8_t settings) {
    return oracle_ws::message_policy(opcode, settings);
}
}
