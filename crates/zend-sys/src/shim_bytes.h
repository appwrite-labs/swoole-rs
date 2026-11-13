/* Functions of a byte range that Rust lends as a `struct iovec` (std's `IoSlice`,
 * which it makes ABI-compatible with one), so Rust can declare them over the
 * borrow, safely. Each reads the range's `iov_len` bytes, touches no PHP state but
 * Zend's allocator, and runs no PHP code. The `swr_bytes_` prefix keeps them out of
 * bindgen's allowlist: crates/zend and crates/phpstdext declare them, typed. */
#ifndef SWOOLE_RS_ZEND_SHIM_BYTES_H
#define SWOOLE_RS_ZEND_SHIM_BYTES_H

#include <sys/uio.h>
#include "shim.h"

/* zend_hash_func: the hash a zend_string of these bytes caches. */
zend_ulong swr_bytes_hash(struct iovec key);

/* is_numeric_string(text, len, lval, dval, false): IS_LONG, IS_DOUBLE or 0, with
 * `*lval` or `*dval` filled; errors off, so nothing is raised. */
uint8_t swr_bytes_numeric(struct iovec text, zend_long *lval, double *dval);

/* php_url_encode: a new string, as urlencode() encodes the bytes. */
zend_string *swr_bytes_url_encode(struct iovec bytes);

/* zend_string_init(bytes, len, 1): a new persistent string of the bytes; out of
 * memory, Zend prints and exits (zend_out_of_memory). */
zend_string *swr_bytes_persistent(struct iovec bytes);

/* estrndup: an emalloc'd NUL-terminated copy of the bytes. */
char *swr_bytes_estrndup(struct iovec bytes);

#endif
