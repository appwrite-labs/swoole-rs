// SPDX-License-Identifier: Apache-2.0
#include "swoole.h"
#include "swoole_rs_error.h"
#include <csignal>
#include <cstdlib>
#include <iostream>
#include <sys/wait.h>
#include <unistd.h>
extern "C" void swr_test_fail_allocation(size_t);
extern "C" size_t swr_test_allocation_countdown();
static void require(bool value) { if (!value) std::abort(); }
static void fail_insert(int code, size_t allocation) {
    swr_test_fail_allocation(allocation);
    require(swr_v1_error_ignore_global(code) == -2);
    require(swr_test_allocation_countdown() == 0);
    require(!swoole_is_ignored_error(code));
}
int main() {
    // The Rust set reports NOMEM (-2) and keeps its state; swoole_ignore_error, which
    // upstream made throw bad_alloc that nothing caught, aborts.
    fail_insert(1, 1); // Bucket allocation.
    fail_insert(1, 2); // Opaque handle after a successful bucket allocation.
    swoole_ignore_error(1);
    require(swoole_is_ignored_error(1));
    swr_test_fail_allocation(1);
    swoole_ignore_error(1); // Duplicate must not consume an allocation failure.
    require(swr_test_allocation_countdown() == 1);
    int failed = 0;
    for (int code = 2; code < 100; ++code) {
        if (swr_v1_error_ignore_global(code) == -2) { failed = code; break; }
    }
    require(failed > 1 && swr_test_allocation_countdown() == 0);
    require(!swoole_is_ignored_error(failed));
    for (int code = 1; code < failed; ++code) require(swoole_is_ignored_error(code));
    const pid_t child = fork();
    require(child >= 0);
    if (child == 0) {
        swr_test_fail_allocation(1);
        swoole_ignore_error(failed);
        _exit(0);
    }
    int status = 0;
    require(waitpid(child, &status, 0) == child);
    require(WIFSIGNALED(status) && WTERMSIG(status) == SIGABRT);
    swoole_ignore_error(failed);
    require(swoole_is_ignored_error(failed));
    std::cout << "{\"bucket_failure\":true,\"handle_failure\":true,\"duplicate_no_allocation\":true,"
                 "\"growth_failure_atomic\":true,\"successful_retry\":true,\"wrapper_aborts\":true}\n";
}
