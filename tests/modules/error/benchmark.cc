// SPDX-License-Identifier: Apache-2.0
// Run only under an explicit exclusive lease, after the differential gate.
#include "swoole.h"
#include <chrono>
#include <cstdint>
#include <cstdlib>
#include <iostream>

const char *reference_swoole_strerror(int);
char reference_sw_error[1024] = {};

struct Sample {
    int64_t nanoseconds;
    uint64_t checksum;
};

static Sample sample(const char *(*lookup)(int), int workload) {
    int codes[] = {/* KNOWN_CASES */};
    for (int &code : codes) {
        if (workload == 1) code = SW_ERROR_MALLOC_FAIL;
        else if (workload == 2) code %= 128;
        else if (workload == 3) code += 20000;
    }
    uint64_t checksum = 0;
    const auto start = std::chrono::steady_clock::now();
    for (int repeat = 0; repeat < 20000; ++repeat)
        for (int code : codes) {
            const char *message = lookup(code);
            if (!message) std::abort();
            checksum += static_cast<unsigned char>(message[0]);
        }
    return {std::chrono::duration_cast<std::chrono::nanoseconds>(
                std::chrono::steady_clock::now() - start).count(), checksum};
}

int main() {
    std::cout << "{\"calls_per_sample\":2600000,\"trials\":[";
    const char *names[] = {"all-known", "hot", "system", "unknown"};
    for (int trial = 0; trial < 7; ++trial) {
        for (int workload = 0; workload < 4; ++workload) {
            Sample upstream, rust;
            if (trial % 2) {
                rust = sample(swoole_strerror, workload);
                upstream = sample(reference_swoole_strerror, workload);
            } else {
                upstream = sample(reference_swoole_strerror, workload);
                rust = sample(swoole_strerror, workload);
            }
            if (upstream.checksum != rust.checksum) std::abort();
            if (trial || workload) std::cout << ',';
            std::cout << "{\"trial\":" << trial << ",\"workload\":\"" << names[workload]
                      << "\",\"upstream_ns\":" << upstream.nanoseconds << ",\"rust_ns\":"
                      << rust.nanoseconds << ",\"checksum\":" << rust.checksum << '}';
        }
    }
    std::cout << "],\"errors\":0}\n";
}
