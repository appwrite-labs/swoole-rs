/* See shim_bytes.h. */
#include "shim_bytes.h"

zend_ulong swr_bytes_hash(struct iovec key) {
    return zend_hash_func(key.iov_base, key.iov_len);
}

uint8_t swr_bytes_numeric(struct iovec text, zend_long *lval, double *dval) {
    return is_numeric_string(text.iov_base, text.iov_len, lval, dval, false);
}

zend_string *swr_bytes_url_encode(struct iovec bytes) {
    return php_url_encode(bytes.iov_base, bytes.iov_len);
}

zend_string *swr_bytes_persistent(struct iovec bytes) {
    return zend_string_init(bytes.iov_base, bytes.iov_len, 1);
}

char *swr_bytes_estrndup(struct iovec bytes) {
    return estrndup(bytes.iov_base, bytes.iov_len);
}
