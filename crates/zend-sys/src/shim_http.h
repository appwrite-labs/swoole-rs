/* Shims crates/phphttp and the Server family use (see shim.h). */
#ifndef SWOOLE_RS_ZEND_SHIM_HTTP_H
#define SWOOLE_RS_ZEND_SHIM_HTTP_H
#include "php.h"
#include "shim_values.h"

/* ZVAL_STRINGL: a new string holding a copy of the bytes. */
void swr_zval_stringl(zval *target, const char *bytes, size_t length);

/* ZVAL_COPY: `target` takes a new reference to `source`'s value. */
void swr_zval_copy(zval *target, const zval *source);

#endif
