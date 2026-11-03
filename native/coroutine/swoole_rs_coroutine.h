/* SPDX-License-Identifier: Apache-2.0 */
#ifndef SWOOLE_RS_COROUTINE_H
#define SWOOLE_RS_COROUTINE_H
#include <stddef.h>
#include <stdio.h>
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
bool swr_v1_coroutine_admit(swr_v1_coroutine_scheduler *);
/* The live coroutines (crates/coroutine/src/live.rs), per thread, from register
 * until release; the void pointers are swoole::Coroutine facades. get: null if
 * cid is not live. older: the newest live coroutine older than `before` (LONG_MAX
 * starts a walk), its id in *cid, or null. print_list prints them to `out`. */
void *swr_v1_coroutine_get(long cid);
size_t swr_v1_coroutine_count(void);
void *swr_v1_coroutine_older(long before, long *cid);
void swr_v1_coroutine_iterator_reset(void);
void *swr_v1_coroutine_iterator_each(void);
void swr_v1_coroutine_print_list(FILE *out);
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
/* Coroutine stacks (crates/coroutine/src/stack.rs). PROTECT maps the stack and
 * makes its lowest page inaccessible (SW_CONTEXT_PROTECT_STACK_PAGE); MARK
 * writes the usage markers (SW_CONTEXT_DETECT_STACK_USAGE). Free with the same
 * size and flags. alloc returns null when out of memory. */
enum { SWR_CTX_STACK_PROTECT = 1, SWR_CTX_STACK_MARK = 2 };
void *swr_v1_context_stack_alloc(size_t size, uint32_t flags);
void swr_v1_context_stack_free(void *stack, size_t size, uint32_t flags);
size_t swr_v1_context_stack_usage(const uint8_t *stack, size_t size);
#ifdef __cplusplus
}
#endif
#endif
