/* Shims the HTTP request parser (crates/phphttp/src/parse.rs) needs. */
#ifndef SWOOLE_RS_ZEND_SHIM_HTTP_CTX_H
#define SWOOLE_RS_ZEND_SHIM_HTTP_CTX_H

#include "shim.h"

/* The slot offset of a declared property of `ce` (0 if it has none). */
uint32_t swr_zend_http_property_offset(zend_class_entry *ce, const char *name, size_t length);
/* array_init_size on the property slot at `offset` of `object`. */
zval *swr_zend_http_property_array(zend_object *object, uint32_t offset, uint32_t size);
/* estrndup. */
char *swr_zend_http_estrndup(const char *text, size_t length);
/* zend_string_init_interned, persistent: for keys interned at MINIT. */
zend_string *swr_zend_http_intern(const char *text, size_t length);
/* PG(max_input_vars). */
zend_long swr_zend_http_max_input_vars(void);
/* Append `file` to the request's `tmpfiles` and register it in
 * SG(rfc1867_uploaded_files) for is_uploaded_file()/move_uploaded_file(). */
void swr_zend_http_upload(zval *tmpfiles, const char *file, size_t length);
/* Swoole's copy of php_default_treat_data for PARSE_STRING and PARSE_COOKIE:
 * the variables in `str` (an emalloc'd string it frees) into `array`. False
 * when it stopped at PG(max_input_vars), for the caller's warning. */
bool swr_zend_http_treat_data(int arg, char *str, zval *array);
/* Unlink the uploaded files the request's `tmpfiles` property (at `offset`)
 * holds now, and forget them in SG(rfc1867_uploaded_files). User code may have
 * replaced the array, so the property is read here, not a cached copy. */
void swr_zend_http_uploads_release(zend_object *object, uint32_t offset);
/* Swoole\Http\Server's RINIT and RSHUTDOWN of SG(rfc1867_uploaded_files),
 * the uploads is_uploaded_file() and move_uploaded_file() know. */
void swr_zend_http_uploads_rinit(void);
void swr_zend_http_uploads_rshutdown(void);
/* A context's references to its request and response objects, released in
 * that order as a zend::bailout::trap body (a property's __destruct may run):
 * `arg` is a swr_zend_http_release, whose `begun` counts the releases begun,
 * so after a bailout in the request's a second call releases the response. */
typedef struct {
    zval *request;
    zval *response;
    uint8_t begun;
} swr_zend_http_release;
void swr_zend_http_release_objects(void *arg);
#endif
