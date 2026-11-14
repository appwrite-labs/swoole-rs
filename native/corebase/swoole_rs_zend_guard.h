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
 * scope. The return value is a swr_zend_guard_kind. This ABI does not contain
 * forced unwinding or pthread cancellation across Rust. PHP coroutine cancel
 * is a distinct, cooperatively resumed PHP operation. */
int swr_zend_try(void (*body)(void *), void *argument);
typedef struct swr_zend_guard {
    volatile struct swr_zend_guard *previous;
    void *context;
    void *target;
    void *original_bailout;
} swr_zend_guard;
void swr_zend_guard_register(void *(*context)(void), void (*resume)(void));
void swr_zend_guard_enter(volatile swr_zend_guard *, void *target);
void swr_zend_guard_leave(volatile swr_zend_guard *);
void swr_zend_guard_scope_cleanup(volatile swr_zend_guard *);
void *swr_zend_guard_target(void);
int swr_zend_guard_pending(void);
void swr_zend_guard_mark(void);
void swr_zend_guard_clear(void);
void swr_zend_guard_resume(void);
#ifdef __cplusplus
}
#endif
/* C shims compile with -fexceptions so native C++ unwinding runs cleanup.
 * No C++ exception may propagate through an active Rust caller.
 * Only Swoole\Thread marks a whole-thread exit, and it needs ZTS: on NTS a
 * scope is a plain zend_try, with no record and nothing pending. */
#ifdef ZTS
#define SWR_ZEND_EXIT_PENDING() swr_zend_guard_pending()
/* Zend longjmp does not run compiler cleanup: unlink explicitly on both arms. */
#define SWR_ZEND_TRY                                                               \
    { volatile swr_zend_guard swr_guard                                            \
        __attribute__((cleanup(swr_zend_guard_scope_cleanup))) = {0};              \
      swr_guard.original_bailout = EG(bailout); zend_try {                          \
        swr_zend_guard_enter(&swr_guard, EG(bailout)); if (!swr_zend_guard_pending())
#define SWR_ZEND_CATCH                                                             \
        if (swr_zend_guard_pending()) { zend_bailout(); }                           \
        swr_zend_guard_leave(&swr_guard); } zend_catch
#define SWR_ZEND_END_TRY()                                                         \
    swr_zend_guard_leave(&swr_guard); zend_end_try(); }
#else
#define SWR_ZEND_EXIT_PENDING() 0
#define SWR_ZEND_TRY { zend_try {
#define SWR_ZEND_CATCH } zend_catch
#define SWR_ZEND_END_TRY() zend_end_try(); }
#endif
#endif
