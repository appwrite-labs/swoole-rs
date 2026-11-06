// SPDX-License-Identifier: Apache-2.0
// Compatibility derived from Swoole v6.2.4 src/core/base.cc (Swoole contributors).
// What stable Rust can't express of base.cc and misc.cc: the globals, C++ object
// lifetimes, the variadic fatal-error hook and the std::string/std::function APIs.
// Rust sequences clean, fork and the thread lifecycle (crates/corebase/src/lifecycle.rs).
#include "swoole.h"
#include "swoole_memory.h"
#include "swoole_string.h"
#include "swoole_signal.h"
#include "swoole_ssl.h"
#include "swoole_coroutine_api.h"
#include "swoole_rs_corebase.h"
#include "swoole_rs_utilities.h"

#include <cstdarg>

swoole::Global SwooleG = {};
thread_local swoole::ThreadGlobal SwooleTG = {};
thread_local char sw_error[SW_ERROR_MSG_SIZE];
std::mutex sw_thread_lock;

static_assert(offsetof(swoole::ThreadGlobal, main_thread) == 3 && offsetof(swoole::ThreadGlobal, buffer_stack) == 8 &&
                  offsetof(swoole::ThreadGlobal, async_threads) == 40,
              "the SwooleTG prefix crates/corebase/src/lifecycle.rs reads");
static_assert(sizeof(swoole::DataHead) == 40 && offsetof(swoole::DataHead, time) == 32, "Rust DataHead layout");

// The log macros Rust emits through as an upstream function (crates/corebase/src/log.rs).
static void corebase_log(int kind, int level, int code, const char *origin, const char *message) {
    switch (kind) {
    case 0: errno = code; swoole_log_as(origin, swoole_sys_warning("%s", message)); break;
    case 1: swoole_log_as(origin, swoole_warning("%s", message)); break;
    case 2: swoole_notice("%s", message); break;
    default: swoole_log_as(origin, swoole_error_log(level, code, "%s", message)); break;
    }
}
static const bool corebase_log_registered = (swr_v1_core_set_log_host(corebase_log), true);

static void fatal_error(int code, const char *format, ...) {
    va_list args;
    size_t length = sw_snprintf(sw_error, SW_ERROR_MSG_SIZE, "(ERROR %d): ", code);
    va_start(args, format);
    length += sw_vsnprintf(sw_error + length, SW_ERROR_MSG_SIZE - length, format, args);
    va_end(args);
    sw_logger()->put(SW_LOG_ERROR, sw_error, length);
    swoole_exit(1);
}

bool swoole_get_env(const char *name, int *value) {
    const char *e = getenv(name);
    if (!e) {
        return false;
    }
    const std::string input(e);
    if (!swr_v1_corebase_parse_stoi(input.c_str(), value)) {
        *value = std::stoi(input);  // throws, as upstream
    }
    return true;
}

// The C++ effects Rust's lifecycles run (crates/corebase/src/native.rs). One function, not a
// table: a sanitizer keeps every global alive, and a table would keep these leaves with it in
// the gates that link this file without the timer, event and signal code.
static uintptr_t effect(int effect, uintptr_t argument) {
    switch (effect) {
    case 0: return reinterpret_cast<uintptr_t>(&SwooleTG);
    case 1: SwooleG = {}; break;
    case 2:
        delete SwooleTG.buffer_stack;  // Rust allocates only when there is none
        SwooleTG.buffer_stack = argument ? new swoole::String(SW_STACK_BUFFER_SIZE) : nullptr;
        break;
    case 3: return reinterpret_cast<uintptr_t>(new swoole::GlobalMemory(SW_GLOBAL_MEMORY_PAGESIZE, true));
    case 4: delete reinterpret_cast<swoole::MemoryPool *>(argument); break;
    case 5: SwooleG.logger->close(); break;
    case 6: SwooleG.logger->reopen(); break;
    case 7: delete SwooleG.logger; break;
    case 8: swoole_signal_clear(); break;
    case 9: swoole_signal_block_all(); break;
    case 10: return swoole_timer_is_available();
    case 11: swoole_timer_free(); break;
    case 12: return swoole_event_is_available();
    case 13: swoole_event_free(); break;
    case 14: return swoole_coroutine_is_in();
    default: std::abort();
    }
    return 0;
}

void swoole_init() {
    swr_v1_core_set_native_host({swoole_get_env, effect});
    if (!swr_v1_core_init()) {
        return;
    }
    SwooleG = {};
    sw_memset_zero(sw_error, SW_ERROR_MSG_SIZE);
    swr_v1_core_config_allocator(SwooleG.config_handle());
    swr_v1_core_set_stdout(stdout);
    SwooleG.fatal_error = fatal_error;
    SwooleG.dns_resolvconf_path = SW_DNS_RESOLV_CONF;
    SwooleG.logger = new swoole::Logger();
    swoole_thread_init(true);
#ifdef SW_DEBUG
    sw_logger()->set_level(0);
    SwooleG.trace_flags = 0x7fffffff;
#else
    sw_logger()->set_level(SW_LOG_INFO);
#endif
    swr_v1_core_set_memory_pool(new swoole::GlobalMemory(SW_GLOBAL_MEMORY_PAGESIZE, true));
#ifndef HAVE_FUTEX
    // Without futex, waiters register in a Rust registry in shared memory.
    if (!swr_v1_utils_wait_registry_init(sw_mem_pool()->alloc(swr_v1_utils_wait_registry_size()))) {
        swoole_fatal_error(SW_ERROR_MALLOC_FAIL, "failed to allocate atomic wait registry");
    }
#endif
    if (!swr_v1_core_config_max_sockets(SwooleG.config_handle(), SW_SESSION_LIST_SIZE)) {
        swoole_sys_warning("getrlimit() failed");
    }
    SwooleG.task_tmpfile = SW_TASK_TMP_DIR "/" SW_TASK_TMP_FILE;
#ifdef HAVE_SIGNALFD
    swoole_signalfd_init();
    SwooleG.enable_signalfd = 1;
#endif
#ifdef __VERSION__
    swr_v1_core_bug_report_init(SWOOLE_BUG_REPORT, __VERSION__, swoole_ssl_get_version_message().c_str());
#else
    swr_v1_core_bug_report_init(SWOOLE_BUG_REPORT, nullptr, swoole_ssl_get_version_message().c_str());
#endif
}

bool swoole_set_task_tmpdir(const std::string &dir) {
    std::unique_lock<std::mutex> _lock(sw_thread_lock);
    (void) dir.at(0);  // throws on an empty path, as upstream
    auto status = swr_v1_core_set_task_tmpdir(
        SwooleG.config_handle(), reinterpret_cast<const uint8_t *>(dir.c_str()), dir.size());
    if (status < 0) {
        SwooleG.task_tmpfile.check(status);
    }
    return status == 0;
}

std::string swoole_get_task_tmpdir() {
    std::unique_lock<std::mutex> _lock(sw_thread_lock);
    auto path = swr_v1_core_config_task_path(SwooleG.config_handle());
    return {path.data, path.length};
}

pid_t swoole_fork_exec(const std::function<void(void)> &fn) {
    return swr_v1_core_fork_exec([](const void *f) { (*static_cast<const std::function<void(void)> *>(f))(); }, &fn);
}

void swoole_random_string(std::string &str, size_t len) {
    for (size_t i = 0; i < len; i++) {
        str.append(1, swr_v1_corebase_random_char());
    }
}
