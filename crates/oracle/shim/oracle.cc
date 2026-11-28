#include "swoole_base64.h"

extern "C" size_t swr_oracle_base64_encode(const unsigned char *in, size_t length, char *out) {
    return oracle::base64_encode(in, length, out);
}
extern "C" size_t swr_oracle_base64_decode(const char *in, size_t length, char *out) {
    return oracle::base64_decode(in, length, out);
}

#include "swoole_hash.h"

extern "C" uint32_t swr_oracle_crc32(const char *data, size_t size) {
    return swoole_crc32(data, size);
}
