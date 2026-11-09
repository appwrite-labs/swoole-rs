/* SPDX-License-Identifier: Apache-2.0 */
/* Swoole\Timer is Rust (crates/phptimer); C++ only clears its timers. */
#ifndef SWOOLE_RS_PHPTIMER_H
#define SWOOLE_RS_PHPTIMER_H
#include <stdint.h>
#ifdef __cplusplus
extern "C" {
#endif
/* php_swoole_timer_clear_all(): remove every PHP timer; 0 if there is no timer. */
uint8_t swr_v1_phptimer_clear_all(void);
#ifdef __cplusplus
}
#endif
#endif
