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
/* The scheduler and each coroutine's state (crates/coroutine/src/coroutine.rs,
 * live.rs, facade.rs). Rust owns both: this thread's scheduler, and each
 * swoole::Coroutine record (a swr_v1_coroutine, then its context). C++ reads
 * fields inline; Rust does every write. The hooks take a record. */
typedef struct swr_v1_coroutine swr_v1_coroutine;
struct swr_v1_coroutine {
    void *native; /* the swoole::Coroutine record */
    long cid;
    int32_t state, resume_code;
    void *task, *cancel_fn;
    swr_v1_coroutine *origin, *first_child, *prev_sibling, *next_sibling;
    long init_msec, switch_usec, execute_usec;
};
typedef void (*swr_v1_coroutine_swap_function)(void *);
typedef struct {
    bool (*swap_in)(void *); /* returns whether the coroutine ended */
    void (*swap_out)(void *);
    void (*destroy)(void *);        /* frees the ended record */
    void (*bailout)(void *);        /* on_bailout(bailout_data), or null */
    void *bailout_data;
    long (*clock)(void);            /* SW_CORO_TIME's clock, or null */
} swr_v1_coroutine_hooks;
typedef struct {
    swr_v1_coroutine *current;
    long last_cid, socket_bound_cid;
    uint64_t peak_num;
    size_t stack_size;
    swr_v1_coroutine_swap_function on_yield, on_resume, on_close;
    swr_v1_coroutine_hooks hooks;
    void *live; /* Rust's map, null until the first coroutine */
    long cursor;
    bool activated, bailout;
} swr_v1_coroutine_scheduler;
enum { SWR_CO_ON_YIELD = 0, SWR_CO_ON_RESUME = 1, SWR_CO_ON_CLOSE = 2 };
/* This thread's scheduler (upstream's SW_THREAD_LOCAL Coroutine::scheduler_). */
swr_v1_coroutine_scheduler *swr_v1_coroutine_scheduler_get(void);
#ifdef __cplusplus
}
#endif
#endif
