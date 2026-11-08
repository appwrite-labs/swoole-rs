// SPDX-License-Identifier: Apache-2.0
// What crates/timer/src/owner.rs (the Rust-owned swoole::Timer) needs from C++: a
// TimerNode's std::function callback and destructor, C++ exceptions carried across
// Rust as exception_ptr, the reactor's std::function hooks, and timers of a plain
// function for Rust callers.
#include "swoole_timer.h"

#include <exception>
#include <memory>

using swoole::Reactor;
using swoole::Timer;
using swoole::TimerNode;

static void *caught() {
    return new std::exception_ptr(std::current_exception());
}

extern "C" {
TimerNode *swr_cpp_timer_node_new(void *data, const swoole::TimerCallback *callback) {
    auto *node = new TimerNode{};
    node->data = data;
    node->type = TimerNode::TYPE_KERNEL;
    node->callback = *callback;
    return node;
}

void *swr_cpp_timer_node_call(Timer *timer, TimerNode *node) {
    try {
        node->callback(timer, node);
        return nullptr;
    } catch (...) {
        return caught();
    }
}

// Runs the destructor and deletes the node, even if the destructor throws.
void *swr_cpp_timer_node_release(TimerNode *node) {
    try {
        std::unique_ptr<TimerNode> owner(node);
        if (node->destructor) {
            node->destructor(node);
        }
        return nullptr;
    } catch (...) {
        return caught();
    }
}

[[noreturn]] void swr_cpp_timer_rethrow(void *failure) {
    std::unique_ptr<std::exception_ptr> primary(static_cast<std::exception_ptr *>(failure));
    std::rethrow_exception(*primary);
}

void swr_cpp_timer_forget(void *failure) {
    delete static_cast<std::exception_ptr *>(failure);
}

// A one-shot timer whose callback is a plain function of its data, for Rust callers.
TimerNode *swr_cpp_timer_after(long msec, void (*callback)(void *data), void *data) {
    return swoole_timer_add(msec, false, [callback](Timer *, TimerNode *tnode) { callback(tnode->data); }, data);
}

void swr_cpp_timer_reactor_timeout(Reactor *reactor, int64_t msec) {
    reactor->timeout_msec = msec;
}

void swr_cpp_timer_attach(Reactor *reactor, Timer *timer) {
    reactor->set_end_callback(Reactor::PRIORITY_TIMER, [timer](Reactor *) { timer->select(); });
    reactor->set_exit_condition(Reactor::EXIT_CONDITION_TIMER,
                                [timer](Reactor *, size_t &) -> bool { return timer->count() == 0; });
    reactor->add_destroy_callback(
        [](void *data) {
            if (swoole_timer_is_available()) {
                // A coroutine bailout longjmps out of the timer callback that resumed it.
                if (static_cast<Reactor *>(data)->bailout) {
                    sw_timer()->abandon_selection();
                }
                swoole_timer_free();
            }
        },
        reactor);
}
}
