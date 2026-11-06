// SPDX-License-Identifier: Apache-2.0
// Compatibility derived from Swoole v6.2.4 src/core/error.cc (Swoole contributors).
// What stable Rust can't express of error.cc: C++ exceptions and the thread_local sw_error.
// swoole_strerror and swoole_is_ignored_error are Rust (crates/error).
#include "swoole.h"

#include <new>

swoole::Exception::Exception(const int code) noexcept : code(code) {
    msg = swoole_strerror(code);
}

void swoole_throw_error(int code) {
    throw swoole::Exception(code);
}

void swoole_ignore_error(int code) {
    if (swr_v1_error_ignore_global(code) != 0) {
        throw std::bad_alloc();
    }
}

void swoole_clear_last_error_msg() {
    sw_error[0] = '\0';
}

const char *swoole_get_last_error_msg() {
    return sw_error;
}
