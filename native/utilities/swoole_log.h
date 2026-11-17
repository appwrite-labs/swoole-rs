/* SPDX-License-Identifier: Apache-2.0 */
/* Swoole's include/swoole_log.h, owned here. The logger and the log macros are Rust's
 * (crates/corebase/src/utilities/src/logger.rs, crates/corebase/src/log.rs); what C++ still expands is
 * the fatal-error forwarder and, compiled off, the curl hooks' trace log. */
#pragma once

// Explicit, because crates/corebase's tests compare them with its LogLevel.
enum {
    SW_LOG_DEBUG = 0,
    SW_LOG_TRACE = 1,
    SW_LOG_INFO = 2,
    SW_LOG_NOTICE = 3,
    SW_LOG_WARNING = 4,
    SW_LOG_ERROR = 5,
    SW_LOG_NONE = 6,
};

#define swoole_fatal_error(code, str, ...) swr_v1_core_fatal_error()(code, str, ##__VA_ARGS__)

// No build defines SW_LOG_TRACE_OPEN.
#define swoole_trace_log(what, str, ...)
