// SPDX-License-Identifier: Apache-2.0
// Each child starts from the same empty process-global sets. Preparation uses --check.
#include "swoole.h"
#include <algorithm>
#include <array>
#include <atomic>
#include <chrono>
#include <cmath>
#include <cstdint>
#include <cstdlib>
#include <cstring>
#include <iostream>
#include <sys/resource.h>
#include <sys/wait.h>
#include <thread>
#include <unistd.h>
#include <vector>

void reference_swoole_ignore_error(int);
bool reference_swoole_is_ignored_error(int);
char reference_sw_error[1024] = {};
struct Sample {
    int64_t ns;
    uint64_t checksum;
    uint64_t peak_rss_bytes;
    double batch_mean_p50_ns, batch_mean_p95_ns, batch_mean_p99_ns;
};
struct Workload { const char *name; int occupancy; int operations; int kind; };
struct Trial { Workload workload; int trial; Sample upstream, rust; };

static void require(bool ok) { if (!ok) std::abort(); }

static int member_key(int index) {
    const int code = index + 1;
    return index & 1 ? -code : code;
}

static int miss_key(int index) { return INT32_MIN + 1 + index; }

static int batch_width(int operations) { return std::max(1, (operations + 127) / 128); }

static std::array<double, 3> latency_quantiles(std::vector<double> &samples) {
    if (samples.empty()) return {};
    std::sort(samples.begin(), samples.end());
    const auto at = [&](double fraction) {
        const auto rank = static_cast<size_t>(std::ceil(fraction * samples.size()));
        return samples[std::min(samples.size() - 1, rank ? rank - 1 : size_t{0})];
    };
    return {at(0.50), at(0.95), at(0.99)};
}

static Sample sample(bool rust, Workload work, bool timed, bool latency) {
    int channel[2];
    require(pipe(channel) == 0);
    const pid_t child = fork();
    require(child >= 0);
    if (child == 0) {
        close(channel[0]);
        const auto insert = rust ? swoole_ignore_error : reference_swoole_ignore_error;
        const auto contains = rust ? swoole_is_ignored_error : reference_swoole_is_ignored_error;
        for (int index = 0; index < work.occupancy; ++index) insert(member_key(index));
        for (int index = 0; index < work.occupancy; ++index) {
            require(contains(member_key(index)));
            require(!contains(miss_key(index)));
        }
        uint64_t checksum = 0;
        int64_t elapsed = 0;
        std::vector<double> batch_means;
        if (latency) batch_means.reserve(128 * (work.kind == 4 ? 4 : 1));
        if (work.kind == 4) {
            constexpr int thread_count = 4;
            require(work.operations % thread_count == 0);
            const int operations_per_thread = work.operations / thread_count;
            std::atomic<int> ready{0};
            std::atomic<bool> start{false};
            struct alignas(64) Checksum { uint64_t value = 0; };
            std::array<Checksum, thread_count> sums{};
            std::array<std::vector<double>, thread_count> thread_batch_means;
            if (latency) for (auto &values : thread_batch_means) values.reserve(128);
            std::vector<std::thread> readers;
            readers.reserve(thread_count);
            for (int thread_index = 0; thread_index < thread_count; ++thread_index) {
                readers.emplace_back([&, thread_index] {
                    ready.fetch_add(1, std::memory_order_release);
                    while (!start.load(std::memory_order_acquire)) std::this_thread::yield();
                    uint64_t local = 0;
                    if (latency) {
                        const int width = batch_width(operations_per_thread);
                        for (int offset = 0; offset < operations_per_thread; offset += width) {
                            const int end = std::min(operations_per_thread, offset + width);
                            const auto batch_begin = timed ? std::chrono::steady_clock::now()
                                                           : std::chrono::steady_clock::time_point{};
                            for (int operation = offset; operation < end; ++operation) {
                                const int index = thread_index * operations_per_thread + operation;
                                local += contains(operation & 1 ? miss_key(index % work.occupancy)
                                                                : member_key(index % work.occupancy));
                            }
                            const auto ns = timed ? std::chrono::duration_cast<std::chrono::nanoseconds>(
                                std::chrono::steady_clock::now() - batch_begin).count() : 0;
                            thread_batch_means[thread_index].push_back(double(ns) / (end - offset));
                        }
                    } else {
                        for (int operation = 0; operation < operations_per_thread; ++operation) {
                            const int index = thread_index * operations_per_thread + operation;
                            local += contains(operation & 1 ? miss_key(index % work.occupancy)
                                                            : member_key(index % work.occupancy));
                        }
                    }
                    sums[thread_index].value = local;
                });
            }
            while (ready.load(std::memory_order_acquire) != thread_count) std::this_thread::yield();
            const auto begin = timed ? std::chrono::steady_clock::now()
                                     : std::chrono::steady_clock::time_point{};
            start.store(true, std::memory_order_release);
            for (auto &reader : readers) reader.join();
            if (timed) elapsed = std::chrono::duration_cast<std::chrono::nanoseconds>(
                std::chrono::steady_clock::now() - begin).count();
            for (const auto &sum : sums) checksum += sum.value;
            if (latency) for (auto &values : thread_batch_means)
                batch_means.insert(batch_means.end(), values.begin(), values.end());
        } else {
            auto operation = [&](int index) {
                const int key = work.kind == 3 ? member_key(index)
                                               : member_key(index % work.occupancy);
                if (work.kind == 2 || work.kind == 3) insert(key);
                else checksum += contains(work.kind == 1 ? miss_key(index % work.occupancy) : key);
            };
            const auto begin = timed ? std::chrono::steady_clock::now()
                                     : std::chrono::steady_clock::time_point{};
            if (latency) {
                for (int offset = 0; offset < work.operations; offset += batch_width(work.operations)) {
                    const int end = std::min(work.operations, offset + batch_width(work.operations));
                    const auto batch_begin = timed ? std::chrono::steady_clock::now()
                                                   : std::chrono::steady_clock::time_point{};
                    for (int index = offset; index < end; ++index) operation(index);
                    const auto ns = timed ? std::chrono::duration_cast<std::chrono::nanoseconds>(
                        std::chrono::steady_clock::now() - batch_begin).count() : 0;
                    batch_means.push_back(double(ns) / (end - offset));
                }
            } else {
                for (int index = 0; index < work.operations; ++index) operation(index);
            }
            if (timed) elapsed = std::chrono::duration_cast<std::chrono::nanoseconds>(
                std::chrono::steady_clock::now() - begin).count();
        }
        if (work.kind == 2 || work.kind == 3) {
            // Validate all inserted values after the timer, including duplicate semantics.
            for (int index = 0; index < (work.kind == 3 ? work.operations : work.occupancy); ++index)
                require(contains(member_key(index)));
            checksum = work.operations;
        }
        const uint64_t expected = work.kind == 1 ? 0
            : work.kind == 4 ? uint64_t(work.operations / 2) : uint64_t(work.operations);
        require(checksum == expected);
        rusage usage{};
        require(getrusage(RUSAGE_SELF, &usage) == 0);
        uint64_t peak = usage.ru_maxrss;
#ifndef __MACH__
        peak *= 1024;
#endif
        const auto quantiles = latency_quantiles(batch_means);
        const Sample result{elapsed, checksum, peak, quantiles[0], quantiles[1], quantiles[2]};
        require(write(channel[1], &result, sizeof(result)) == sizeof(result));
        close(channel[1]);
        std::exit(0); // Exercise normal native/Rust global destruction outside the timed region.
    }
    close(channel[1]);
    Sample result{};
    const auto received = read(channel[0], &result, sizeof(result));
    close(channel[0]);
    int status = 0;
    require(waitpid(child, &status, 0) == child);
    require(WIFEXITED(status) && WEXITSTATUS(status) == 0 && received == sizeof(result));
    require(timed ? result.ns > 0 : result.ns == 0);
    return result;
}

