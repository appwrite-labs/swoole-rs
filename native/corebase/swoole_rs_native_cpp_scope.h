/* SPDX-License-Identifier: Apache-2.0 */
#pragma once
#include <cstdlib>
#include <exception>
#if defined(__GLIBCXX__)
#include <cxxabi.h>
#endif

namespace swoole_rs {
// Pure C++ in SDK-only consumers. PHP entries explicitly supply these callbacks;
// no Zend symbol is referenced by this header or by its default runtime.
struct NativeCppRuntime {
    void *(*context)() = nullptr;
    int (*pending)() = nullptr;
    void (*mark)() = nullptr;
    void (*clear)() = nullptr;
};

// Native entry custody, outside the Rust entry it protects. Construct before
// its native jump target; keep that target live while the scope is registered.
// take() acknowledges only this scope's exception after its Rust entry returns.
// Re-throw there: a legitimate native catch may recover before an outer Rust
// frame, or an outer native effect must capture before that frame is crossed.
class NativeCppScope {
    NativeCppRuntime runtime_;
    void *context_;
    NativeCppScope *previous_;
    std::exception_ptr exception_;
    bool linked_ = true;
    static NativeCppScope *&head() {
        static thread_local NativeCppScope *scope = nullptr;
        return scope;
    }
    bool compatible(const NativeCppRuntime &runtime) const {
        return runtime_.context == runtime.context && runtime_.pending == runtime.pending &&
            runtime_.mark == runtime.mark && runtime_.clear == runtime.clear;
    }
  public:
    explicit NativeCppScope(NativeCppRuntime runtime = {})
        : runtime_(runtime), context_(runtime.context ? runtime.context() : nullptr), previous_(head()) {
        head() = this;
    }
    NativeCppScope(const NativeCppScope &) = delete;
    NativeCppScope &operator=(const NativeCppScope &) = delete;
    ~NativeCppScope() { leave(); }
    void leave() {
        if (exception_) std::abort();
        if (!linked_) return;
        auto **slot = &head();
        while (*slot && *slot != this) slot = &(*slot)->previous_;
        if (!*slot) std::abort();
        *slot = previous_;
        linked_ = false;
    }
    static NativeCppScope *current(NativeCppRuntime runtime = {}) {
        void *context = runtime.context ? runtime.context() : nullptr;
        for (auto *scope = head(); scope; scope = scope->previous_) {
            if (scope->compatible(runtime) && scope->context_ == context) return scope;
        }
        return nullptr;
    }
    template <typename Function> bool capture(Function &&function) {
        if ((runtime_.context && runtime_.context() != context_) || !linked_) std::abort();
        if (exception_ || (runtime_.pending && runtime_.pending())) return false;
        try {
            function();
            return true;
#if defined(__GLIBCXX__)
        } catch (const __cxxabiv1::__forced_unwind &) {
            throw;
#endif
        } catch (...) {
            exception_ = std::current_exception();
            if (!exception_) throw;
            if (runtime_.mark) runtime_.mark();
            return false;
        }
    }
    bool has_exception() const { return bool(exception_); }
    std::exception_ptr take() {
        if (!exception_) return {};
        if ((runtime_.context && runtime_.context() != context_) ||
            (runtime_.pending && runtime_.pending() != 3)) std::abort();
        auto exception = exception_;
        exception_ = nullptr;
        if (runtime_.clear) runtime_.clear();
        return exception;
    }
};
}  // namespace swoole_rs
