// SPDX-License-Identifier: Apache-2.0
// What crates/phpcollections' Swoole\Coroutine\Channel needs from C++
// (swoole_rs_phpcollections.h): the coroutine module's Channel, whose
// suspension and timers stay C++, and Swoole's fatal error.
#include "php_swoole_cxx.h"
#include "swoole_rs_phpcollections.h"
#include "swoole_coroutine_channel.h"

using swoole::coroutine::Channel;

static const swr_v1_phpcollections_channel_ops channel_ops = {
    [](size_t capacity) -> void * { return new Channel(capacity); },
    [](void *chan) { delete static_cast<Channel *>(chan); },
    [](void *chan, void *data, double timeout) -> uint8_t { return static_cast<Channel *>(chan)->push(data, timeout); },
    [](void *chan, double timeout) { return static_cast<Channel *>(chan)->pop(timeout); },
    [](void *chan) { return static_cast<Channel *>(chan)->pop_data(); },
    [](void *chan) -> int32_t { return static_cast<Channel *>(chan)->get_error(); },
    [](void *chan) -> uint8_t { return static_cast<Channel *>(chan)->close(); },
    [](void *chan) { return static_cast<Channel *>(chan)->length(); },
    [](void *chan) -> uint8_t { return static_cast<Channel *>(chan)->is_empty(); },
    [](void *chan) -> uint8_t { return static_cast<Channel *>(chan)->is_full(); },
    [](void *chan) { return static_cast<Channel *>(chan)->consumer_num(); },
    [](void *chan) { return static_cast<Channel *>(chan)->producer_num(); },
    [] { swoole_fatal_error(SW_ERROR_WRONG_OPERATION, "must call constructor first"); },
};

void php_swoole_channel_coro_minit(int module_number) {
    swr_v1_phpcollections_channel_minit(module_number, SWOOLE_G(use_shortname), &channel_ops);
}
