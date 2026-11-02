/* SPDX-License-Identifier: Apache-2.0 */
#ifndef SWOOLE_RS_COROUTINE_H
#define SWOOLE_RS_COROUTINE_H
#include <stddef.h>
#include <stdint.h>
#include <stdbool.h>
#ifdef __cplusplus
extern "C" {
#endif
/* The scheduler and each coroutine's state (crates/coroutine/src/coroutine.rs).
 * C++ stores them in their original scopes and reads fields inline; every write
 * goes through a function below. Pointer arguments are live and unshared. */
typedef struct swr_v1_coroutine swr_v1_coroutine;
struct swr_v1_coroutine {
    void *native; /* the swoole::Coroutine facade */
    long cid;
    int32_t state, resume_code;
    void *task, *cancel_fn;
    swr_v1_coroutine *origin, *first_child, *prev_sibling, *next_sibling;
    long init_msec, switch_usec, execute_usec;
};
typedef void (*swr_v1_coroutine_swap_function)(void *);
typedef struct {
    swr_v1_coroutine *current;
    long last_cid, socket_bound_cid;
    uint64_t peak_num;
    size_t stack_size;
    swr_v1_coroutine_swap_function on_yield, on_resume, on_close;
    bool activated;
} swr_v1_coroutine_scheduler;
enum { SWR_CO_ON_YIELD = 0, SWR_CO_ON_RESUME = 1, SWR_CO_ON_CLOSE = 2 };
long swr_v1_coroutine_register(swr_v1_coroutine_scheduler *, swr_v1_coroutine *, void *native, long init_msec, long now);
bool swr_v1_coroutine_admit(swr_v1_coroutine_scheduler *, size_t count);
void swr_v1_coroutine_set_active(swr_v1_coroutine_scheduler *, bool);
void swr_v1_coroutine_set_stack_size(swr_v1_coroutine_scheduler *, size_t);
void swr_v1_coroutine_bind_socket(swr_v1_coroutine_scheduler *, long cid);
void swr_v1_coroutine_set_callback(swr_v1_coroutine_scheduler *, uint32_t slot, swr_v1_coroutine_swap_function);
void swr_v1_coroutine_enter(swr_v1_coroutine_scheduler *, swr_v1_coroutine *);
void swr_v1_coroutine_wake(swr_v1_coroutine *);
void swr_v1_coroutine_suspend(swr_v1_coroutine *);
void swr_v1_coroutine_leave(swr_v1_coroutine_scheduler *, swr_v1_coroutine *);
void swr_v1_coroutine_end(swr_v1_coroutine *);
void swr_v1_coroutine_release(swr_v1_coroutine_scheduler *, swr_v1_coroutine *);
void swr_v1_coroutine_set_origin(swr_v1_coroutine *, swr_v1_coroutine *origin);
void swr_v1_coroutine_set_task(swr_v1_coroutine *, void *);
void swr_v1_coroutine_set_cancel_fn(swr_v1_coroutine *, void *);
void swr_v1_coroutine_cancel(swr_v1_coroutine *);
void swr_v1_coroutine_timeout(swr_v1_coroutine *);
void swr_v1_coroutine_switch_time(swr_v1_coroutine *yielding, swr_v1_coroutine *resuming, long now);
long swr_v1_coroutine_execute_usec(const swr_v1_coroutine *, long now);
/* swoole::coroutine::Channel (crates/coroutine/src/channel.rs): an owned
 * handle with whole pop/push/close operations that call back into C++ for the
 * timer, yield and resume. Rust never touches a channel after a resume that ends
 * its operation, and holds no reference to it across an effect. */
typedef struct swr_v1_channel swr_v1_channel;
typedef struct {
    size_t length, producers, consumers;
    int32_t error;
    bool closed, full, empty;
} swr_v1_channel_view;
enum { SWR_CH_CANCELED = 1, SWR_CH_TIMEDOUT = 2 };
enum { SWR_CH_TRACE_PUSHED = 0, SWR_CH_TRACE_CLOSED = 1 };
enum { SWR_CH_ALLOCATION_REFUSED = -5 };
typedef struct {
    void *ctx;
    void *current;
    void (*arm_timer)(void *);
    void (*delete_timer)(void *);
    uint32_t (*suspend)(void *, uint32_t role); /* returns SWR_CH_CANCELED | SWR_CH_TIMEDOUT */
    void (*resume)(void *, uint32_t role, void *co);
    void (*trace)(void *, uint32_t what, size_t length);
} swr_v1_channel_effects;
typedef struct { int32_t status; void *data; } swr_v1_channel_outcome;
swr_v1_channel *swr_v1_channel_new(size_t capacity);
void swr_v1_channel_free(swr_v1_channel *);
swr_v1_channel_view swr_v1_channel_read(const swr_v1_channel *);
void *swr_v1_channel_pop_data(swr_v1_channel *);
void swr_v1_channel_remove(swr_v1_channel *, uint32_t role, void *co);
swr_v1_channel_outcome swr_v1_channel_pop(swr_v1_channel *, bool positive_timeout, const swr_v1_channel_effects *);
int32_t swr_v1_channel_push(swr_v1_channel *, bool positive_timeout, void *data, const swr_v1_channel_effects *);
bool swr_v1_channel_close(swr_v1_channel *, const swr_v1_channel_effects *);
/* Typed same-layout Context cells. Raw initialization is performed only at the
 * original publication points; no reference to uninitialized fields is formed. */
typedef struct { void *pointer; } swr_v1_context_stack_cell;
typedef struct { uint32_t size; } swr_v1_context_extent;
typedef struct { uint8_t value; } swr_v1_context_completion;
typedef struct { void *pointer, *top; size_t size; uint8_t present; } swr_v1_context_stack_view;
enum { SWR_CTX_STACK_ADMIT = 0, SWR_CTX_STACK_READ = 1, SWR_CTX_STACK_TOP = 2,
       SWR_CTX_STACK_CLEAR = 3, SWR_CTX_STACK_MARK = 4, SWR_CTX_STACK_USAGE = 5 };
swr_v1_context_extent swr_v1_context_extent_new(size_t requested);
/* First constructor statement seeds completion; thread backend has null extent. */
size_t swr_v1_context_prepare(swr_v1_context_completion *, const swr_v1_context_extent *);
void swr_v1_context_finish(swr_v1_context_completion *);
uint8_t swr_v1_context_completed(const swr_v1_context_completion *);
swr_v1_context_stack_view swr_v1_context_stack_read(const swr_v1_context_stack_cell *, const swr_v1_context_extent *);
/* ADMIT follows Native allocation, CLEAR follows Native free without another
 * observation. READ/TOP refresh after Native effects; MARK uses original full
 * requested size, USAGE uses stored truncated extent. Payload marker access
 * requires the same live allocation/initialized spans as frozen marker exports.
 * No Native resource/context/function/thread effects occur inside Rust. */
swr_v1_context_stack_view swr_v1_context_stack_step(swr_v1_context_stack_cell *,
    const swr_v1_context_extent *, uint32_t operation, void *allocation, size_t requested);
/* Native-owned stack bytes, no retained pointer or native effects. Only marker
 * spans are written/read; usage requires those spans already initialized. */
void swr_v1_context_stack_mark(uint8_t *stack, size_t size);
size_t swr_v1_context_stack_usage(const uint8_t *stack, size_t size);
#ifdef __cplusplus
}
#endif
#endif
