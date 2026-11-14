// MQTT's length decoder and RESP's parse behind C entry points, once per engine:
// build.rs copies the pinned bodies (upstream) or the bodies with the protocols
// overlay patch applied (candidate, which calls the Rust exports) into
// SWR_PROTO_INC unchanged.
#include <cerrno>
#include <cstdint>
#include <cstdlib>
#include <cstring>
#include <string>
#include <sys/types.h>
#include <vector>
#include "swoole_rs_protocols.h"

#define SWR_CAT2(a, b) a##_##b
#define SWR_CAT(a, b) SWR_CAT2(a, b)

namespace SWR_PROTO_ENGINE {
// As upstream include/swoole.h, swoole_log.h (logging off), swoole_mqtt.h and swoole_protocol.h.
enum { SW_OK = 0, SW_ERR = -1 };
#define SW_CRLF "\r\n"
#define SW_CRLF_LEN 2
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
#include SWR_PROTO_INC
}  // namespace SWR_PROTO_ENGINE

extern "C" {
// Defined once, by the upstream build.
#if SWR_PROTO_UPSTREAM
void swr_oracle_free(void *pointer) { free(pointer); }
#endif

// get_package_length over size bytes.
int64_t SWR_CAT(swr_oracle_mqtt_length, SWR_PROTO_ENGINE)(const char *data, uint32_t size) {
    SWR_PROTO_ENGINE::PacketLength length{data, size, 0};
    return SWR_PROTO_ENGINE::get_package_length(nullptr, nullptr, &length);
}

// parse over length bytes followed by a NUL, from errno 0. Returns each string as
// u32 length and bytes in a buffer of *size bytes to release with swr_oracle_free;
// *error is errno after.
char *SWR_CAT(swr_oracle_resp_parse, SWR_PROTO_ENGINE)(const char *data, size_t length, size_t *size, int *error) {
    errno = 0;
    std::vector<std::string> items = SWR_PROTO_ENGINE::parse(data, length);
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
}
