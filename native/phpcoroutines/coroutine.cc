// SPDX-License-Identifier: Apache-2.0
// Compatibility derived from Swoole v6.2.4 ext-src/swoole_coroutine.cc (Swoole contributors).
// What crates/phpcoroutines needs of C++ that stable Rust can't express: the
// std::function Coroutine::bailout takes, and a Coroutine's Context, whose
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
}
