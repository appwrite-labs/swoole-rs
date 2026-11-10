#ifndef SWOOLE_RS_LOCKS_H
#define SWOOLE_RS_LOCKS_H
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
#ifdef __linux__ /* matches the Rust cfg(target_os = "linux") */
/* Futex wait/wake; non-futex builds use upstream's C++ registry. */
int32_t swr_v1_locks_wait(uint32_t *, double timeout);
int32_t swr_v1_locks_wakeup(uint32_t *, int32_t n);
#endif
int32_t swr_v1_locks_coroutine_try(uint32_t *, uintptr_t held_cookie, int64_t held_cid, uintptr_t current_cookie, int64_t current_cid, uint8_t blocking);
int32_t swr_v1_locks_coroutine_release(uint32_t *);
double swr_v1_locks_coroutine_backoff(double second, double initial, double maximum);
/* PHP Swoole\Atomic, Swoole\Lock and Swoole\Coroutine\Lock are Rust
 * (crates/phplocks), MINITs included. */
#ifdef SW_THREAD /* config.h: ZTS builds with thread support */
/* Swoole\Thread\Atomic, Atomic\Long and Lock, whose resources Thread\Map,
 * ArrayList and Queue share. */
void swr_v1_phplocks_thread_atomic_minit(void);
#endif
#ifdef __cplusplus
}
#endif
#endif

