#include "swoole_rs_framing.h"
#include <cassert>
#include <cstdint>
#include <cstring>
#include <iostream>
static size_t checks;
#define CHECK(x) do { assert(x); ++checks; } while (0)
int main(int argc, char **) {
    if (argc > 1) {
        // Expected invalid caller: prove instrumentation reaches the Rust read.
        auto *p = new uint8_t[1]{0}; int64_t result;
        return swr_v1_framing_unpack('C', p + 1, 1, &result);
    }
    CHECK(sizeof(SwrFramingIo) == 32);
    CHECK(swr_v1_framing_width('?') == 0);
    int64_t out = 918;
    uint8_t data[16]{};
    CHECK(swr_v1_framing_unpack('?', nullptr, 0, &out) == -1 && out == 918);
    CHECK(swr_v1_framing_unpack('N', data, 3, &out) == -2 && out == 918);
    CHECK(swr_v1_framing_unpack('C', nullptr, 1, &out) == -2);
    CHECK(swr_v1_framing_unpack('C', data, 1, nullptr) == -2);
    CHECK(swr_v1_framing_default('C', data, 1, 0, 0, 0, &out) == -1);
    CHECK(swr_v1_framing_default('N', data, 3, 2, 4, 0, &out) == 0 && out == 6);
    CHECK(swr_v1_framing_default('?', data, 1, 0, 1, 0, &out) == -2);
    CHECK(swr_v1_framing_default('N', data, 1, 0, 1, 0, &out) == -1);
    uint64_t huge = INT64_MAX;
    memcpy(data, &huge, 8);
    CHECK(swr_v1_framing_default('q', data, 8, 0, 8, 1, &out) == -1);
    // MIME lookup: the last suffix, lowercased; an empty or invalid extent finds nothing.
    const char *json = "a.html.JSON";
    CHECK(!strcmp(swr_v1_framing_mime_get((const uint8_t *) json, strlen(json)), "application/json"));
    CHECK(swr_v1_framing_mime_exists((const uint8_t *) json, strlen(json)));
    CHECK(!strcmp(swr_v1_framing_mime_get(nullptr, 0), "application/octet-stream"));
    CHECK(!swr_v1_framing_mime_exists(nullptr, 1));
    // Null receives and a buffer longer than its allocation fail before any effect.
    static size_t effects;
    SwrFramingCall count = [](void *, uint32_t, uint64_t) -> int64_t { ++effects; return 0; };
    CHECK(swr_v1_framing_recv(nullptr, nullptr, count) == -1);
    struct { size_t length, size; int64_t offset; char *str; } buffer{2, 1, 0, nullptr};
    uint8_t protocol[20]{};
    SwrFramingIo io{protocol, &buffer, SWR_FRAMING_MODE_LENGTH, 1, 0};
    CHECK(swr_v1_framing_recv(&io, nullptr, nullptr) == -1);
    CHECK(swr_v1_framing_recv(&io, nullptr, count) == -1);
    // A receive larger than the remaining allocation is refused.
    buffer.length = 0; protocol[12] = 3;
    CHECK(swr_v1_framing_recv(&io, nullptr, count) == -1 && effects == 0);
    std::cout << "{\"checks\":" << checks << ",\"passed\":true}\n";
}
