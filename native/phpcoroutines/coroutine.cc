// SPDX-License-Identifier: Apache-2.0
// Compatibility derived from Swoole v6.2.4 ext-src/swoole_coroutine.cc (Swoole contributors).
// What crates/phpcoroutines needs of C++ that stable Rust can't express: a
// Coroutine's context and std::function members (timer, defer, bailout and the
// user exit condition), and the C++ objects it reads
// (Server, Reactor, AsyncThreads, io_uring) and the build's #ifdefs.
#include "swoole_rs_phphost.h"
#include "swoole_server.h"
#include "swoole_async.h"
#include "swoole_rs_uring.h"

using swoole::Coroutine;
using swoole::Reactor;
using swoole::TimerNode;

// crates/phpcoroutines reads a Coroutine's Rust state (co_) at its address.
#pragma GCC diagnostic push
#pragma GCC diagnostic ignored "-Winvalid-offsetof"
struct CoroutineLayout : Coroutine {
    static constexpr size_t state_offset() {
        return offsetof(CoroutineLayout, co_);
    }
};
#pragma GCC diagnostic pop
static_assert(CoroutineLayout::state_offset() == 0, "swr_v1_coroutine starts a Coroutine");

static Coroutine *co(void *c) {
    return static_cast<Coroutine *>(c);
}

extern "C" {
uint32_t swr_cpp_phpcoroutines_host_flags(uint32_t mask) {
    auto set = [mask](uint32_t flag, bool on) -> uint32_t { return (mask & flag) && on ? flag : 0; };
    uint32_t flags = 0;
    if (mask & SWR_PHPCOROUTINES_SERVER_STARTED) {
        flags |= set(SWR_PHPCOROUTINES_SERVER_STARTED, sw_server() && sw_server()->is_started());
    }
    if (mask & SWR_PHPCOROUTINES_FATAL) {
        flags |= set(SWR_PHPCOROUTINES_FATAL, php_swoole_is_fatal_error() || (sw_reactor() && sw_reactor()->bailout));
    }
    flags |= set(SWR_PHPCOROUTINES_MAIN_THREAD, sw_is_main_thread());
#ifdef SW_CORO_TIME
    flags |= mask & SWR_PHPCOROUTINES_CORO_TIME;
#endif
#ifdef SW_THREAD
    flags |= mask & SWR_PHPCOROUTINES_THREAD;
#endif
    return flags;
}

// A coroutine that ended in an exception or bailout takes the process down.
void swr_cpp_phpcoroutines_bailout() {
    Coroutine::bailout([]() {
        if (sw_reactor()) {
            sw_reactor()->running = false;
            sw_reactor()->bailout = true;
        }
        zend_bailout();
    });
}

void swr_cpp_phpcoroutines_kernel_coroutine(long count, double sleep_time) {
    Coroutine::create([=](void *) {
        SW_LOOP_N(count) {
            swoole::coroutine::System::sleep(sleep_time);
        }
    });
}

void swr_cpp_phpcoroutines_stats(swr_v1_phpcoroutines_stats *stats) {
    stats->event_num = sw_reactor() ? sw_reactor()->get_event_num() : 0;
    if (sw_async_threads()) {
        stats->aio_task_num = sw_async_threads()->get_task_num();
        stats->aio_worker_num = sw_async_threads()->get_worker_num();
        stats->aio_queue_size = sw_async_threads()->get_queue_size();
    }
#ifdef SW_USE_IOURING
    if (auto iouring = swr_v1_uring_current()) {
        stats->iouring = 1;
        stats->iouring_task_num = swr_v1_uring_task_num(iouring);
        stats->iouring_sq_usage_percent = swr_v1_uring_sq_usage_percent(iouring);
        stats->iouring_waiting_task_num = swr_v1_uring_waiting_num(iouring);
    }
#endif
}

void *swr_cpp_phpcoroutines_stack(void *c, size_t *size) {
    *size = co(c)->get_ctx().get_stack_size();
    return co(c)->get_ctx().get_stack();
}

uint8_t swr_cpp_phpcoroutines_is_end(void *c) {
    return co(c)->is_end();
}

// Activation: deactivate when the reactor is freed, then Coroutine::activate.
void swr_cpp_phpcoroutines_activate(void (*deactivate)(void *)) {
    sw_reactor()->add_destroy_callback(deactivate, nullptr);
    Coroutine::activate();
}

void swr_cpp_phpcoroutines_defer(void (*fn)(void *), void *data) {
    swoole_event_defer(fn, data);
}

void *swr_cpp_phpcoroutines_timer_add(double timeout, void *data, void (*fn)(void *)) {
    return swoole_timer_add(timeout, false, [data, fn](swoole::Timer *, TimerNode *) { fn(data); });
}

// Co::set(['exit_condition' => ...]): Rust owns the callable.
void swr_cpp_phpcoroutines_set_exit_condition(uint8_t set) {
    if (set) {
        SwooleG.user_exit_condition = [](Reactor *, size_t &) {
            uint8_t exit = 0;
            if (swr_v1_phpcoroutines_can_exit(&exit)) {
                zend_bailout();
            }
            return exit != 0;
        };
        if (sw_reactor()) {
            sw_reactor()->set_exit_condition(Reactor::EXIT_CONDITION_USER_AFTER_DEFAULT, SwooleG.user_exit_condition);
        }
    } else if (sw_reactor()) {
        sw_reactor()->remove_exit_condition(Reactor::EXIT_CONDITION_USER_AFTER_DEFAULT);
        SwooleG.user_exit_condition = nullptr;
    }
}
}
