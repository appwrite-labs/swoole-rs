/* SPDX-License-Identifier: Apache-2.0 */
#ifndef SWOOLE_RS_TIMER_RUNTIME_H
#define SWOOLE_RS_TIMER_RUNTIME_H
#include <stdint.h>
#ifdef __cplusplus
extern "C" {
#endif
/* Stateless LP64 clock/backend facade; no callback, allocation or errno change.
 * Native code retains clock acquisition, signals and public layouts.
 * Original defined signed-arithmetic/conversion ranges remain the parity scope. */
typedef struct { int64_t seconds, fraction; } swr_timer_native_time;
/* manual=0, reactor=1, system=2; inputs are copied native booleans. */
uint32_t swr_v1_timer_backend(uint32_t manual, uint32_t reactor);
/* setitimer(ITIMER_REAL) every milliseconds, zero/negative disarm: 0, or -1 with errno. */
int swr_v1_timer_system_set(int64_t milliseconds);
int64_t swr_v1_timer_relative_msec(int64_t absolute, int64_t base);
int64_t swr_v1_timer_rearm_after_add(int64_t current, int64_t delay);
/* Valid independent deadline output; 1 writes result, 0 leaves output unchanged. */
int32_t swr_v1_timer_delay_deadline(int64_t now, int64_t previous, int64_t delay, int64_t *deadline);
#ifdef __cplusplus
}
#endif
#endif
