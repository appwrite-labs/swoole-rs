/* Functions of a byte range that Rust lends as a `struct iovec` (std's `IoSlice`,
 * which it makes ABI-compatible with one), so Rust can declare them over the
 * borrow, safely. Each reads the range's `iov_len` bytes, touches no PHP state but
 * Zend's allocator, and runs no PHP code. The `swr_bytes_` prefix keeps them out of
 * bindgen's allowlist: crates/zend, crates/phpstdext and crates/phphttp declare them,
 * typed. */
#ifndef SWOOLE_RS_ZEND_SHIM_BYTES_H
#define SWOOLE_RS_ZEND_SHIM_BYTES_H

/* shim.h (php.h) first: a system header before it fixes glibc's feature macros
 * without _GNU_SOURCE, and zend_operators.h's memrchr goes undeclared. */
#include "shim.h"
#include <sys/uio.h>

/* is_numeric_string(text, len, lval, dval, false) over a NUL-terminated copy of the
 * bytes: IS_LONG, IS_DOUBLE or 0, with `*lval` or `*dval` filled; errors off, so
 * nothing is raised. */
uint8_t swr_bytes_numeric(struct iovec text, zend_long *lval, double *dval);

/* php_url_decode of a new string's copy of the bytes: them as urldecode() decodes
 * them. */
zend_string *swr_bytes_url_decode(struct iovec bytes);

/* estrndup: an emalloc'd NUL-terminated copy of the bytes. */
char *swr_bytes_estrndup(struct iovec bytes);

/* json_decode(text, true) into `out`, in place of what it held, which is not
 * released (depth 0, as Server::command passes), or false. Objects decode as
 * arrays, so no PHP code runs. */
void swr_bytes_json_decode(struct iovec text, zval *out);

/* php_json_decode_ex(out, text, len, options, depth) inside zend_try (nonzero for a
 * bailout), over the bytes but their last, which the decoder may read: Swoole's
 * zend::json_decode of a window of a string, with the byte after it lent too. `out`
 * then holds the result, in place of what it held, which is not released. */
int swr_bytes_json_decode_ex(zval *out, struct iovec text, zend_long options, zend_long depth);

#endif
