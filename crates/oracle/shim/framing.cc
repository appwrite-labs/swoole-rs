// The pinned packet length function and swoole_unpack behind C entry points.
// build.rs copies their bodies from upstream/src/protocol/base.cc unchanged into
// framing.inc. Stubbed here: logging, the socket the length function ignores,
// and swoole.h's byte-swap macros.
#include <arpa/inet.h>
#include <cstdint>
#include <cstring>
#include <sys/types.h>

#define SW_ERR -1
#define SW_LITTLE_ENDIAN 1
#define SW_BIG_ENDIAN 2
#if __BYTE_ORDER__ == __ORDER_LITTLE_ENDIAN__
#define SW_BYTE_ORDER SW_LITTLE_ENDIAN
#else
#define SW_BYTE_ORDER SW_BIG_ENDIAN
#endif
#define swoole_swap_endian16(x) __builtin_bswap16(x)
#define swoole_swap_endian32(x) __builtin_bswap32(x)
#define swoole_swap_endian64(x) __builtin_bswap64(x)
#define swoole_debug(...) ((void) 0)
#define swoole_error_log(...) ((void) 0)

namespace oracle_framing {
namespace network {
struct Socket {};
}  // namespace network
struct PacketLength {
    const char *buf;
    uint32_t buf_size;
    uint32_t header_len;
};
int64_t swoole_unpack(char type, const void *data);
uint64_t swoole_ntoh64(uint64_t value);
// The fields default_length_func reads, with upstream's types.
struct Protocol {
    char package_length_type;
    uint8_t package_length_size;
    uint16_t package_length_offset;
    uint16_t package_body_offset;
    uint8_t (*get_package_length_size)(network::Socket *);
    static ssize_t default_length_func(const Protocol *protocol, network::Socket *socket, PacketLength *pl);
};
#include "framing.inc"
}  // namespace oracle_framing

extern "C" {
int64_t swr_oracle_framing_unpack(char type, const void *data) {
    return oracle_framing::swoole_unpack(type, data);
}

ssize_t swr_oracle_framing_length(char type, uint8_t size, uint16_t offset, uint16_t body_offset,
                                  const char *data, uint32_t length, uint32_t *header_length) {
    oracle_framing::Protocol protocol{type, size, offset, body_offset, nullptr};
    oracle_framing::PacketLength pl{data, length, 0};
    ssize_t total = oracle_framing::Protocol::default_length_func(&protocol, nullptr, &pl);
    *header_length = pl.header_len;
    return total;
}
}
