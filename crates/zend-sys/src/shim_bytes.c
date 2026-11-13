/* See shim_bytes.h. */
#include "shim_bytes.h"

zend_ulong swr_bytes_hash(struct iovec key) {
    return zend_hash_func(key.iov_base, key.iov_len);
}

uint8_t swr_bytes_numeric(struct iovec text, zend_long *lval, double *dval) {
    /* The parse reads its first byte before the length and scans to a NUL, so it
     * gets a terminated copy. */
    char *copy = estrndup(text.iov_base, text.iov_len);
    uint8_t kind = is_numeric_string(copy, text.iov_len, lval, dval, false);
    efree(copy);
    return kind;
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
