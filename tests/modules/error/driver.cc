// SPDX-License-Identifier: Apache-2.0
#include "swoole.h"
#include "swoole_rs_error.h"
#include "swoole_rs_error_resolver.h"
#include <array>
#include <atomic>
#include <climits>
#include <cstdlib>
#include <iostream>
#include <string>
#include <thread>
#include <vector>
#include <csignal>
#include <sys/wait.h>
#include <unistd.h>

const char *reference_swoole_strerror(int);
void reference_swoole_ignore_error(int);
bool reference_swoole_is_ignored_error(int);
void reference_swoole_clear_last_error_msg();
const char *reference_swoole_get_last_error_msg();
// The reference error.cc's sw_error; the hybrid's is crates/error's per-thread buffer.
char reference_sw_error[1024] = {};

static void require(bool value) {
    if (!value) std::abort();
}

static int formatted = 0;
static int last_formatted = 0;
extern "C" const char *test_formatter(int32_t code) {
    ++formatted;
    last_formatted = code;
    return "native formatter sentinel";
}

int main() {
    const int known[] = {/* KNOWN_CASES */};
    const int enums[] = {/* ALL_ENUMS */};
    size_t compared = 0;
    const auto check = [&](int code) {
        bool mapped = false;
        for (int candidate : known) mapped |= candidate == code;
        const char *rust = swr_v1_error_message(code);
        require((rust != nullptr) == mapped);
        const std::string reference = reference_swoole_strerror(code);
        require(reference == swoole_strerror(code));
        if (mapped) require(reference == rust && rust == swr_v1_error_message(code));
        ++compared;
    };
    for (int code = -128; code <= SW_ERROR_END + 128; ++code) check(code);
    for (int code : enums) check(code); // Compiler evaluates the pinned enum, not a Python renumbering.
    for (int code : {INT_MIN, INT_MIN + 1, -65536, INT_MAX - 1, INT_MAX}) check(code);
    for (int code : known) {
        require(swr_v1_error_resolve(code, test_formatter) == swr_v1_error_message(code));
        require(swr_v1_error_resolve(code, nullptr) == swr_v1_error_message(code));
        require(swr_v1_error_strerror(code, nullptr) == swr_v1_error_message(code));
    }
    require(formatted == 0);
    for (int code : {INT_MIN, INT_MAX}) {
        require(std::string(swr_v1_error_resolve(code, test_formatter)) == "native formatter sentinel");
        require(last_formatted == code);
        require(swr_v1_error_resolve(code, nullptr) == nullptr);
    }
    require(formatted == 2);
    for (int code : {INT_MIN, 499}) {
        require(std::string(swr_v1_error_strerror(code, test_formatter)) == "native formatter sentinel");
        require(last_formatted == code);
        require(swr_v1_error_strerror(code, nullptr) == nullptr);
    }
    require(formatted == 4);
    require(std::string(swr_v1_error_strerror(500, test_formatter)).find("500") != std::string::npos);
    require(formatted == 4);
    reference_swoole_strerror(500);

    // Unknown storage, NUL placement and trailing bytes match the original buffer.
    const char *unknown = nullptr;
    for (int code : {INT_MAX, 500, 599, INT_MAX, 551}) {
        const char *expected = reference_swoole_strerror(code);
        const char *actual = swoole_strerror(code);
        require(std::memcmp(expected, actual, 32) == 0);
        require(unknown == nullptr || unknown == actual);
        unknown = actual;
    }
    // Upstream's throw reached no handler in the extension, so it terminated; Rust aborts.
    const pid_t thrower = fork();
    require(thrower >= 0);
    if (thrower == 0) {
        swoole_throw_error(SW_ERROR_BAD_PORT);
        _exit(0);
    }
    int thrower_status = 0;
    require(waitpid(thrower, &thrower_status, 0) == thrower);
    require(WIFSIGNALED(thrower_status) && WTERMSIG(thrower_status) == SIGABRT);
    for (int code : {0, int(SW_ERROR_MALLOC_FAIL), INT_MAX}) {
        require(!swoole_is_ignored_error(code) && !reference_swoole_is_ignored_error(code));
        swoole_ignore_error(code);
        reference_swoole_ignore_error(code);
        require(swoole_is_ignored_error(code) && reference_swoole_is_ignored_error(code));
    }
    // Exercise the real native wrappers over arbitrary signed values and duplicates.
    uint32_t random = 123;
    for (int index = 0; index < 20000; ++index) {
        random = random * 1664525u + 1013904223u;
        const int code = static_cast<int32_t>(random);
        require(swoole_is_ignored_error(code) == reference_swoole_is_ignored_error(code));
        if (index % 3) {
            swoole_ignore_error(code);
            reference_swoole_ignore_error(code);
            swoole_ignore_error(code);
            require(swoole_is_ignored_error(code) && reference_swoole_is_ignored_error(code));
        }
    }
    const int child_code = -777777;
    require(!swoole_is_ignored_error(child_code));
    const pid_t child = fork();
    require(child >= 0);
    if (child == 0) {
        swoole_ignore_error(child_code);
        reference_swoole_ignore_error(child_code);
        require(swoole_is_ignored_error(child_code) && reference_swoole_is_ignored_error(child_code));
        std::exit(0); // Run native/global Rust-handle destructors in the child too.
    }
    int child_status = 0;
    require(waitpid(child, &child_status, 0) == child);
    require(WIFEXITED(child_status) && WEXITSTATUS(child_status) == 0);
    require(!swoole_is_ignored_error(child_code) && !reference_swoole_is_ignored_error(child_code));
    std::strcpy(sw_error, "last error sentinel");
    std::strcpy(reference_sw_error, "last error sentinel");
    require(swoole_get_last_error_msg() == sw_error);
    require(std::string(swoole_get_last_error_msg()) == reference_swoole_get_last_error_msg());
    swoole_clear_last_error_msg();
    reference_swoole_clear_last_error_msg();
    require(sw_error[0] == '\0' && reference_sw_error[0] == '\0');
    // Each thread has its own buffer, as upstream's thread_local.
    std::thread([] { require(sw_error[0] == '\0'); std::strcpy(sw_error, "other thread"); }).join();
    require(sw_error[0] == '\0');

    std::vector<const char *> pointers;
    std::vector<std::string> messages;
    for (int code : known) {
        pointers.push_back(swr_v1_error_message(code));
        messages.emplace_back(pointers.back());
    }
    std::atomic<bool> valid{true};
    std::vector<std::thread> threads;
    for (int thread = 0; thread < 8; ++thread) threads.emplace_back([&] {
        for (int repeat = 0; repeat < 1000; ++repeat)
            for (size_t index = 0; index < pointers.size(); ++index) {
                const char *message = swr_v1_error_message(known[index]);
                if (message != pointers[index] || messages[index] != message) valid.store(false);
                if (swoole_is_ignored_error(known[index]) != reference_swoole_is_ignored_error(known[index]))
                    valid.store(false);
            }
    });
    for (auto &thread : threads) thread.join();
    require(valid.load());
    std::cout << "{\"known_cases\":" << pointers.size() << ",\"numeric_checks\":" << compared
              << ",\"concurrent_calls\":" << 8 * 1000 * pointers.size()
              << ",\"ignored_state_operations\":20000,\"fork_isolation\":true,\"native_fallback_and_state\":true}\n";
}
