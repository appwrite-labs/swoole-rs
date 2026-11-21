// The pinned WebSocket codec behind C entry points. build.rs copies the function
// bodies and definitions from upstream/src/protocol/websocket.cc and
// upstream/include/swoole_websocket.h unchanged into websocket.inc; only logging
// and the socket types the length query ignores are stubbed here.
#include <arpa/inet.h>
#include <cinttypes>
#include <cstdint>
#include <cstring>
#include <sys/types.h>

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
// Upstream get_package_length ignores its protocol and socket parameters.
#pragma GCC diagnostic ignored "-Wunused-parameter"
#include "websocket.inc"
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
}
