#ifndef SWOOLE_RS_ZEND_SHIM_FACADE_H
#define SWOOLE_RS_ZEND_SHIM_FACADE_H
#include "php.h"
#include "zend_exceptions.h"
/* User code invoked by object bool casts and callable resolution stays in C. */
int swr_zend_try_facade_truth(zval *, bool *);
int swr_zend_try_facade_callable(zval *, zend_fcall_info_cache *, bool *);
/* zend_get_callable_name_ex(value) into `name` (not released first), inside zend_try. */
int swr_zend_try_facade_name(zval *, zval *name);
/* zval_refcount_p of a zval that holds a counted value (a string, array, object,
 * resource or reference); 0 for any other. */
uint32_t swr_zval_facade_refcount(const zval *);

int swr_zend_try_facade_throw(zend_class_entry *, const char *, zend_long);
/* zend_throw_error(NULL, "%s", message) and object_init_ex(target, ce) inside
 * zend_try: with no frame executing, the Error either throws is a fatal error. */
int swr_zend_try_facade_throw_error(const char *message);
int swr_zend_try_facade_object_init(zval *target, zend_class_entry *ce);

int swr_zend_try_facade_clone(const zval *source, zval *result);

#endif
