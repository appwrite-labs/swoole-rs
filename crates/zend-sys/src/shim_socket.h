/* Coroutine\Socket's options and import (crates/phpclients/src/socket_options.rs). */
#ifndef SWOOLE_RS_ZEND_SHIM_SOCKET_H
#define SWOOLE_RS_ZEND_SHIM_SOCKET_H
#include "php.h"

/* php_stream_from_zval (a TypeError for any other resource), then
 * php_stream_cast(stream, PHP_STREAM_AS_SOCKETD, fd, REPORT_ERRORS), inside
 * zend_try: 0 the descriptor is in *fd, 1 failed, 2 bailed out. */
int swr_zend_stream_socketd(zval *zstream, php_stream **stream, int *fd);
/* php_stream_set_option(stream, PHP_STREAM_OPTION_READ_BUFFER, PHP_STREAM_BUFFER_NONE, NULL). */
void swr_zend_stream_unbuffer(php_stream *stream);
/* Z_OBJPROP_P(object). */
HashTable *swr_zend_object_properties(zval *object);
/* convert_to_array(value), then Z_ARRVAL_P(value). */
HashTable *swr_zend_convert_to_array(zval *value);
/* zend_zval_value_name (PHP 8.3+), else zend_zval_type_name. */
const char *swr_zend_zval_value_name(const zval *value);
#endif
