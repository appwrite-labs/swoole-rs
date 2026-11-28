/* Callables kept past a call (timer and event callbacks), and Swoole's global
 * function aliases of static methods. Included by shim.h for bindgen. */
#ifndef SWOOLE_RS_ZEND_CALLABLE_H
#define SWOOLE_RS_ZEND_CALLABLE_H
#include "php.h"
#include "zend_closures.h"

/* Swoole's zend::Function, whose layout it shares. */
typedef struct {
    zend_fcall_info fci;
    zend_fcall_info_cache fcc;
} swr_zend_callable;
/* sw_zend_fci_cache_persist: hold the bound object and closure. */
void swr_zend_callable_hold(swr_zend_callable *callable);
/* sw_zend_fci_params_discard, then sw_zend_fci_cache_discard; may run PHP
 * destructors, so Rust runs it under swr_zend_try. */
void swr_zend_callable_release(void *callable);
/* swr_zend_callable_release when it provably runs no PHP code: no arguments,
 * and each held object stays referenced and is already a GC root or not
 * collectable, so releasing it only drops a reference. 1 if released, 0 if
 * nothing was touched (use the trapped release). */
int swr_zend_callable_release_quiet(swr_zend_callable *callable);
/* zval_ptr_dtor of a zval, as a body for swr_zend_try (it may run destructors). */
void swr_zend_zval_release(void *value);

#endif
