/* SPDX-License-Identifier: Apache-2.0 */
#pragma once
#include "swoole_rs.h"
#include <cstdlib>
#include <cstring>
#include <exception>
#if defined(__GLIBCXX__)
#include <cxxabi.h>
#endif

namespace swoole_rs {
// Ordinary C++ exception custody only. Zend bailout/custom thread exit require
// their own native transport; pthread forced unwind must never be captured here.
class NativeException {
    std::exception_ptr pending_;
  public:
    template <typename Function> ServerReply capture(Function &&function) {
        try {
            return {function(), 0};
#if defined(__GLIBCXX__)
        } catch (const __cxxabiv1::__forced_unwind &) {
            throw;
#endif
        } catch (...) {
            pending_ = std::current_exception();
            if (!pending_) throw;
            return {0, 1};
        }
    }
    int64_t finish(ServerReply reply) const {
        if (reply.fault) {
            if (reply.fault != 1 || !pending_) std::abort();
            std::rethrow_exception(pending_);
        }
        int64_t value;
        static_assert(sizeof(value) == sizeof(reply.value), "native reply scalar width");
        std::memcpy(&value, &reply.value, sizeof(value));
        return value;
    }
};
}  // namespace swoole_rs
