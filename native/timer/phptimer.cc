// SPDX-License-Identifier: Apache-2.0
// What crates/phptimer's Swoole\Timer needs from C++ (swoole_rs_phptimer.h):
// the C++ Timer's operations, the callback and destructor trampolines that
// re-raise a bailout Rust reports, and the zend::Function call.
#include "swoole_rs_phphost.h"
#include "swoole_server.h"
#include "swoole_rs_phptimer.h"
#include "swoole_rs_phpprocess.h"

#include "ext/spl/spl_array.h"

using swoole::Timer;
using swoole::TimerNode;

static void timer_dtor(TimerNode *tnode) {
    if (swr_v1_phptimer_free(tnode->data)) {
        auto *timer = sw_timer();
        if (!timer) zend_bailout();
        // Return through release_node so its detached TimerNode is deleted first.
        timer->abandon_selection([] { zend_bailout(); });
    }
}

static void timer_callback(Timer *timer, TimerNode *tnode) {
    if (swr_v1_phptimer_call(tnode->data)) {
        // Keep shutdown-visible metadata until native owner teardown.
        timer->abandon_selection();
        zend_bailout();
    }
}

// Whether a timer added here runs on this process's reactor.
static bool timer_uses_reactor() {
    if (auto *server = sw_server()) {
        return server->is_user_worker() || (server->is_task_worker() && server->task_enable_coroutine);
    }
    auto *pool = static_cast<swoole::ProcessPool *>(swr_v1_phpprocess_pool_current());
    return !pool || !pool->is_master();
}

static const swr_v1_phptimer_ops timer_ops = {
    []() -> uint8_t { return swoole_timer_is_available(); },
    [](int64_t ms, uint8_t persistent, void *callable) -> int64_t {
        if (UNEXPECTED(!sw_reactor() && timer_uses_reactor())) {
            php_swoole_check_reactor();
        }
        TimerNode *tnode = swoole_timer_add((long) ms, persistent, timer_callback, callable);
        if (UNEXPECTED(!tnode)) {
            return 0;
        }
        tnode->type = TimerNode::TYPE_PHP;
        tnode->destructor = timer_dtor;
        return tnode->id;
    },
    [](int64_t id, swr_v1_phptimer_node *node) -> uint8_t {
        TimerNode *tnode = swoole_timer_get(id);
        if (!tnode) {
            return 0;
        }
        *node = {tnode->exec_msec, tnode->exec_count, tnode->interval, tnode->round,
                 tnode->type == TimerNode::TYPE_PHP, tnode->removed};
        return 1;
    },
    [](size_t *count, uint64_t *round) {
        *count = SwooleTG.timer->count();
        *round = SwooleTG.timer->get_round();
    },
    [](int64_t *ids, size_t capacity) -> size_t {
        auto all = SwooleTG.timer->get_ids();
        size_t copied = std::min(capacity, all.size());
        std::copy_n(all.begin(), copied, ids);
        return copied;
    },
    [](int64_t id) -> uint8_t {
        Timer *timer = SwooleTG.timer;
        TimerNode *tnode = timer ? timer->get(id, TimerNode::TYPE_PHP) : nullptr;
        return tnode && timer->remove(tnode);
    },
    [](void *invocation) {
        auto *call = static_cast<swr_v1_phptimer_invocation *>(invocation);
        call->ok = static_cast<zend::Function *>(call->callable)->call(nullptr, php_swoole_is_enable_coroutine());
    },
    [](const char *message) { php_swoole_fatal_error(E_WARNING, "%s", message); },
    [](const char *message) { php_swoole_error(E_WARNING, "%s", message); },
};

void php_swoole_timer_minit(int module_number) {
    swr_v1_phptimer_minit(module_number, spl_ce_ArrayIterator, &timer_ops);
}
