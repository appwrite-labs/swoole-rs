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
 * live.rs). C++ stores them in their original scopes and reads fields inline;
 * every write goes through a function below. Pointer arguments are live and
 * unshared. run/resume/yield/bailout are whole operations that call the swap
 * callbacks and the hooks, which take a swoole::Coroutine facade. */
typedef struct swr_v1_coroutine swr_v1_coroutine;
struct swr_v1_coroutine {
    void *native; /* the swoole::Coroutine facade */
    long cid;
    int32_t state, resume_code;
    void *task, *cancel_fn;
    swr_v1_coroutine *origin, *first_child, *prev_sibling, *next_sibling;
    long init_msec, switch_usec, execute_usec;
};
/* A yield's cancel function (cancel_fn points to one): call(native, data) marks
 * the coroutine canceled (swr_v1_coroutine_cancel) once nothing can fail, then
 * wakes it and returns whether it did. */
typedef struct {
    bool (*call)(void *native, void *data);
    void *data;
} swr_v1_coroutine_canceler;
typedef void (*swr_v1_coroutine_swap_function)(void *);
typedef struct {
    bool (*swap_in)(void *); /* returns whether the coroutine ended */
    void (*swap_out)(void *);
    void (*destroy)(void *); /* deletes the ended facade */
    void (*bailout)(void);   /* calls on_bailout */
    long (*clock)(void);     /* SW_CORO_TIME's clock, or null */
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
/* register returns whether to activate the scheduler. */
bool swr_v1_coroutine_register(swr_v1_coroutine_scheduler *, swr_v1_coroutine *, void *native, long init_msec, long now);
long swr_v1_coroutine_run(swr_v1_coroutine_scheduler *, swr_v1_coroutine *);
void swr_v1_coroutine_resume(swr_v1_coroutine_scheduler *, swr_v1_coroutine *);
void swr_v1_coroutine_yield(swr_v1_coroutine_scheduler *, swr_v1_coroutine *);
void swr_v1_coroutine_yield_cancelable(swr_v1_coroutine_scheduler *, swr_v1_coroutine *, swr_v1_coroutine_canceler *);
void swr_v1_coroutine_bailout(swr_v1_coroutine_scheduler *);
/* The live coroutines; the void pointers are swoole::Coroutine facades. get:
 * null if cid is not live. older: the newest live coroutine older than `before`
 * (LONG_MAX starts a walk), its id in *cid, or null. print_list prints them to
 * `out`. */
void *swr_v1_coroutine_get(const swr_v1_coroutine_scheduler *, long cid);
size_t swr_v1_coroutine_count(const swr_v1_coroutine_scheduler *);
void *swr_v1_coroutine_older(const swr_v1_coroutine_scheduler *, long before, long *cid);
void swr_v1_coroutine_iterator_reset(swr_v1_coroutine_scheduler *);
void *swr_v1_coroutine_iterator_each(swr_v1_coroutine_scheduler *);
void swr_v1_coroutine_print_list(const swr_v1_coroutine_scheduler *, FILE *out);
/* set_active also clears (true) or sets (false) the bailout mark. */
void swr_v1_coroutine_set_active(swr_v1_coroutine_scheduler *, bool);
void swr_v1_coroutine_set_stack_size(swr_v1_coroutine_scheduler *, size_t);
void swr_v1_coroutine_bind_socket(swr_v1_coroutine_scheduler *, long cid);
void swr_v1_coroutine_set_callback(swr_v1_coroutine_scheduler *, uint32_t slot, swr_v1_coroutine_swap_function);
void swr_v1_coroutine_set_task(swr_v1_coroutine *, void *);
void swr_v1_coroutine_cancel(swr_v1_coroutine *);
void swr_v1_coroutine_timeout(swr_v1_coroutine *);
long swr_v1_coroutine_execute_usec(const swr_v1_coroutine *, long now);
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
void swr_v1_thread_context_clean(void);
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
int swr_v1_context_init(swr_v1_context *, size_t stack_size, uint32_t flags, void (*entry)(void *), void *data);
void swr_v1_context_free(swr_v1_context *);
void swr_v1_context_swap_in(swr_v1_context *);
void swr_v1_context_swap_out(swr_v1_context *);
#ifdef __cplusplus
}
#endif
#endif
