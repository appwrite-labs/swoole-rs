#ifndef SWOOLE_RS_PHPCOROUTINES_H
#define SWOOLE_RS_PHPCOROUTINES_H
#include <stddef.h>
#include <stdint.h>
#ifdef __cplusplus
extern "C" {
#endif
/* PHP coroutines (Rust, crates/phpcoroutines): each coroutine's PHP context
 * and VM swap, activation, the preemptive scheduler, Swoole\Coroutine's
 * methods, go()/defer(), the exit and silence handlers and the autoload hook.
 * C++ lends the coroutine core's operations; Rust's zim_swoole_coroutine_* and
 * zif_* are the handlers its tables name. A coroutine's context is opaque. */
struct _zend_function_entry;
struct _zend_class_entry;
struct _zend_execute_data;
struct _zend_fcall_info_cache;
struct _zval_struct;

/* The globals and server state the frontend reads (bits of `flags`). */
enum {
    SWR_PHPCOROUTINES_CLI = 1,
    SWR_PHPCOROUTINES_SHORTNAME = 2,
    SWR_PHPCOROUTINES_LIBRARY = 4,
    SWR_PHPCOROUTINES_FIBER_MOCK = 8,
    SWR_PHPCOROUTINES_PREEMPTIVE = 16,
    SWR_PHPCOROUTINES_SHUTDOWN_FUNCTIONS = 32,
    SWR_PHPCOROUTINES_SERVER_STARTED = 64,
    SWR_PHPCOROUTINES_MAIN_THREAD = 128,
    SWR_PHPCOROUTINES_FATAL = 256,
};

struct swr_v1_phpcoroutines_stats {
    int64_t event_num;
    int64_t signal_listener_num;
    int64_t aio_task_num;
    int64_t aio_worker_num;
    int64_t aio_queue_size;
    int64_t c_stack_size;
    int64_t coroutine_num;
    int64_t coroutine_peak_num;
    int64_t coroutine_last_cid;
    uint8_t iouring;
    int64_t iouring_task_num;
    int64_t iouring_sq_usage_percent;
    int64_t iouring_waiting_task_num;
};

struct swr_v1_phpcoroutines_ops {
    uint32_t (*flags)(void);
    /* Coroutine statics */
    void *(*current)(void);
    /* get_current_safe(): a fatal error outside a coroutine */
    void *(*current_safe)(void);
    void *(*current_task)(void);
    long (*current_cid)(void);
    void *(*task_by_cid)(long cid);
    void *(*by_cid)(long cid);
    /* Coroutine::create(main_func, args) */
    long (*create)(void *args);
    size_t (*count)(void);
    long (*elapsed)(long cid);
    long (*execute_time)(long cid);
    size_t (*list)(long *cids, size_t capacity);
    void (*stats)(struct swr_v1_phpcoroutines_stats *stats);
    /* swoole_test_kernel_coroutine's coroutine */
    void (*kernel_coroutine)(long count, double sleep);
    /* one coroutine */
    void (*set_task)(void *co, void *task);
    /* its origin's task, null for the main context */
    void *(*origin_task)(void *co);
    long (*cid)(void *co);
    long (*origin_cid)(void *co);
    int (*state)(void *co);
    /* its C stack, or null under thread contexts */
    void *(*stack)(void *co, size_t *size);
    void (*yield)(void *co);
    void (*yield_cancelable)(void *co, uint8_t (*cancel)(void *co));
    uint8_t (*yield_ex)(void *co, double timeout);
    void (*resume)(void *co);
    uint8_t (*cancel)(void *co);
    uint8_t (*is_canceled)(void *co);
    uint8_t (*is_end)(void *co);
    /* activation: the reactor; its destroy callback, Coroutine::activate and
     * the switch callbacks; hooks and unsafe functions */
    void (*check_reactor)(void);
    void (*attach)(void (*on_yield)(void *), void (*on_resume)(void *), void (*on_close)(void *));
    void (*detach)(void);
    void (*deactivate)(void);
    void (*hook)(uint32_t hook_flags);
    void (*unhook)(void);
    void (*signal_block_all)(void);
    /* bodies for swr_zend_try: they run PHP */
    void (*load_library)(void *arg);
    void (*deadlock_check)(void *arg);
    /* SW_GLOBAL_HOOK_ON_CORO_START or _STOP, when set */
    void (*coro_hook)(uint8_t stop, void *task);
    long (*absolute_msec)(void);
    void (*defer)(void (*fn)(void *), void *data);
    void *(*timer_add)(double timeout, long cid, void (*fn)(long cid));
    void (*timer_del)(void *timer);
    void (*set_last_error)(int code);
    /* php_swoole_fatal_error(level, "%s", message) */
    void (*fatal_error)(int level, const char *message);
    /* php_swoole_error_ex(level, code, "%s", message) */
    void (*error_ex)(int level, int code, const char *message);
    /* PHP 8.4+ exit(): the original handler (0 if none), and its fallback */
    uint8_t (*exit_original)(struct _zend_execute_data *execute_data, struct _zval_struct *return_value);
    void (*exit_fallback)(const char *message, size_t length, long status);
};

/* MINIT: the classes, constants, opcode handlers and the autoload hook. */
void swr_v1_phpcoroutines_minit(const struct _zend_function_entry *coroutine_methods,
                                const struct _zend_function_entry *exit_exception_methods, int32_t module_number,
                                struct _zend_class_entry *exception, struct _zend_class_entry *array_iterator,
                                struct _zend_class_entry *array_object,
                                const struct swr_v1_phpcoroutines_ops *ops);
void swr_v1_phpcoroutines_rinit(void);
/* 1 if deactivation bailed out, which the caller re-raises. */
uint8_t swr_v1_phpcoroutines_rshutdown(void);

/* PHPCoroutine::create: the new cid or Coroutine::ERR_*; 1 in *bailout when
 * activation bailed out, which the caller re-raises. */
long swr_v1_phpcoroutines_create(struct _zend_fcall_info_cache *fcc, uint32_t argc, struct _zval_struct *argv,
                                 struct _zval_struct *callable, uint8_t *bailout);
/* The reactor's destroy callback: 1 if the deadlock check bailed out. */
uint8_t swr_v1_phpcoroutines_deactivate(void);

/* A coroutine's entry, from the C++ main function that traps its bailouts:
 * open the context (create_context), enter it (the start hook and fiber
 * notification; returns its zend::Function), pop its defer tasks, close it
 * (destroy_context). */
void *swr_v1_phpcoroutines_open(void *args);
void *swr_v1_phpcoroutines_enter(void *context);
void *swr_v1_phpcoroutines_defer_next(void *context);
void swr_v1_phpcoroutines_close(void *context);

long swr_v1_phpcoroutines_cid(void);
/* PHPCoroutine::config */
uint32_t swr_v1_phpcoroutines_hook_flags(void);
void swr_v1_phpcoroutines_set_hook_flags(uint32_t flags);
void swr_v1_phpcoroutines_set_max_num(uint64_t max);
void swr_v1_phpcoroutines_set_deadlock_check(uint8_t enable);
void swr_v1_phpcoroutines_set_preemptive(uint8_t enable);
uint8_t swr_v1_phpcoroutines_activated(void);
/* The tracer's blocking detection: with `watch`, count the switches of
 * `task` (a coroutine's context) from 0; without, stop counting those of the
 * running context (null `task`). Returns the count. */
size_t swr_v1_phpcoroutines_watch_switches(void *task, uint8_t watch);
/* sw_php_print_backtrace: 1 if output bailed out. */
uint8_t swr_v1_phpcoroutines_print_backtrace(long cid, long options, long limit, struct _zval_struct *return_value);
#ifdef __cplusplus
}
#endif
#endif
