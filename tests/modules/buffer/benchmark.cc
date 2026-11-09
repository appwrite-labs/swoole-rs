// Compare the actual upstream Buffer with the patched facade on valid inputs.
#include "swoole.h"
#include "swoole_buffer.h"
#include <chrono>
#include <cstring>
#include <iostream>
#include <stdexcept>
#include <vector>

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

static void require(bool result, const char *message) {
    if (!result) throw std::runtime_error(message);
}

int main(int argc, char **argv) {
    try {
        require(argc == 5 || (argc == 6 && std::strcmp(argv[5], "peek") == 0), "Expected bytes chunk-size vector-count iterations [peek]");
        const bool peek = argc == 6;
        const auto size = uint32_t(std::stoul(argv[1]));
        const auto chunk_size = uint32_t(std::stoul(argv[2]));
        const auto vectors = uint32_t(std::stoul(argv[3]));
        const bool validate = std::strcmp(argv[4], "check") == 0;
        const auto iterations = validate ? 1 : std::stoull(argv[4]);
        require(size && chunk_size && iterations && (!vectors || size % vectors == 0), "Invalid workload");
        std::vector<char> bytes(size);
        for (size_t i = 0; i < size; ++i) bytes[i] = char((i * 197 + 17) & 255);
        std::vector<iovec> iov(vectors);
        for (uint32_t i = 0; i < vectors; ++i) iov[i] = {bytes.data() + i * (size / vectors), size / vectors};
        swoole::Buffer buffer(chunk_size);
        auto append = [&]() {
            if (vectors) buffer.append(iov.data(), iov.size(), 0);
            else buffer.append(bytes.data(), size);
        };
        auto verify = [&]() {
            append();
            require(buffer.length() == size, "Wrong byte accounting");
            require(buffer.count() == (size + chunk_size - 1) / chunk_size, "Wrong chunk count");
            size_t offset = 0;
            while (!buffer.empty()) {
                const auto front = head(buffer);
                require(front.length == front.size && front.length <= size - offset, "Wrong chunk length");
                require(std::memcmp(front.data, bytes.data() + offset, front.length) == 0, "Wrong payload");
                offset += front.length;
                buffer.pop();
            }
            require(offset == size && buffer.length() == 0 && buffer.count() == 0, "Wrong final state");
        };
        verify();
        if (validate) { std::cout << "{\"validation_only\":true,\"errors\":0}\n"; return 0; }
        for (unsigned i = 0; i < 100; ++i) { append(); while (!buffer.empty()) buffer.pop(); }
        uint64_t checksum = 0, expected_checksum = 0;
        if (peek) for (size_t offset = 0; offset < size; offset += chunk_size) expected_checksum += uint8_t(bytes[offset]);
        const auto start = std::chrono::steady_clock::now();
        for (uint64_t i = 0; i < iterations; ++i) {
            append();
            while (!buffer.empty()) {
                if (peek) checksum += uint8_t(head(buffer).data[0]);
                buffer.pop();
            }
        }
        const auto ns = std::chrono::duration_cast<std::chrono::nanoseconds>(std::chrono::steady_clock::now() - start).count();
        require(checksum == expected_checksum * iterations, "Wrong consumed checksum");
        require(buffer.empty() && buffer.length() == 0 && buffer.count() == 0, "Wrong timed final state");
        verify();
        std::cout << "{\"iterations\":" << iterations << ",\"bytes_per_cycle\":" << size
                  << ",\"elapsed_ns\":" << ns << ",\"ns_per_cycle\":" << double(ns) / iterations
                  << ",\"bytes_per_second\":" << double(size) * iterations * 1e9 / ns
                  << ",\"peek_front\":" << (peek ? "true" : "false")
                  << ",\"errors\":0}\n";
        return 0;
    } catch (const std::exception &error) {
        std::cerr << error.what() << '\n';
        return 1;
    }
}
