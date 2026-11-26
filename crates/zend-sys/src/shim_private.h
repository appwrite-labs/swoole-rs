#ifndef SWOOLE_RS_ZEND_SHIM_PRIVATE_H
#define SWOOLE_RS_ZEND_SHIM_PRIVATE_H
#include "php.h"
#include "zend_closures.h"
#include "zend_exceptions.h"
#include "shim_values.h"
/* Individual Zend property effects, each containing possible PHP magic-method bailouts. */
zend_object *swr_zend_facade_exception(void);
/* Keep UnwindExit in EG(exception) for the VM. Report other exceptions inside zend_try. */
int swr_zend_try_uncaught_exception(bool *unwind_exit);
/* call_user_function(EG(function_table), NULL, name, retval, args) inside zend_try:
 * 1 if it bailed out, else *result is its result. */
int swr_zend_try_facade_user_call(zval *name, swr_zvals args, zval *retval, int *result);
/* Work after a coroutine call, inside its zend_try (zend::host::Then). */
typedef void (*swr_zend_then)(void *);
int swr_zend_try_facade_coroutine(long (*)(zend_fcall_info_cache *, uint32_t, zval *, zval *, uint8_t *),
                                  zend_fcall_info_cache *, uint32_t, zval *, zval *, long *,
                                  swr_zend_then, void *, bool *);
HashTable *swr_zend_facade_function_table(void);
/* sw_zend_create_object_deny's body: a bare object of `ce` (its declared properties
 * undefined), then the "can not be created" Error inside zend_try (`*bailed` set for
 * a bailout). */
zend_object *swr_zend_object_deny(zend_class_entry *ce, bool *bailed);
#include "zend_smart_str.h"
/* zend_is_callable_at_frame(value, $this of frame or NULL, frame, 0, cache, NULL)
 * inside zend_try: 1 if it bailed out, else *result says whether it is callable. */
int swr_zend_try_facade_callable_frame(zval *value, zend_execute_data *frame, zend_fcall_info_cache *cache,
                                       bool *result);
int swr_zend_try_facade_register_functions(const zend_function_entry *, HashTable *, int, int *);
HashTable *swr_zend_facade_compiler_functions(void);
#endif
