/* Shims crates/phpserver uses (see shim.h). */
#ifndef SWOOLE_RS_ZEND_SHIM_SERVER_H
#define SWOOLE_RS_ZEND_SHIM_SERVER_H
#include "shim.h"

/* php_var_serialize(value) inside zend_try (__serialize and __sleep run PHP):
 * the new string, or NULL when nothing was written; *bailed when it bailed out. */
zend_string *swr_zend_serialize(zval *value, bool *bailed);

/* php_var_unserialize(out, data, length) inside zend_try: whether it succeeded,
 * with *offset where it stopped; *bailed when it bailed out. */
bool swr_zend_unserialize(zval *out, const char *data, size_t length, size_t *offset, bool *bailed);

/* SEPARATE_ARRAY(dest); php_array_merge(Z_ARRVAL_P(dest), Z_ARRVAL_P(src)) inside
 * zend_try (a replaced value's destructor runs PHP): 1 if it bailed out. */
int swr_zend_try_array_merge(zval *dest, zval *src);
#endif
