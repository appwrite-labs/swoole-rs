// Native String consumer; baseline and candidate use the same source and flags.
#include "swoole_string.h"
#include <atomic>
#include <chrono>
#include <cstring>
#include <iostream>
#include <stdexcept>
#include <vector>
constexpr int integers[] = {0, 1, -1, 10, -10, 999, -999, INT32_MIN, INT32_MAX};
// Keep whole-query observations across the same native call boundary in both
// engines; prevent a compiler from hoisting unchanged pure queries from a trial.
__attribute__((noinline)) uint64_t query_cycle(const swoole::String &value, const char *expected) {
    std::atomic_signal_fence(std::memory_order_seq_cst);
    static const char missing[] = {char(255), char(255), char(255)};
    return value.starts_with(expected, 32) + 2 * value.ends_with(expected + 4064, 32) +
           4 * value.equals(expected, 4096) + 8 * value.contains(missing, sizeof(missing)) +
           16 * value.contains(expected + 4000, 32);
}
__attribute__((noinline)) uint64_t dense_query_cycle(const swoole::String &value, bool long_needle) {
    std::atomic_signal_fence(std::memory_order_seq_cst);
    static const std::string needle = [] { std::string n(64, 'a'); n[32] = 'b'; return n; }();
    return long_needle ? value.contains(needle) : value.contains("aaaaaaab", 8);
}
__attribute__((noinline)) uint64_t split_cycle(swoole::String &value, bool missing) {
    value.offset = 0; uint64_t packets = 0, bytes = 0;
    const auto consumed = value.split(missing ? "\r\n?" : "\r\n", missing ? 3 : 2,
        [&](const char *, size_t length) { ++packets; bytes += length; return true; });
    if (missing) {
        if (consumed != 0 || packets || value.offset != 4093) throw std::runtime_error("Missing delimiter accounting");
        return value.offset;
    }
    if (consumed != 4096 || packets != 16 || value.offset != 4096) throw std::runtime_error("Split accounting");
    return bytes + packets;
}
int main(int argc, char **argv) {
    try {
        if (argc != 3) throw std::runtime_error("Expected workload iterations");
        SwooleG.std_allocator = {std::malloc, std::calloc, std::realloc, std::free}; SwooleG.pagesize = 4096;
        const std::string workload = argv[1]; const bool validate = std::string(argv[2]) == "check";
        const auto iterations = validate ? 1 : std::stoull(argv[2]);
        if (!iterations) throw std::runtime_error("Empty benchmark");
        char bytes[256]; for (size_t i = 0; i < sizeof(bytes); ++i) bytes[i] = static_cast<char>(i);
        swoole::String value(64);
        const bool query = workload == "query-4k";
        const bool dense = workload == "query-dense" || workload == "query-dense-long";
        const bool integer = workload == "append-integer" || workload == "append-integer-cold";
        const bool split = workload == "split-4k" || workload == "split-missing";
        if (dense) std::memset(bytes, 'a', sizeof(bytes));
        if (split) { bytes[254] = '\r'; bytes[255] = '\n'; }
        std::vector<char> expected;
        for (unsigned i = 0; i < 16; ++i) expected.insert(expected.end(), bytes, bytes + sizeof(bytes));
        if (integer) {
            expected.clear();
            for (int number : integers) { auto text = std::to_string(number); expected.insert(expected.end(), text.begin(), text.end()); }
        }
        if (workload == "write-reduce") { std::copy(bytes, bytes + 128, expected.begin() + 1000); expected.erase(expected.begin(), expected.begin() + 128); }
        if (query || dense || split) value.append(expected.data(), expected.size());
        auto cycle = [&]() -> uint64_t {
            if (query) return query_cycle(value, expected.data());
            if (dense) return dense_query_cycle(value, workload == "query-dense-long");
            if (split) return split_cycle(value, workload == "split-missing");
            value.clear();
            if (integer) { if (workload == "append-integer-cold") value = swoole::String(1); for (int number : integers) value.append(number); }
            else if (workload == "append-cold") { value = swoole::String(64); for (unsigned i = 0; i < 16; ++i) value.append(bytes, sizeof(bytes)); }
            else if (workload == "append-4k") for (unsigned i = 0; i < 16; ++i) value.append(bytes, sizeof(bytes));
            else if (workload == "repeat-4k") value.repeat(bytes, sizeof(bytes), 16);
            else if (workload == "write-reduce") { value.repeat(bytes, sizeof(bytes), 16); value.write(1000, bytes, 128); value.reduce(128); }
            else throw std::runtime_error("Unknown workload");
            return static_cast<uint8_t>(value.str[0]);
        };
        auto verify = [&]() {
            if (value.length != expected.size() || value.size < value.length || std::memcmp(value.str, expected.data(), expected.size()) != 0)
                throw std::runtime_error("Byte/accounting mismatch");
        };
        const auto observed = cycle();
        if ((dense && observed != 0) || (query && observed != 23) || (split && observed != (workload == "split-missing" ? 4093 : 4112))) throw std::runtime_error("Algorithm preflight mismatch");
        verify();
        if (validate) { std::cout << "{\"validation_only\":true,\"errors\":0}\n"; return 0; }
        for (unsigned i = 0; i < 100; ++i) cycle();
        uint64_t checksum = 0;
        const auto start = std::chrono::steady_clock::now();
        for (uint64_t i = 0; i < iterations; ++i) { checksum += cycle(); }
        const auto ns = std::chrono::duration_cast<std::chrono::nanoseconds>(std::chrono::steady_clock::now() - start).count();
        verify();
        const unsigned first = integer ? '0' : (dense ? 0 : (query ? 23 : (split ? (workload == "split-missing" ? 4093 : 4112) : (workload == "write-reduce" ? 128 : 0))));
        if (checksum != uint64_t(first) * iterations) throw std::runtime_error("Checksum mismatch");
        std::cout << "{\"iterations\":" << iterations << ",\"ns_per_cycle\":" << double(ns) / iterations << ",\"errors\":0}\n";
        return 0;
    } catch (const std::exception &error) { std::cerr << error.what() << '\n'; return 1; }
}
