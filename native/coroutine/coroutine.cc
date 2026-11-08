// SPDX-License-Identifier: Apache-2.0
// Compatibility derived from Swoole v6.2.4 src/coroutine/base.cc (Swoole contributors).
// What crates/coroutine can't express of a swoole::Coroutine: the thread-local
// scheduler and bailout callback, the facade's construction and delete (its
// Context keeps a std::function), and the std::function arguments and timers.
// Rust owns the rest (crates/coroutine/src/facade.rs).
#include "swoole_coroutine.h"

namespace swoole {

#ifdef SW_CORO_TIME
static long coroutine_clock() {
    return time<seconds_type>(true);
}
#else
#define coroutine_clock nullptr
#endif

SW_THREAD_LOCAL swr_v1_coroutine_scheduler Coroutine::scheduler_ = {
    nullptr,
    0,
    0,
    0,
    SW_DEFAULT_C_STACK_SIZE,
    nullptr,
    nullptr,
    nullptr,
    {&Coroutine::swap_in, &Coroutine::swap_out, &Coroutine::destroy, &Coroutine::call_on_bailout, coroutine_clock},
    nullptr,
    LONG_MAX,
    false,
    false};
Coroutine::Live Coroutine::coroutines;
SW_THREAD_LOCAL Coroutine::BailoutCallback Coroutine::on_bailout = nullptr;

// Expected never to run: the scheduler can no longer resume a coroutine.
static void bailed_out() {
    swoole_error("have been bailout, can not resume any coroutine");
}

bool Coroutine::swap_in(void *co) {
    auto *self = static_cast<Coroutine *>(co);
    self->ctx.swap_in();
    return self->ctx.is_end();
}

void Coroutine::swap_out(void *co) {
    static_cast<Coroutine *>(co)->ctx.swap_out();
}

void Coroutine::destroy(void *co) {
    auto *self = static_cast<Coroutine *>(co);
#if !defined(SW_USE_THREAD_CONTEXT) && defined(SW_CONTEXT_DETECT_STACK_USAGE)
    swoole_trace_log(SW_TRACE_CONTEXT,
                     "coroutine#%ld stack memory use less than %ld bytes",
                     self->get_cid(),
                     self->ctx.get_stack_usage());
#endif
    delete self;
}

void Coroutine::call_on_bailout() {
    on_bailout();
}

bool Coroutine::call_cancel(void *co, void *fn) {
    auto copy = *static_cast<CancelFunc *>(fn);
    return copy(static_cast<Coroutine *>(co));
}

#ifdef SW_USE_THREAD_CONTEXT
namespace coroutine {
void thread_context_init();
void thread_context_clean();
}  // namespace coroutine
#endif

void Coroutine::activate() {
#ifdef SW_USE_THREAD_CONTEXT
    coroutine::thread_context_init();
#endif
    swr_v1_coroutine_set_active(&scheduler_, true);
    on_bailout = nullptr;
}

void Coroutine::deactivate() {
#ifdef SW_USE_THREAD_CONTEXT
    coroutine::thread_context_clean();
#endif
    swr_v1_coroutine_set_active(&scheduler_, false);
    on_bailout = bailed_out;
}

Coroutine::Coroutine(const CoroutineFunc &fn, void *private_data) : ctx(get_stack_size(), fn, private_data) {
    if (swr_v1_coroutine_register(&scheduler_, &co_, this, Timer::get_absolute_msec(), time<seconds_type>(true))) {
        activate();
    }
}

// Inside a coroutine, Rust switches to the main context and never returns.
void Coroutine::bailout(const BailoutCallback &func) {
    Coroutine *co = get_current();
    if (co && !func) {
        swoole_error("bailout without callback function");
    }
    on_bailout = co ? func : bailed_out;
    swr_v1_coroutine_bailout(&scheduler_);
    if (co) {
        exit(SW_CORO_BAILOUT_EXIT_CODE);
    }
}
}  // namespace swoole

extern "C" {
swr_v1_coroutine_scheduler *swr_cpp_coroutine_scheduler() {
    return swoole::Coroutine::get_scheduler();
}

long swr_cpp_coroutine_create(void (*routine)(void *), void *arg, const CoroutineFunc *fn) {
    return fn ? swoole::Coroutine::create(*fn, arg) : swoole::Coroutine::create(routine, arg);
}

void *swr_cpp_coroutine_timer_add(double timeout, void (*fn)(void *), void *data) {
    return swoole_timer_add(
        timeout, false, [fn](swoole::Timer *, swoole::TimerNode *tnode) { fn(tnode->data); }, data);
}
}
