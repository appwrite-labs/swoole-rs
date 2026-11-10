/* SPDX-License-Identifier: Apache-2.0 */
/* Swoole's include/swoole_timer.h, owned here: TimerNode, and swoole::Timer as a
 * view of the Rust-owned timer (crates/timer/src/owner.rs). Upstream's file and
 * src/core/timer.cc are deleted, so `#include "swoole_timer.h"` finds this copy on
 * the Rust prefix's include path. */
#pragma once

#include "swoole.h"

#include "swoole_rs_timer.h"
#include "swoole_reactor.h"
#include "swoole_util.h"

#include <cstddef>
#include <vector>

#define SW_TIMER_MIN_MS 1
#define SW_TIMER_MIN_SEC 0.001
#define SW_TIMER_MAX_MS LONG_MAX
#define SW_TIMER_MAX_SEC ((double) LONG_MAX / 1000)

/**
 * manually_trigger:
 * Once enabled, the timer will no longer be triggered by event polling or the operating system's timer;
 * instead, it will be managed directly at the user space.
 */
SW_API swoole::Timer *swoole_timer_create(bool manually_trigger = false);
SW_API long swoole_timer_after(long ms, swoole::TimerCallback callback, void *private_data = nullptr);
SW_API long swoole_timer_tick(long ms, swoole::TimerCallback callback, void *private_data = nullptr);
SW_API swoole::TimerNode *swoole_timer_add(double ms,
                                           bool persistent,
                                           swoole::TimerCallback callback,
                                           void *private_data = nullptr);
SW_API swoole::TimerNode *swoole_timer_add(long ms,
                                           bool persistent,
                                           swoole::TimerCallback callback,
                                           void *private_data = nullptr);
SW_API bool swoole_timer_del(swoole::TimerNode *tnode);
SW_API bool swoole_timer_exists(long timer_id);
SW_API void swoole_timer_delay(swoole::TimerNode *tnode, long delay_ms);
SW_API swoole::TimerNode *swoole_timer_get(long timer_id);
SW_API bool swoole_timer_clear(long timer_id);
SW_API void swoole_timer_free();
SW_API void swoole_timer_select();
SW_API bool swoole_timer_is_available();

namespace swoole {
static_assert(sizeof(long) == 8, "Timer ABI v1 requires a 64-bit native long");
typedef void (*TimerDestructor)(TimerNode *);

// crates/timer/src/owner.rs allocates and frees every node.
struct TimerNode {
    enum Type {
        TYPE_KERNEL,
        TYPE_PHP,
    };
    long id;
    Type type;
    int64_t exec_msec;
    int64_t interval;
    uint64_t exec_count;
    uint64_t round;
    bool removed;
    TimerCallback callback;
    void *data;
    TimerDestructor destructor;
};
#pragma GCC diagnostic push
#pragma GCC diagnostic ignored "-Winvalid-offsetof"
static_assert(offsetof(TimerNode, exec_msec) == 16 && offsetof(TimerNode, removed) == 48 &&
                  offsetof(TimerNode, destructor) == 72,
              "crates/timer/src/owner.rs's TimerNode");
#pragma GCC diagnostic pop
}  // namespace swoole

extern "C" {
swoole::Timer *swr_v1_timer_owner_new(bool manually_trigger);
void swr_v1_timer_owner_free(swoole::Timer *timer);
swr_v1_timer *swr_v1_timer_owner_queue(const swoole::Timer *timer);
void *swr_v1_timer_owner_reactor(const swoole::Timer *timer);
void swr_v1_timer_owner_reinit(swoole::Timer *timer, bool manually_trigger);
swoole::TimerNode *swr_v1_timer_owner_add(
    swoole::Timer *timer, long msec, bool persistent, void *data, swoole::TimerCallback callback);
bool swr_v1_timer_owner_remove(swoole::Timer *timer, long id);
void swr_v1_timer_owner_update(swoole::Timer *timer, const swoole::TimerNode *node);
void swr_v1_timer_owner_delay(swoole::Timer *timer, const swoole::TimerNode *node, long delay_ms);
// A C++ exception a callback or destructor throws aborts at Rust's frames (panic = "abort").
void swr_v1_timer_owner_select(swoole::Timer *timer);
void swr_v1_timer_owner_abandon(swoole::Timer *timer, void (*bailout)());
}

namespace swoole {
// Rust owns the record: create it with swoole_timer_create (or swr_v1_timer_owner_new).
class Timer {
    swr_v1_timer *queue() const {
        return swr_v1_timer_owner_queue(this);
    }
    swr_v1_timer_stats stats() const {
        swr_v1_timer_stats stats{};
        swr_v1_timer_stats_get(queue(), &stats);
        return stats;
    }

  public:
    Timer() = delete;
    ~Timer() = delete;

    Reactor *get_reactor() const {
        return static_cast<Reactor *>(swr_v1_timer_owner_reactor(this));
    }
    TimerNode *add(long msec, bool persistent, void *data, TimerCallback callback) {
        return swr_v1_timer_owner_add(this, msec, persistent, data, callback);
    }
    bool remove(long id) {
        return swr_v1_timer_owner_remove(this, id);
    }
    bool remove(TimerNode *tnode) {
        return tnode && remove(tnode->id);
    }
    void update(const TimerNode *tnode) {
        swr_v1_timer_owner_update(this, tnode);
    }
    void delay(TimerNode *tnode, long delay_ms) {
        swr_v1_timer_owner_delay(this, tnode, delay_ms);
    }
    void reinit(bool manually_trigger = false) {
        swr_v1_timer_owner_reinit(this, manually_trigger);
    }
    void select() {
        swr_v1_timer_owner_select(this);
    }
    // Only after a caught Zend bailout whose native selection will never resume.
    void abandon_selection(void (*bailout)() = nullptr) {
        swr_v1_timer_owner_abandon(this, bailout);
    }
    TimerNode *get(long id) const {
        return static_cast<TimerNode *>(swr_v1_timer_token(queue(), id));
    }
    TimerNode *get(long id, const TimerNode::Type type) const {
        TimerNode *tnode = get(id);
        return (tnode && tnode->type == type) ? tnode : nullptr;
    }
    size_t count() const {
        return stats().count;
    }
    uint64_t get_round() const {
        return stats().round;
    }
    std::vector<int64_t> get_ids() const {
        std::vector<int64_t> ids(count());
        size_t copied = 0;
        swr_v1_timer_ids(queue(), ids.data(), ids.size(), &copied);
        ids.resize(copied);
        return ids;
    }
};

static inline long sec2msec(const long sec) {
    return sec * 1000;
}

static inline long sec2msec(const int sec) {
    return sec * 1000;
}

static inline long sec2msec(const double sec) {
    return static_cast<long>(sec * 1000);
}

static inline double msec2sec(const int msec) {
    return static_cast<double>(msec) / 1000.0;
}
}  // namespace swoole