int main(int argc, char **argv) {
    const bool check = argc == 2 && std::strcmp(argv[1], "--check") == 0;
    const bool check_latency = argc == 2 && std::strcmp(argv[1], "--check-latency") == 0;
    const bool timed_latency = argc == 2 && std::strcmp(argv[1], "--latency") == 0;
    require(argc == 1 || check || check_latency || timed_latency);
    const bool timed = argc == 1 || timed_latency;
    const bool latency = check_latency || timed_latency;
    const Workload workloads[] = {
        {"hit-8", 8, 500000, 0}, {"miss-8", 8, 500000, 1}, {"duplicate-8", 8, 500000, 2},
        {"hit-4096", 4096, 500000, 0}, {"miss-4096", 4096, 500000, 1},
        {"duplicate-4096", 4096, 500000, 2}, {"insert-4096", 0, 4096, 3},
        {"insert-65536", 0, 65536, 3}, {"concurrent-read-4096", 4096, 500000, 4}
    };
    std::vector<Trial> results;
    for (int trial = 0; trial < (timed ? 7 : 1); ++trial) {
        for (const auto work : workloads) {
            Sample upstream, rust;
            if (trial % 2) { rust = sample(true, work, timed, latency); upstream = sample(false, work, timed, latency); }
            else { upstream = sample(false, work, timed, latency); rust = sample(true, work, timed, latency); }
            require(upstream.checksum == rust.checksum);
            results.push_back({work, trial, upstream, rust});
        }
    }
    // No stdout before child exits: normal child shutdown cannot duplicate buffered JSON.
    std::cout << "{\"errors\":0,\"timed\":" << (timed ? "true" : "false")
              << ",\"latency_mode\":" << (latency ? "true" : "false") << ",\"trials\":[";
    bool first = true;
    for (const auto &row : results) {
        if (!first) std::cout << ',';
        first = false;
        std::cout << "{\"workload\":\"" << row.workload.name << "\",\"trial\":" << row.trial
                  << ",\"operations\":" << row.workload.operations << ",\"checksum\":" << row.rust.checksum
                  << ",\"upstream_ns\":" << row.upstream.ns << ",\"rust_ns\":" << row.rust.ns
                  << ",\"upstream_peak_rss_bytes\":" << row.upstream.peak_rss_bytes
                  << ",\"rust_peak_rss_bytes\":" << row.rust.peak_rss_bytes;
        if (latency) {
            std::cout << ",\"upstream_batch_mean_ns_p50\":" << row.upstream.batch_mean_p50_ns
                      << ",\"upstream_batch_mean_ns_p95\":" << row.upstream.batch_mean_p95_ns
                      << ",\"upstream_batch_mean_ns_p99\":" << row.upstream.batch_mean_p99_ns
                      << ",\"rust_batch_mean_ns_p50\":" << row.rust.batch_mean_p50_ns
                      << ",\"rust_batch_mean_ns_p95\":" << row.rust.batch_mean_p95_ns
                      << ",\"rust_batch_mean_ns_p99\":" << row.rust.batch_mean_p99_ns;
        }
        std::cout << '}';
    }
    std::cout << "]}\n";
}
