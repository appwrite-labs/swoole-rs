/* SPDX-License-Identifier: Apache-2.0 */
/* Swoole's include/swoole_coroutine.h and swoole_coroutine_context.h, owned here: the
 * Coroutine and Context facades over crates/coroutine's records, keeping what native/,
 * the vendored code and the compiled core-tests still call. Upstream's files are
 * deleted, so `#include "swoole_coroutine.h"` finds this copy on the Rust prefix's
 * include path. */
#pragma once

#include "swoole_string.h"
#include "swoole_socket.h"
#include "swoole_reactor.h"
#include "swoole_timer.h"
#include "swoole_async.h"
#include "swoole_util.h"

#include "swoole_rs_coroutine.h"

#include <climits>

#include <functional>
#include <string>
#include <system_error>
#include <unordered_map>

typedef std::chrono::microseconds seconds_type;
typedef std::function<void(void *)> CoroutineFunc;

static_assert(sizeof(void *) != 8 || (sizeof(swr_v1_coroutine) == 96 && sizeof(swr_v1_coroutine_scheduler) == 128),
              "crates/coroutine layout");

namespace swoole {
namespace coroutine {
// A view over crates/coroutine's context record: Rust allocates the stack and
// switches onto it, or with SW_USE_THREAD_CONTEXT runs it on its own thread.
class Context {
  public:
    Context(size_t stack_size, CoroutineFunc fn, void *private_data) : fn_(std::move(fn)), private_data_(private_data) {
        if (int error = swr_v1_context_init(&ctx_, stack_size, FLAGS, &run, this)) {
            throw std::system_error(error, std::system_category());
        }
    }
    ~Context() {
        swr_v1_context_free(&ctx_);
    }
    void swap_in() {
        swr_v1_context_swap_in(&ctx_);
    }
    void swap_out() {
        swr_v1_context_swap_out(&ctx_);
    }
#ifdef SW_CONTEXT_DETECT_STACK_USAGE
    ssize_t get_stack_usage() {
        return swr_v1_context_stack_usage(ctx_.stack, ctx_.stack_size);
    }
#endif
    char *get_stack() const {
        return reinterpret_cast<char *>(ctx_.stack);
    }
    size_t get_stack_size() const {
        return ctx_.stack_size;
    }
    bool is_end() const {
        return ctx_.end;
    }

  protected:
    CoroutineFunc fn_;
    void *private_data_;
    swr_v1_context ctx_;
    static constexpr uint32_t FLAGS = 0
#ifdef SW_USE_THREAD_CONTEXT
                                      | SWR_CTX_THREAD
#endif
#ifdef SW_CONTEXT_PROTECT_STACK_PAGE
                                      | SWR_CTX_STACK_PROTECT
#endif
#ifdef SW_CONTEXT_DETECT_STACK_USAGE
                                      | SWR_CTX_STACK_MARK
#endif
        ;

    static void run(void *self) {
        auto *_this = static_cast<Context *>(self);
        _this->fn_(_this->private_data_);
    }
};
}  // namespace coroutine

// A facade over the Rust-owned scheduler (crates/coroutine): getters read its
// state inline; the operations are Rust's, which call back into the hooks below
// for the context switch, delete and on_bailout (native/coroutine/coroutine.cc).
class Coroutine {
  public:
    enum State {
        STATE_INIT = 0,
        STATE_WAITING,
        STATE_RUNNING,
        STATE_END,
    };

    enum ResumeCode {
        RC_OK = 0,
        RC_TIMEDOUT = -1,
        RC_CANCELED = -2,
    };

    typedef void (*SwapCallback)(void *);
    typedef std::function<void(void)> BailoutCallback;
    typedef std::function<bool(swoole::Coroutine *)> CancelFunc;

    void resume() {
        swr_v1_coroutine_resume(&scheduler_, &co_);
    }
    void yield() {
        swr_v1_coroutine_yield(&scheduler_, &co_);
    }
    void yield(CancelFunc *cancel_fn) {
        swr_v1_coroutine_canceler canceler = {&call_cancel, cancel_fn};
        swr_v1_coroutine_yield_cancelable(&scheduler_, &co_, &canceler);
    }
    bool cancel();

    bool yield_ex(double timeout = -1);

    long get_init_msec() const {
        return co_.init_msec;
    }

    long get_cid() const {
        return co_.cid;
    }

    Coroutine *get_origin() const {
        return co_.origin ? static_cast<Coroutine *>(co_.origin->native) : nullptr;
    }

    long get_origin_cid() const {
        return sw_likely(co_.origin) ? co_.origin->cid : -1;
    }

    void *get_task() const {
        return co_.task;
    }

    bool is_end() const {
        return ctx.is_end();
    }

    bool is_canceled() const {
        return co_.resume_code == RC_CANCELED;
    }

    bool is_timedout() const {
        return co_.resume_code == RC_TIMEDOUT;
    }

    bool is_suspending() const {
        return co_.state == STATE_WAITING;
    }

