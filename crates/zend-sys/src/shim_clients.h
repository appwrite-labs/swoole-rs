/* Shims the client classes (crates/phpclients) need. */
#ifndef SWOOLE_RS_ZEND_SHIM_CLIENTS_H
#define SWOOLE_RS_ZEND_SHIM_CLIENTS_H

#include "shim.h"

/* zend_string_alloc, zend_string_realloc and zend_string_free, non-persistent. */
zend_string *swr_zend_string_alloc(size_t length);
zend_string *swr_zend_string_realloc(zend_string *string, size_t length);
void swr_zend_string_free(zend_string *string);
#endif
