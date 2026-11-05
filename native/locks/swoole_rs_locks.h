#ifndef SWOOLE_RS_LOCKS_H
#define SWOOLE_RS_LOCKS_H
#ifndef SWOOLE_RS_LOCKS
#error "Locks overlay requires --enable-swoole-rs-locks"
#endif
#if defined(__APPLE__)
#include <TargetConditionals.h>
#if !TARGET_OS_OSX
#error "Rust locks require validated macOS or Linux platform contracts"
#endif
#elif !defined(__linux__) || defined(__ANDROID__)
#error "Rust locks require validated macOS or Linux platform contracts"
#endif
#include <stdint.h>
#include <stddef.h>
#ifdef __cplusplus
extern "C" {
#endif
/* kind is pinned Lock::Type: RW=1, mutex=3, spin=5. Caller supplies size/alignment. */
size_t swr_v1_locks_size(int32_t kind);
size_t swr_v1_locks_align(int32_t kind);
int32_t swr_v1_locks_init(void *, int32_t kind, int32_t shared);
int32_t swr_v1_locks_lock(void *, int32_t kind, int32_t operation, int32_t timeout, uint32_t capabilities);
int32_t swr_v1_locks_unlock(void *, int32_t kind);
void swr_v1_locks_destroy(void *, int32_t kind);
/* operation: load=0, store=1, add-fetch=2, sub-fetch=3, CAS=4. */
uint32_t swr_v1_locks_atomic32(uint32_t *, uint32_t operation, uint32_t value, uint32_t expected);
int64_t swr_v1_locks_atomic64(int64_t *, uint32_t operation, int64_t value, int64_t expected);
void swr_v1_locks_barrier_init(uint32_t *count, uint32_t *arrived, uint32_t parties);
void swr_v1_locks_barrier_wait(uint32_t *count, uint32_t *arrived);
int32_t swr_v1_locks_pthread_barrier(void *, size_t, void *, size_t, uint32_t operation, int32_t shared, uint32_t parties);
#ifdef __linux__ /* matches the Rust cfg(target_os = "linux") */
/* Futex wait/wake; non-futex builds use upstream's C++ registry. */
int32_t swr_v1_locks_wait(uint32_t *, double timeout);
int32_t swr_v1_locks_wakeup(uint32_t *, int32_t n);
#endif
int32_t swr_v1_locks_coroutine_try(uint32_t *, uintptr_t held_cookie, int64_t held_cid, uintptr_t current_cookie, int64_t current_cid, uint8_t blocking);
int32_t swr_v1_locks_coroutine_release(uint32_t *);
double swr_v1_locks_coroutine_backoff(double second, double initial, double maximum);
/* Complete coroutine owner. The immutable host lives for the process; only
 * scheduler/pool leaves cross the native boundary. */
typedef struct SwrCoroutineLock SwrCoroutineLock;
typedef struct { int32_t code; uint8_t bailed; } SwrCoroutineOutcome;
typedef struct {
    void *(*current)(void);
    int64_t (*cid)(void *);
    uint32_t *(*alloc_shared)(void);
    void (*free_shared)(uint32_t *);
    SwrCoroutineOutcome (*sleep)(double);
    SwrCoroutineOutcome (*wait)(uint32_t *);
    SwrCoroutineOutcome (*wake)(uint32_t *);
    uint8_t (*warning)(void);
} SwrCoroutineHost;
SwrCoroutineLock *swr_v1_locks_coroutine_new(uint8_t shared, const SwrCoroutineHost *);
void swr_v1_locks_coroutine_free(SwrCoroutineLock *);
SwrCoroutineOutcome swr_v1_locks_coroutine_lock(SwrCoroutineLock *, int32_t operation);
SwrCoroutineOutcome swr_v1_locks_coroutine_unlock(SwrCoroutineLock *);
const SwrCoroutineHost *swr_cpp_coroutine_lock_native_host(void);
void swr_v1_phplocks_coroutine_minit(const SwrCoroutineHost *, void (*unconstructed)(void *));
/* PHP Atomic classes (crates/phplocks). Rust registers the classes with its
 * method tables (the stubs' arginfo, crates/phpstubs), and the methods are its
 * zim_swoole_*atomic* exports. Atomic words come from `alloc`/`release` (the
 * global pool). */
struct _zend_class_entry;
void swr_v1_phplocks_atomic_minit(struct _zend_class_entry *exception,
                                  void *(*alloc)(uint32_t size),
                                  void (*release)(void *word));
/* Swoole\Lock: `kinds` has bit 1 << Lock::Type per lock type the build has;
 * `capabilities` is what swr_v1_locks_lock takes; `unconstructed` reports a
 * method called before the constructor (a fatal error). */
void swr_v1_phplocks_lock_minit(int32_t module_number,
                                uint32_t kinds, uint32_t capabilities,
                                struct _zend_class_entry *exception,
                                void *(*alloc)(uint32_t size), void (*unconstructed)(void));
#ifdef SW_THREAD /* config.h: ZTS builds with thread support */
/* Swoole\Thread\Atomic, Atomic\Long and Lock, whose resources Thread\Map,
 * ArrayList and Queue share. Thread\Lock takes Swoole\Lock's types, so its MINIT
 * follows swr_v1_phplocks_lock_minit; `unconstructed` is
 * php_swoole_fatal_error(E_ERROR, "%s", message). */
void swr_v1_phplocks_thread_atomic_minit(void);
void swr_v1_phplocks_thread_lock_minit(void (*unconstructed)(const char *message));
#endif
#ifdef __cplusplus
}
#endif
#endif

