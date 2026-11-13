/* Shims the coroutine HTTP clients (crates/phpclients) need: Swoole's property
 * and array helpers from php_swoole_private.h, rebuilt over Zend's own macros.
 */
#ifndef SWOOLE_RS_ZEND_SHIM_HTTP_CLIENT_H
#define SWOOLE_RS_ZEND_SHIM_HTTP_CLIENT_H
#include "shim.h"

/* sw_zend_read_property_ex: the declared property's slot, read directly. */
zval *swr_zend_property_slot(zend_object *object, const char *name, size_t length);
/* sw_zend_read_property_not_null_ex: the property, or NULL when it is null. */
zval *swr_zend_property_not_null(zend_object *object, const char *name, size_t length);
/* Swoole's zend::array_add_or_merge: add `element` (taking it over) under `key`,
 * or turn an existing value into a list and append to it. */
void swr_zend_array_add_or_merge(zval *array, const char *key, size_t length, zval *element);

/* zend_is_true, whose fast path is inline. */
bool swr_zval_is_true(const zval *value);

/* php_url_decode in place: the decoded length. */
size_t swr_zend_url_decode(char *data, size_t length);
/* php_swoole_http_build_query: http_build_query($data) as RFC 1738, or NULL. */
zend_string *swr_zend_http_build_query(zval *data);

/* instanceof_function(ce, parent). */
bool swr_zend_instanceof(const zend_class_entry *ce, const zend_class_entry *parent);

/* SEPARATE_ARRAY. */
void swr_zend_separate_array(zval *array);
#endif
