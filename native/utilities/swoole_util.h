/* SPDX-License-Identifier: Apache-2.0 */
/* Swoole's include/swoole_util.h, owned here: ON_SCOPE_EXIT and swoole::time.
 * Upstream's file is deleted, so `#include "swoole_util.h"` finds this copy on the Rust prefix's include path. */
#pragma once

#include "swoole_rs_utilities.h"

#include <cstdio>
#include <cstdarg>
#include <cassert>
#include <ctime>

#include <string>
#include <memory>
#include <chrono>
#include <set>
#include <vector>
#include <stack>
#include <thread>
#include <type_traits>
#include <algorithm>

namespace swoole {
// Keep parameter 'steady' as false for backward compatibility.
template <typename T>
static inline long time(bool steady = false) {
    if (sw_likely(steady)) {
        auto now = std::chrono::steady_clock::now();
        return std::chrono::duration_cast<T>(now.time_since_epoch()).count();
    } else {
        auto now = std::chrono::system_clock::now();
        return std::chrono::duration_cast<T>(now.time_since_epoch()).count();
    }
}

static inline long get_timezone() {
    return swr_v1_utils_timezone();
}

template <typename Fun>
class ScopeGuard {
  public:
    explicit ScopeGuard(Fun &&f) : _fun(std::forward<Fun>(f)), _active(true) {}

    ~ScopeGuard() {
        if (_active) {
            _fun();
        }
    }

    void dismiss() {
        _active = false;
    }

    ScopeGuard() = delete;
    ScopeGuard(const ScopeGuard &) = delete;
    ScopeGuard &operator=(const ScopeGuard &) = delete;

    ScopeGuard(ScopeGuard &&rhs) noexcept : _fun(std::move(rhs._fun)), _active(rhs._active) {
        rhs.dismiss();
    }

  private:
    Fun _fun;
    bool _active;
};

namespace detail {
enum class ScopeGuardOnExit {};

template <typename Fun>
inline ScopeGuard<Fun> operator+(ScopeGuardOnExit, Fun &&fn) {
    return ScopeGuard<Fun>(std::forward<Fun>(fn));
}
}  // namespace detail

#define __SCOPE_GUARD_CONCATENATE_IMPL(s1, s2) s1##s2
#define __SCOPE_GUARD_CONCATENATE(s1, s2) __SCOPE_GUARD_CONCATENATE_IMPL(s1, s2)

/**
 * Call the specified function when exiting the scope, similar to Golang's defer function.
 * After using this helper macro,
 * it is not necessary to manually release resources before the return statement of the failed branch.
 */
#define ON_SCOPE_EXIT                                                                                                  \
    auto __SCOPE_GUARD_CONCATENATE(ext_exitBlock_, __LINE__) = swoole::detail::ScopeGuardOnExit() + [&]()
}  // namespace swoole
