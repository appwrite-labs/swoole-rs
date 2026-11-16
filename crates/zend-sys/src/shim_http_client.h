/* Shims the coroutine HTTP clients (crates/phpclients) need: Swoole's property
 * and array helpers from php_swoole_private.h, rebuilt over Zend's own macros.
 */
#ifndef SWOOLE_RS_ZEND_SHIM_HTTP_CLIENT_H
#define SWOOLE_RS_ZEND_SHIM_HTTP_CLIENT_H
#include "shim.h"
#include <sys/uio.h>

/* Swoole's zend::array_add_or_merge: add `element` (taking it over) under `key`,
 * or turn an existing value into a list and append to it. */
void swr_lent_array_add_or_merge(zval *array, struct iovec key, zval *element);

/* zend_is_true, whose fast path is inline. */
bool swr_zval_is_true(const zval *value);

/* php_swoole_http_build_query: http_build_query($data) as RFC 1738, or NULL. */
zend_string *swr_zend_http_build_query(zval *data);

/* instanceof_function(ce, parent). */
bool swr_zend_instanceof(const zend_class_entry *ce, const zend_class_entry *parent);
/* Whether `ce`'s properties_info names `member`. */
bool swr_zend_class_declares(const zend_class_entry *ce, zend_string *member);

/* SEPARATE_ARRAY of a zval holding an array; anything else is left alone. */
void swr_zend_separate_array(zval *array);
#endif
