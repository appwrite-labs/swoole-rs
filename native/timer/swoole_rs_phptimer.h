/* SPDX-License-Identifier: Apache-2.0 */
/* Swoole\Timer (Rust, crates/phptimer): C++ lends the timer's operations; Rust
 * registers the classes, functions and constants, and its zif_swoole_timer_*
 * handlers are the method table's. The C++ callback and destructor trampolines
 * call swr_v1_phptimer_call/free and re-raise a bailout they report (1). */
#ifndef SWOOLE_RS_PHPTIMER_H
#define SWOOLE_RS_PHPTIMER_H
#include <stddef.h>
#include <stdint.h>
#ifdef __cplusplus
extern "C" {
#endif
struct _zend_class_entry;
struct swr_v1_phptimer_node {
    int64_t exec_msec;
    uint64_t exec_count;
    int64_t interval;
    uint64_t round;
    uint8_t php;
    uint8_t removed;
};
struct swr_v1_phptimer_invocation {
    void *callable;
    uint8_t ok;
};
struct swr_v1_phptimer_ops {
    uint8_t (*available)(void);
    int64_t (*add)(int64_t ms, uint8_t persistent, void *callable);
    uint8_t (*get)(int64_t id, struct swr_v1_phptimer_node *node);
    void (*stats)(size_t *count, uint64_t *round);
    size_t (*ids)(int64_t *ids, size_t capacity);
    uint8_t (*clear)(int64_t id); /* swoole_timer_clear's lookup and del of a PHP timer */
    uint8_t (*clear_all)(void);
    void (*call)(void *invocation);
    void (*fatal_warning)(const char *message);
    void (*warning)(const char *message);
};
void swr_v1_phptimer_minit(int32_t module_number,
                           struct _zend_class_entry *array_iterator, const struct swr_v1_phptimer_ops *ops);
uint8_t swr_v1_phptimer_call(void *callable);
uint8_t swr_v1_phptimer_free(void *callable);
#ifdef __cplusplus
}
#endif
#endif
