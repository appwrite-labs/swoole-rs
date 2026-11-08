// SPDX-License-Identifier: Apache-2.0
// Compatibility derived from Swoole v6.2.4 ext-src/swoole_coroutine.cc (Swoole contributors).
// What crates/phpcoroutines needs of C++ that stable Rust can't express: the
// std::functions Coroutine::bailout and SwooleG.user_exit_condition take
// (phpevents' event.cc reads the latter), and a Coroutine's Context, whose
// record sits behind a std::function member.
#include "swoole_rs_phphost.h"
#include "swoole_coroutine.h"

using swoole::Coroutine;
using swoole::Reactor;

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

extern "C" {
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

void *swr_cpp_phpcoroutines_stack(void *c, size_t *size) {
    auto *co = static_cast<Coroutine *>(c);
    *size = co->get_ctx().get_stack_size();
    return co->get_ctx().get_stack();
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