    void set_task(void *_task) {
        swr_v1_coroutine_set_task(&co_, _task);
    }

    coroutine::Context &get_ctx() {
        return ctx;
    }

    // Upstream's range-for over the live coroutines (`co.second` is each one),
    // newest first; Rust keeps them.
    static struct Live {
        struct iterator {
            std::pair<long, Coroutine *> entry;
            std::pair<long, Coroutine *> &operator*() {
                return entry;
            }
            iterator &operator++() {
                entry.second = static_cast<Coroutine *>(swr_v1_coroutine_older(&scheduler_, entry.first, &entry.first));
                return *this;
            }
            bool operator!=(const iterator &other) const {
                return entry.second != other.entry.second;
            }
        };
        iterator begin() const {
            return ++iterator{{LONG_MAX, nullptr}};
        }
        iterator end() const {
            return {{0, nullptr}};
        }
    } coroutines;

    static void set_on_yield(SwapCallback func) {
        swr_v1_coroutine_set_callback(&scheduler_, SWR_CO_ON_YIELD, func);
    }
    static void set_on_resume(SwapCallback func) {
        swr_v1_coroutine_set_callback(&scheduler_, SWR_CO_ON_RESUME, func);
    }
    static void set_on_close(SwapCallback func) {
        swr_v1_coroutine_set_callback(&scheduler_, SWR_CO_ON_CLOSE, func);
    }
    static void bailout(const BailoutCallback &func);

    static long create(const CoroutineFunc &fn, void *args = nullptr) {
#ifdef SW_USE_THREAD_CONTEXT
        try {
            return (new Coroutine(fn, args))->run();
        } catch (const std::system_error &e) {
            swoole_set_last_error(e.code().value());
            swoole_warning("failed to create coroutine, Error: %s[%d]", e.what(), swoole_get_last_error());
            return -1;
        }
#else
        return (new Coroutine(fn, args))->run();
#endif
    }

    static void activate();
    static void deactivate();

    static Coroutine *get_current() {
        return scheduler_.current ? static_cast<Coroutine *>(scheduler_.current->native) : nullptr;
    }

    static Coroutine *get_current_safe() {
        if (sw_unlikely(!scheduler_.current)) {
            swoole_fatal_error(SW_ERROR_CO_OUT_OF_COROUTINE, "API must be called in the coroutine");
        }
        return get_current();
    }

    static void *get_current_task() {
        return sw_likely(scheduler_.current) ? scheduler_.current->task : nullptr;
    }

    static long get_current_cid() {
        return sw_likely(scheduler_.current) ? scheduler_.current->cid : -1;
    }

    static Coroutine *get_by_cid(long cid) {
        return static_cast<Coroutine *>(swr_v1_coroutine_get(&scheduler_, cid));
    }

    static void *get_task_by_cid(long cid) {
        Coroutine *co = get_by_cid(cid);
        return sw_likely(co) ? co->get_task() : nullptr;
    }

    static size_t get_stack_size() {
        return scheduler_.stack_size;
    }

    static void set_socket_bound_cid(long cid) {
        swr_v1_coroutine_bind_socket(&scheduler_, cid);
    }

    static long get_last_cid() {
        return scheduler_.last_cid;
    }

    static long get_socket_bound_cid() {
        return scheduler_.socket_bound_cid;
    }

    static size_t count() {
        return swr_v1_coroutine_count(&scheduler_);
    }

    static uint64_t get_peak_num() {
        return scheduler_.peak_num;
    }

    static long get_elapsed(long cid) {
        Coroutine *co = cid == 0 ? get_current() : get_by_cid(cid);
        return sw_likely(co) ? Timer::get_absolute_msec() - co->get_init_msec() : -1;
    }

    static void print_list();
    static swr_v1_coroutine_scheduler *get_scheduler() {
        return &scheduler_;
    }
    static void print_socket_bound_error(int sock_fd, const char *event_str, long bound_cid);

  protected:
    static SW_THREAD_LOCAL swr_v1_coroutine_scheduler scheduler_;
    static SW_THREAD_LOCAL BailoutCallback on_bailout; /* when bailout */

    swr_v1_coroutine co_;
    coroutine::Context ctx;

    Coroutine(const CoroutineFunc &fn, void *private_data);
    long run() {
        return swr_v1_coroutine_run(&scheduler_, &co_);
    }

    // The hooks scheduler_ hands Rust.
    static bool swap_in(void *co);
    static void swap_out(void *co);
    static void destroy(void *co);
    static void call_on_bailout();
    // Calls a copy of the CancelFunc `fn`.
    static bool call_cancel(void *co, void *fn);
};

namespace coroutine {
bool run(const CoroutineFunc &fn, void *arg = nullptr);
}  // namespace coroutine
}  // namespace swoole

// For gdb.
swoole::Coroutine *swoole_coroutine_iterator_each();
void swoole_coroutine_iterator_reset();
swoole::Coroutine *swoole_coroutine_get(long cid);
size_t swoole_coroutine_count();
