/* SPDX-License-Identifier: Apache-2.0 */
/* Swoole's include/swoole_thread.h, owned here: swoole::Thread, a C++ handle over the
 * Rust-owned thread record (swoole_rs_corebase_thread.h). Starting a thread runs a C++
 * callable, which stable Rust can't name; everything else is a Rust call. */
#pragma once

#include "swoole_rs_corebase_thread.h"
#include <memory>
#include <system_error>
#include <thread>
#include <tuple>

long swoole_thread_get_native_id();

namespace swoole {
class Thread {
    mutable CoreThread state_{};

    static void check(int error, const char *operation) {
        if (!error) return;
#if defined(_LIBCPP_VERSION)
        std::__throw_system_error(error, operation);
#else
        (void) operation;
        std::__throw_system_error(error);
#endif
    }
    template<typename Callable> struct Task {
        Callable callable;
        explicit Task(const Callable &fn) : callable(fn) {}
        static void *run(void *context) {
            std::unique_ptr<Task> task(static_cast<Task *>(context));
            std::move(task->callable)();
            return nullptr;
        }
    };

  public:
    Thread() noexcept = default;
    Thread(const Thread &) = delete;
    Thread &operator=(const Thread &) = delete;
    Thread(Thread &&source) noexcept {
        (void) swr_v1_core_thread_move(&state_, &source.state_);
    }
    Thread &operator=(Thread &&source) noexcept {
        if (!swr_v1_core_thread_move(&state_, &source.state_)) std::terminate();
        return *this;
    }
    ~Thread() {
        if (joinable()) std::terminate();
    }
    bool is_alive() const { return swr_v1_core_thread_alive(&state_); }
    bool joinable() const { return swr_v1_core_thread_joinable(&state_); }
    void join() {
        int error = swr_v1_core_thread_join_check(&state_);
        if (!error) error = pthread_join(swr_v1_core_thread_id(&state_), nullptr);
        check(swr_v1_core_thread_join_complete(&state_, error), "thread::join failed");
    }
    void detach() { check(swr_v1_core_thread_detach(&state_), "thread::detach failed"); }
    int get_exit_status() const { return swr_v1_core_thread_status(&state_); }
    pthread_t get_id() {
        return swr_v1_core_thread_id(&state_);
    }
    template<typename Callable> void start(Callable fn) {
#if defined(_LIBCPP_VERSION)
        std::unique_ptr<std::__thread_struct> tls(new std::__thread_struct);
        using NativeTask = std::tuple<std::unique_ptr<std::__thread_struct>, Callable>;
        std::unique_ptr<NativeTask> task(new NativeTask(std::move(tls), fn));
        const auto entry = &std::__thread_proxy<NativeTask>;
#else
        std::unique_ptr<Task<Callable>> task(new Task<Callable>(fn));
        const auto entry = &Task<Callable>::run;
#endif
        const bool occupied = joinable();
        const int error = swr_v1_core_thread_start(&state_, entry, task.get());
        if (!error) task.release();
        check(error, "thread constructor failed");
        if (occupied) std::terminate();
    }
    void enter() { swr_v1_core_thread_enter(&state_); }
    void exit(const int status) { swr_v1_core_thread_exit(&state_, status); }
};
}  // namespace swoole
