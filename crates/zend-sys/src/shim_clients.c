/* Stays C (docs/zend.md, What stays C): allocator (zend_string_alloc, _realloc and
 * _free). */
#include "shim_clients.h"

/* memory_limit must not bail out of an allocation Rust makes (docs/zend.md, memory_limit). */
#ifndef ZEND_HAS_OOM
#error "this PHP bails out of the allocator at memory_limit, over Rust frames: build against 8.2.35, 8.3.36, 8.4.27, 8.5.12 or later"
#endif

zend_string *swr_zend_string_alloc(size_t length) {
    return zend_string_alloc(length, 0);
}

zend_string *swr_zend_string_realloc(zend_string *string, size_t length) {
    return zend_string_realloc(string, length, 0);
}

void swr_zend_string_free(zend_string *string) {
    zend_string_free(string);
}

