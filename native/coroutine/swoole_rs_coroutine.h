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
 * swoole::Coroutine record (a swr_v1_coroutine, then its swr_v1_context). C++
 * reads fields inline; every write goes through a function below. Pointer
 * arguments are live and unshared. run/resume/yield/bailout are whole
 * operations that call the swap callbacks and the hooks, which take a record. */
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
/* Coroutine::bailout: inside a coroutine callback(data) runs in the main
 * context (PHP's bailout, or the process exits); outside one, later resumes
 * are refused. */
void swr_v1_coroutine_bail(void (*callback)(void *), void *data);
/* Coroutine stacks (crates/coroutine/src/stack.rs). PROTECT maps the stack and
 * makes its lowest page inaccessible (SW_CONTEXT_PROTECT_STACK_PAGE); MARK
 * writes the usage markers (SW_CONTEXT_DETECT_STACK_USAGE). Free with the same
 * size and flags. alloc returns null when out of memory. */
enum { SWR_CTX_STACK_PROTECT = 1, SWR_CTX_STACK_MARK = 2 };
void *swr_v1_context_stack_alloc(size_t size, uint32_t flags);
void swr_v1_context_stack_free(void *stack, size_t size, uint32_t flags);
size_t swr_v1_context_stack_usage(const uint8_t *stack, size_t size);
/* A coroutine's context (crates/coroutine/src/context.rs): the stack and the
 * boost.context switch onto it, with AddressSanitizer's fiber annotations when
 * its runtime is loaded. init runs entry(data) on the first swap_in and exits
 * with upstream's fatal error when out of memory; the record must not move until
 * free. swap_out from the running context returns to its last swap_in; end is
 * set once entry returns. With SWR_CTX_THREAD the context is a thread with no
 * stack (crates/coroutine/src/thread.rs) and init returns the errno of a failed
 * spawn, else 0; thread_context_init/clean bracket the activating thread. */
enum { SWR_CTX_THREAD = 4 };
void swr_v1_thread_context_init(void);
typedef struct {
    void *fctx, *swap_fctx;
    uint8_t *stack;
    uint32_t stack_size, flags;
    void (*entry)(void *);
    void *data;
    const void *asan_bottom;
    size_t asan_size;
    bool end;
} swr_v1_context;
#ifdef __cplusplus
}
#endif
#endif
