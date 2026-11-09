#ifndef SWOOLE_RS_PHPCOROUTINES_H
#define SWOOLE_RS_PHPCOROUTINES_H
#include <stdbool.h>
#include <stddef.h>
#include <stdint.h>
#include <sys/types.h>
#ifdef __cplusplus
extern "C" {
#endif
/* PHP coroutines (Rust, crates/phpcoroutines): each coroutine's PHP context
 * and VM swap, activation, the preemptive scheduler, Swoole\Coroutine's
 * methods, go()/defer(), the exit and silence handlers and the autoload hook.
 * php_swoole_coroutine_{minit,rinit,rshutdown} are Rust; native/phpcoroutines
 * lends what needs C++ (coroutine.cc: std::function members and the
 * Context). Rust's zim_swoole_coroutine_* and zif_* are the handlers its
 * tables name. A coroutine's context is opaque. */
struct _zend_class_entry;
struct _zend_execute_data;
struct _zend_fcall_info_cache;
struct _zval_struct;
struct _zend_array;
struct _zend_string;

/* PHPCoroutine::create: the new cid or Coroutine::ERR_*; 1 in *bailout when
 * activation bailed out, which the caller re-raises. */
long swr_v1_phpcoroutines_create(struct _zend_fcall_info_cache *fcc, uint32_t argc, struct _zval_struct *argv,
                                 struct _zval_struct *callable, uint8_t *bailout);
long swr_v1_phpcoroutines_cid(void);
/* PHPCoroutine::config */
uint32_t swr_v1_phpcoroutines_hook_flags(void);
void swr_v1_phpcoroutines_set_hook_flags(uint32_t flags);
/* PHPCoroutine::set_hook_flags: the flags, recorded in the options too. */
void swr_v1_phpcoroutines_set_hook_option(uint32_t flags);
void swr_v1_phpcoroutines_set_preemptive(uint8_t enable);
/* The tracer's blocking detection: with `watch`, count the switches of
 * `task` (a coroutine's context) from 0; without, stop counting those of the
 * running context (null `task`). Returns the count. */
size_t swr_v1_phpcoroutines_watch_switches(void *task, uint8_t watch);
/* The tracer, at MINIT with its PHP_INI_SYSTEM switches (crates/phpswoole calls
 * it): observes calls when profiling or blocking detection is on. Rust's
 * zif_swoole_tracer_* are the functions' handlers. */
void swr_v1_phpcoroutines_tracer_minit(int64_t blocking_threshold, bool blocking_detection, bool profile,
                                       bool leak_detection);
void swr_v1_phpcoroutines_tracer_rinit(void);
void swr_v1_phpcoroutines_tracer_rshutdown(void);
/* sw_php_print_backtrace: 1 if output bailed out. */
uint8_t swr_v1_phpcoroutines_print_backtrace(long cid, long options, long limit, struct _zval_struct *return_value);
#ifdef __cplusplus
}
#endif
#endif
