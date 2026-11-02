/* Shims the thread-shared containers (crates/phpthread) need: inline functions
 * and macros of zend_hash.h, zend_operators.h and zend_execute.h. */
#ifndef SWOOLE_RS_ZEND_SHIM_THREAD_H
#define SWOOLE_RS_ZEND_SHIM_THREAD_H

#include "shim.h"

/* zend_array_is_list. */
bool swr_zend_array_is_list(const zend_array *array);

/* ZEND_HASH_FOREACH_KEY_VAL with ZVAL_DEREF: `key` is null for an int key. */
void swr_zend_hash_foreach_kv(const HashTable *table,
                              void (*each)(zend_string *key, zend_ulong index, zval *value, void *context),
                              void *context);

/* _zend_handle_numeric_str: whether a string key is a canonical integer. */
bool swr_zend_handle_numeric_str(const char *key, size_t length, zend_ulong *index);

/* zend_dval_to_lval_safe. */
zend_long swr_zend_dval_to_lval_safe(double value);

/* Z_RES_HANDLE_P. */
zend_long swr_zend_resource_handle(const zval *value);

/* is_numeric_string(text, length, &lval, &dval, false): IS_LONG, IS_DOUBLE or 0. */
uint8_t swr_zend_is_numeric_string(const char *text, size_t length, zend_long *lval, double *dval);
/* zendi_smart_strcmp. */
int swr_zend_smart_strcmp(zend_string *a, zend_string *b);
#endif
