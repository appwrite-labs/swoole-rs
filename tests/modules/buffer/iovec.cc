// Defined positive POSIX vector consumers only; no original offset-edge oracle.
#include "swoole.h"
#include "swoole_buffer.h"
#ifdef SWOOLE_RS_BUFFER
#include "swoole_rs_buffer_posix.h"
#endif
#include <array>
#include <cstddef>
#include <iostream>
#include <stdexcept>
#include <string>
#ifndef SWOOLE_RS_BUFFER
swoole::Global SwooleG{};  // upstream's; the Rust archive defines its own
#endif
// The first entry's unsent bytes: upstream's front() view, the Rust Buffer's typed_front().
struct Head { const char *data; uint32_t length, size, offset; };
static Head head(const swoole::Buffer &buffer) {
#ifdef SWOOLE_RS_BUFFER
    const auto front = buffer.typed_front();
    return {reinterpret_cast<const char *>(front.data), front.length, front.length, front.offset};
#else
    const auto *front = buffer.front();
    return {front->value.str, front->length, front->size, front->offset};
#endif
}
static unsigned checks = 0;
static void check(bool ok, const char *what) { if (!ok) throw std::runtime_error(what); ++checks; }
int main() {
    try {
#ifdef SWOOLE_RS_BUFFER
        const auto layout = swr_v1_buffer_iovec_layout();
        check(layout.size == sizeof(iovec), "actual C/Rust iovec size");
        check(layout.alignment == alignof(iovec), "actual C/Rust iovec alignment");
        check(layout.base_offset == offsetof(iovec, iov_base), "actual C/Rust base field offset");
        check(layout.length_offset == offsetof(iovec, iov_len), "actual C/Rust length field offset");
#endif
        for (unsigned count : {1u, 8u, 9u, 32u}) {
            std::array<std::array<char, 3>, 32> bytes{};
            std::array<iovec, 32> parts{};
            std::string expected;
            for (unsigned i = 0; i < count; ++i) {
                bytes[i] = {char(0), char(i), char(255)};
                parts[i] = {bytes[i].data(), bytes[i].size()};
                expected.append(bytes[i].data(), bytes[i].size());
            }
            for (unsigned chunk : {0u, 1u, 4u}) {
                swoole::Buffer buffer(chunk);
                buffer.append(parts.data(), count, 0);
                check(buffer.length() == expected.size(), "positive vector byte count");
                const size_t effective = chunk ? chunk : INT_MAX;
                check(buffer.count() == (expected.size() + effective - 1) / effective, "positive vector chunk count");
                std::string copied;
                while (!buffer.empty()) {
                    const auto front = head(buffer);
                    check(front.offset == 0 && front.length <= front.size, "initialized front span");
                    copied.append(front.data, front.length);
                    buffer.pop();
                    check(buffer.length() == expected.size() - copied.size(), "positive vector pop accounting");
                }
                check(copied == expected && buffer.count() == 0, "exact vector binary data/drain");
            }
        }
        std::cout << "{\"consumer_checks\":" << checks << ",\"cases\":12,\"layout\":{\"size\":" << sizeof(iovec)
                  << ",\"alignment\":" << alignof(iovec) << ",\"base_offset\":" << offsetof(iovec, iov_base)
                  << ",\"length_offset\":" << offsetof(iovec, iov_len) << "},\"errors\":0}\n";
    } catch (const std::exception &error) { std::cerr << error.what() << '\n'; return 1; }
}
