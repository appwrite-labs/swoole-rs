#ifndef SWOOLE_RS_ZEND_GUARD_H
#define SWOOLE_RS_ZEND_GUARD_H
/* Native effect scopes. Records live on a native stack until leave(), including
 * while that coroutine is suspended. No Rust owner may be active below a target.
 * A typed whole-thread exit remains pending until its native request boundary. */
#ifdef __cplusplus
extern "C" {
#endif
enum swr_zend_guard_kind { SWR_ZEND_RETURN = 0, SWR_ZEND_BAILOUT = 1, SWR_ZEND_THREAD_EXIT = 2 };
/* body must be native and contain C++ exceptions before returning to this C
 * scope. The return value is a swr_zend_guard_kind. */
int swr_zend_try(void (*body)(void *), void *argument);
typedef struct swr_zend_guard {
    volatile struct swr_zend_guard *previous;
    void *context;
    void *target;
} swr_zend_guard;
void swr_zend_guard_register(void *(*context)(void), void (*resume)(void));
void swr_zend_guard_enter(volatile swr_zend_guard *, void *target);
void swr_zend_guard_leave(volatile swr_zend_guard *);
void *swr_zend_guard_target(void);
int swr_zend_guard_pending(void);
void swr_zend_guard_mark(void);
void swr_zend_guard_clear(void);
void swr_zend_guard_resume(void);
#ifdef __cplusplus
}
#endif
/* Zend's native jump buffer is established before registration. Volatile keeps
 * records defined after setjmp returns through longjmp. A normally returning
 * coroutine effect still reports a pending whole-thread exit to its Rust caller. */
#define SWR_ZEND_TRY                                                               \
    { volatile swr_zend_guard swr_guard; zend_try {                                 \
        swr_zend_guard_enter(&swr_guard, EG(bailout)); if (!swr_zend_guard_pending())
#define SWR_ZEND_CATCH                                                             \
        if (swr_zend_guard_pending()) { zend_bailout(); }                           \
    } zend_catch
#define SWR_ZEND_END_TRY()                                                         \
    zend_end_try(); swr_zend_guard_leave(&swr_guard); }
#endif
