#ifndef SWOOLE_RS_UTILITIES_H
#define SWOOLE_RS_UTILITIES_H
#include <stdint.h>
#include <stddef.h>
#include <stdbool.h>
#ifdef __cplusplus
extern "C" {
#endif
/* Native-compatible aligned shared u32 atomic, native release store remains outside Rust. */
void swr_v1_utils_spinlock(const uint32_t *, uint32_t cpus, uint32_t backoff_limit);
#ifdef __linux__ /* matches the Rust cfg(target_os = "linux") */
/* Futex wait/wake on a shared aligned native u32 atomic.
 * Caller owns lifetime; no callback or allocation; preserves native errno. */
int32_t swr_v1_utils_wait(const uint32_t *, double timeout);
int32_t swr_v1_utils_wakeup(const uint32_t *, int32_t n);
#endif
#ifdef __cplusplus
}
#endif
#endif
