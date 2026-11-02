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

/* Caller-local cursor. Native publishes one action and refreshes facts after
 * every timer/queue/callback operation. No borrow crosses native execution. */
typedef struct { uint32_t phase; } swr_v1_channel_cursor;
enum swr_channel_operation { SWR_CH_PUSH = 0, SWR_CH_POP = 1, SWR_CH_CLOSE = 2 };
enum swr_channel_fact {
    SWR_CH_CLOSED = 1, SWR_CH_EMPTY = 2, SWR_CH_FULL = 4,
    SWR_CH_PRODUCERS = 8, SWR_CH_CONSUMERS = 16, SWR_CH_POSITIVE_TIMEOUT = 32,
    SWR_CH_TIMER = 64, SWR_CH_CANCELED = 128, SWR_CH_TIMEDOUT = 256
};
enum swr_channel_action {
    SWR_CH_DONE = 0, SWR_CH_ARM_TIMER = 1, SWR_CH_YIELD = 2,
    SWR_CH_DELETE_TIMER = 3, SWR_CH_TRANSFER = 4, SWR_CH_WAKE_PRODUCER = 5,
    SWR_CH_WAKE_CONSUMER = 6, SWR_CH_PUBLISH_CLOSED = 7,
    SWR_CH_ERROR_TIMEOUT = -1, SWR_CH_ERROR_CLOSED = -2,
    SWR_CH_ERROR_CANCELED = -3, SWR_CH_INVALID = -4
};
int32_t swr_v1_channel_step(swr_v1_channel_cursor *, uint32_t op, uint32_t facts);
/* Sole pointer-token FIFOs. Native owns payloads and translates allocation
 * failures after return. Each call requires a live, exclusively accessed owner. */
typedef struct swr_v1_channel_tokens swr_v1_channel_tokens;
swr_v1_channel_tokens *swr_v1_channel_tokens_new(void);
swr_v1_channel_tokens *swr_v1_channel_tokens_clone(const swr_v1_channel_tokens *);
void swr_v1_channel_tokens_free(swr_v1_channel_tokens *);
size_t swr_v1_channel_tokens_len(const swr_v1_channel_tokens *);
int32_t swr_v1_channel_tokens_push(swr_v1_channel_tokens *, void *);
int32_t swr_v1_channel_tokens_front(const swr_v1_channel_tokens *, void **);
void swr_v1_channel_tokens_pop(swr_v1_channel_tokens *);
void swr_v1_channel_tokens_remove(swr_v1_channel_tokens *, void *);
/* Whole inline Channel metadata; passive initialized POD copy is allowed.
 * Native Channel prefix/queue offsets are statically qualified in its header.
 * State/three queue handles are disjoint and initialized; no pointee or effect
 * enters Rust. All borrows end before Native logging/timer/yield/resume. */
typedef struct { size_t capacity; uint8_t closed; int32_t error; } swr_v1_channel_state;
typedef struct { size_t length, producers, consumers; int32_t error;
                 uint8_t closed, full, empty; } swr_v1_channel_view;
typedef struct { swr_v1_channel_cursor step; uint32_t pending; } swr_v1_channel_owned_cursor;
typedef struct { int32_t action; void *token; } swr_v1_channel_owned_action;
enum { SWR_CH_ALLOCATION_REFUSED = -5 };
swr_v1_channel_state swr_v1_channel_state_new(size_t capacity);
swr_v1_channel_view swr_v1_channel_view_read(const swr_v1_channel_state *,
    const swr_v1_channel_tokens *data, const swr_v1_channel_tokens *producers,
    const swr_v1_channel_tokens *consumers);
void *swr_v1_channel_pop_data(swr_v1_channel_tokens *data);
/* Complete waiter operation: enqueue/pop select producer only for role1;
 * remove selects consumer only for role2. Removal erases ALL matching identities.
 * Native retains tracing, timers, callbacks, yield/resume and bad_alloc effects. */
typedef struct { int32_t status; uint32_t role; void *token; } swr_v1_channel_waiter_result;
enum { SWR_CH_WAITER_ENQUEUE = 0, SWR_CH_WAITER_REMOVE = 1, SWR_CH_WAITER_POP = 2 };
swr_v1_channel_waiter_result swr_v1_channel_waiter(swr_v1_channel_tokens *producers,
    swr_v1_channel_tokens *consumers, uint32_t operation, uint32_t role, void *token);
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
/* Cursor zero-seeded, fixed operation; only external timer/cancel/timeout bits.
 * PUBLISH_CLOSED requests Native trace, then next entry publishes closed before
 * queue observation. Terminal wake must never reenter or access Channel again.
 * Allocation refusal is translated to Native bad_alloc only after return. */
swr_v1_channel_owned_action swr_v1_channel_owned_step(swr_v1_channel_owned_cursor *,
    swr_v1_channel_state *, swr_v1_channel_tokens *data,
    swr_v1_channel_tokens *producers, swr_v1_channel_tokens *consumers,
    uint32_t op, uint32_t external, void *input);
/* Whole pop/push/close, driving the step machine to completion and performing
 * each Native effect through `effects`: `query` returns fresh CANCELED/TIMEDOUT
 * bits, `suspend` enqueues the current coroutine and yields (returning
 * SWR_CH_ALLOCATION_REFUSED instead of raising), `arm_timer`/`delete_timer` drive
 * the timeout, and `resume` wakes a token. `status` 0 succeeded (`data` is the
 * popped token for a pop), negative is a SWR_CH_ERROR_*, and -5 means Native must
 * raise bad_alloc after return. close uses only `resume`. */
typedef struct {
    void *ctx;
    uint32_t (*query)(void *);
    void (*arm_timer)(void *);
    void (*delete_timer)(void *);
    int32_t (*suspend)(void *);
    void (*resume)(void *, void *);
} swr_v1_channel_effects;
typedef struct { int32_t status; void *data; } swr_v1_channel_outcome;
swr_v1_channel_outcome swr_v1_channel_pop(swr_v1_channel_state *, swr_v1_channel_tokens *data,
    swr_v1_channel_tokens *producers, swr_v1_channel_tokens *consumers,
    bool positive_timeout, const swr_v1_channel_effects *);
swr_v1_channel_outcome swr_v1_channel_push(swr_v1_channel_state *, swr_v1_channel_tokens *data,
    swr_v1_channel_tokens *producers, swr_v1_channel_tokens *consumers,
    bool positive_timeout, void *input, const swr_v1_channel_effects *);
swr_v1_channel_outcome swr_v1_channel_close(swr_v1_channel_state *, swr_v1_channel_tokens *data,
    swr_v1_channel_tokens *producers, swr_v1_channel_tokens *consumers,
    const swr_v1_channel_effects *);
/* Native-owned stack bytes, no retained pointer or native effects. Only marker
 * spans are written/read; usage requires those spans already initialized. */
void swr_v1_context_stack_mark(uint8_t *stack, size_t size);
size_t swr_v1_context_stack_usage(const uint8_t *stack, size_t size);
#ifdef __cplusplus
}
static_assert(sizeof(swr_v1_channel_cursor) == 4, "channel cursor ABI");
#endif
#endif
