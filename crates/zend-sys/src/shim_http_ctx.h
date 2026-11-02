/* Shims the HTTP request parser (crates/phphttp/src/parse.rs) needs. */
#ifndef SWOOLE_RS_ZEND_SHIM_HTTP_CTX_H
#define SWOOLE_RS_ZEND_SHIM_HTTP_CTX_H

#include "shim.h"

/* The slot offset of a declared property of `ce` (0 if it has none). */
uint32_t swr_zend_http_property_offset(zend_class_entry *ce, const char *name, size_t length);
/* array_init_size on the property slot at `offset` of `object`. */
zval *swr_zend_http_property_array(zend_object *object, uint32_t offset, uint32_t size);
/* Swoole's zend::array_add_or_merge: add `element` under `key` (a numeric
 * string is an integer key), or append it to a list of the values the key
 * already holds. */
void swr_zend_http_add_or_merge(zval *array, const char *key, size_t length, zval *element);
/* estrndup. */
char *swr_zend_http_estrndup(const char *text, size_t length);
/* zend_string_init_interned, persistent: for keys interned at MINIT. */
zend_string *swr_zend_http_intern(const char *text, size_t length);
/* PG(max_input_vars). */
zend_long swr_zend_http_max_input_vars(void);
/* Append `file` to the request's `tmpfiles` and register it in
 * SG(rfc1867_uploaded_files) for is_uploaded_file()/move_uploaded_file(). */
void swr_zend_http_upload(zval *tmpfiles, const char *file, size_t length);
/* zval_get_string, as a zend::bailout::trap body (it may run __toString):
 * `arg` is a swr_zend_http_string. */
typedef struct {
    zval *value;
    zend_string *result;
} swr_zend_http_string;
void swr_zend_http_get_string(void *arg);
#endif
