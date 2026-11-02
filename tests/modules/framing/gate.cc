#include "swoole_rs_framing.h"
#include <algorithm>
#include <cassert>
#include <cctype>
#include <clocale>
#include <cstdint>
#include <cstring>
#include <iostream>
#include <random>
#include <string>
#include <vector>
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
    // Empty and invalid extents never manufacture a borrow or modify output.
    CHECK(swr_v1_framing_mime_suffix(nullptr, 0, nullptr, 0, 1) == 0);
    CHECK(swr_v1_framing_mime_suffix(nullptr, 1, data, sizeof(data), 1) == SIZE_MAX);
    CHECK(swr_v1_framing_mime_suffix(data, 3, nullptr, 3, 1) == SIZE_MAX);
    SwrFramingSpan key{}, value{};
    CHECK(swr_v1_framing_mime_count() == 362);
    CHECK(!swr_v1_framing_mime_entry(362, &key, &value));
    CHECK(!swr_v1_framing_mime_entry(0, nullptr, &value));
    for (size_t n = 0; n < 362; n++) {
        CHECK(swr_v1_framing_mime_entry(n, &key, &value));
        CHECK(key.data && key.len && value.data && value.len);
        SwrFramingSpan again{}, again_value{};
        CHECK(swr_v1_framing_mime_entry(n, &again, &again_value));
        CHECK(key.data == again.data && value.data == again_value.data);
    }
    std::mt19937 rng(1241);
    for (const char *locale: {"C", "C.UTF-8"}) {
        if (!setlocale(LC_CTYPE, locale)) { std::cerr << "locale unavailable: " << locale << '\n'; continue; }
        for (int n = 0; n < 5000; n++) {
            std::string s(rng() % 100, '\0'); for (auto &c: s) c = rng();
            size_t start = s.find_last_of('.') + 1;
            for (uint8_t native_signed: {0, 1}) {
                std::vector<uint8_t> output(s.size() + 2, 171);
                auto len = swr_v1_framing_mime_suffix((const uint8_t *) s.data(), s.size(), output.data() + 1, s.size(), native_signed);
                CHECK(len == s.size() - start);
                CHECK(output.front() == 171 && output.back() == 171);
                for (size_t k = 0; k < len; k++) {
                    auto c = (uint8_t) s[start + k];
                    CHECK(output[k + 1] == (uint8_t) std::tolower(native_signed && c == 255 ? EOF : (int) c));
                }
                if (len) CHECK(swr_v1_framing_mime_suffix((const uint8_t *) s.data(), s.size(), output.data() + 1, len - 1, native_signed) == SIZE_MAX);
            }
        }
    }
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
