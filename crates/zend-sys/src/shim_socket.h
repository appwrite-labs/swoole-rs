/* Coroutine\Socket's options and import (crates/phpclients/src/socket_options.rs). */
#ifndef SWOOLE_RS_ZEND_SHIM_SOCKET_H
#define SWOOLE_RS_ZEND_SHIM_SOCKET_H
#include "php.h"
#include <netinet/in.h>

/* php_stream_from_zval (a TypeError for any other resource), then
 * php_stream_cast(stream, PHP_STREAM_AS_SOCKETD, fd, REPORT_ERRORS), inside
 * zend_try: 0 the descriptor is in *fd, 1 failed, 2 bailed out. */
int swr_zend_stream_socketd(zval *zstream, php_stream **stream, int *fd);
/* php_stream_from_zval_no_verify, then php_stream_set_option(stream,
 * PHP_STREAM_OPTION_READ_BUFFER, PHP_STREAM_BUFFER_NONE, NULL); nothing for a
 * value that holds no stream. */
void swr_zend_stream_unbuffer(zval *zstream);
/* php_network_gethostbyname(host): 0 with the first address of an AF_INET entry
 * copied to *address, 1 for an entry of another family, -1 for none. */
int swr_zend_gethostbyname4(const char *host, struct in_addr *address);
/* Z_OBJPROP of `object` inside zend_try (nonzero for a bailout: a lazy object's
 * initializer runs PHP code): `out` then holds the table, if any, with a reference
 * of its own (none for an immutable one), in place of what it held, which is not
 * released. */
int swr_zend_try_object_properties(zend_object *object, zval *out);
/* convert_to_array(value) inside zend_try: nonzero for a bailout. */
int swr_zend_try_convert_to_array(zval *value);
#endif
