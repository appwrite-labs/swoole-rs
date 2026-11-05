#ifndef SWOOLE_RS_REACTOR_DEFER_H
#define SWOOLE_RS_REACTOR_DEFER_H
#include "swoole_rs_reactor.h"
#include <cstdlib>
#include <functional>
#include <memory>
#include <new>
#include <utility>
namespace swoole {
// Native payload custody/effects only; Rust owns the sole ordering container.
// Distinct from the unchanged public CallbackManager/TaskList compatibility API.
class ReactorDeferredQueue {
    using Task = std::pair<std::function<void(void *)>, void *>;
    swr_v1_reactor_deferred *owner_ = nullptr;
  public:
    ReactorDeferredQueue() = default;
    ReactorDeferredQueue(const ReactorDeferredQueue &) = delete;
    ReactorDeferredQueue &operator=(const ReactorDeferredQueue &) = delete;
    void append(const std::function<void(void *)> &function, void *data) {
        std::unique_ptr<Task> task(new Task(function, data));
        switch (swr_v1_reactor_deferred_push(&owner_, reinterpret_cast<uintptr_t>(task.get()))) {
        case 0: task.release(); return;
        case -2: throw std::bad_alloc();
        default: std::abort();
        }
    }
    void execute() {
        uint32_t phase = 0;
        for (;;) {
            uintptr_t token = 0;
            int32_t action = swr_v1_reactor_deferred_step(owner_, &phase, &token);
            if (action == 0) return;
            if (action != 1) std::abort();
            // Exact original lexical stack copy; Rust detaches the fresh front
            // only after this Native effect. No extra invocation allocation.
            Task copied = *reinterpret_cast<const Task *>(token);
            if (swr_v1_reactor_deferred_step(owner_, &phase, &token) != 2) std::abort();
            delete reinterpret_cast<Task *>(token);
            copied.first(copied.second);
        }
    }
    ~ReactorDeferredQueue() noexcept {
        for (;;) {
            uintptr_t token = 0;
            switch (swr_v1_reactor_deferred_clear(&owner_, &token)) {
            case 0: return;
            case 1: delete reinterpret_cast<Task *>(token); break;
            default: std::abort();
            }
        }
    }
};
}
#endif
