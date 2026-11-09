// SPDX-License-Identifier: Apache-2.0
// Compatibility derived from Swoole v6.2.4 src/core/base.cc (Swoole contributors).
// What stays C++ of base.cc: the objects Rust's lifecycles create and delete (the
// memory pool, whose vtable admin.cc's dynamic_cast needs; the thread's String), the
// build's compile-time strings, the variadic fatal-error hook, and the std::string,
// std::function and throwing APIs. Rust owns SwooleG, SwooleTG, the init, clean, fork
// and thread sequences (crates/corebase), the process logger (crates/process) and the
// log macros Rust's logs run (crates/utilities); swoole_init here only registers them
// before Rust's body runs.
#include "swoole.h"
#include "swoole_memory.h"
#include "swoole_string.h"
#include "swoole_signal.h"
#include "swoole_timer.h"
#include "swoole_ssl.h"
#include "swoole_coroutine_api.h"
#include "swoole_rs_corebase.h"
#include "swoole_rs_utilities.h"

#include <cstdarg>

static_assert(sizeof(swoole::DataHead) == 40 && offsetof(swoole::DataHead, time) == 32, "Rust DataHead layout");

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
    case 2:
        delete SwooleTG.buffer_stack;  // Rust allocates only when there is none
        SwooleTG.buffer_stack = argument ? new swoole::String(SW_STACK_BUFFER_SIZE) : nullptr;
        break;
    case 3: return reinterpret_cast<uintptr_t>(new swoole::GlobalMemory(SW_GLOBAL_MEMORY_PAGESIZE, true));
    case 4: delete reinterpret_cast<swoole::MemoryPool *>(argument); break;
    case 8: swoole_signal_clear(); break;
    case 10: return swoole_timer_is_available();
    case 11: swoole_timer_free(); break;
    case 12: return swoole_event_is_available();
    case 13: swoole_event_free(); break;
    case 14: return swoole_coroutine_is_in();
    case 15:
        sw_memset_zero(sw_error, SW_ERROR_MSG_SIZE);
        swr_v1_core_set_stdout(stdout);
        swr_v1_core_set_fatal_error(reinterpret_cast<void *>(fatal_error));
        break;
    case 16:  // once Rust has made the logger
#ifdef SW_DEBUG
        sw_logger()->set_level(0);
        swr_v1_core_set_trace_flags(0x7fffffff);
#endif
        swr_v1_core_set_memory_pool(new swoole::GlobalMemory(SW_GLOBAL_MEMORY_PAGESIZE, true));
#ifndef HAVE_FUTEX
        // Without futex, waiters register in a Rust registry in shared memory.
        if (!swr_v1_utils_wait_registry_init(sw_mem_pool()->alloc(swr_v1_utils_wait_registry_size()))) {
            swoole_fatal_error(SW_ERROR_MALLOC_FAIL, "failed to allocate atomic wait registry");
        }
#endif
#ifdef HAVE_SIGNALFD
        swoole_signalfd_init();
        SwooleG.enable_signalfd = 1;
#endif
#ifdef __VERSION__
        swr_v1_core_bug_report_init(SWOOLE_BUG_REPORT, __VERSION__, "OPENSSL_VERSION: " OPENSSL_VERSION_TEXT "\n");
#else
        swr_v1_core_bug_report_init(SWOOLE_BUG_REPORT, nullptr, "OPENSSL_VERSION: " OPENSSL_VERSION_TEXT "\n");
#endif
        break;
    default: std::abort();
    }
    return 0;
}

// Rust's swoole_init (crates/corebase/src/lifecycle.rs), once the effects are registered. Only
// here, not as this file loads, so the gates that never call it link without those leaves.
void swoole_init() {
    swr_v1_core_set_log_host(swr_v1_utils_log);
    swr_v1_core_set_native_host({swoole_get_env, effect, swr_v1_process_logger});
    swr_v1_core_swoole_init();
}

bool swoole_set_task_tmpdir(const std::string &dir) {
    (void) dir.at(0);  // throws on an empty path, as upstream
    return swr_v1_core_set_task_tmpdir(dir.c_str());
}

pid_t swoole_fork_exec(const std::function<void(void)> &fn) {
    return swr_v1_core_fork_exec([](const void *f) { (*static_cast<const std::function<void(void)> *>(f))(); }, &fn);
}
