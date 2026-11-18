/* Stays C: allocator. These conversions use Zend-owned, NUL-terminated storage. */
#include "shim_bytes.h"
#include "ext/json/php_json.h"

uint8_t swr_bytes_numeric(struct iovec text, zend_long *lval, double *dval) {
    /* allocator: the parse reads its first byte before the length and scans to a NUL, so it
     * gets a terminated copy. */
    char *copy = estrndup(text.iov_base, text.iov_len);
    uint8_t kind = is_numeric_string(copy, text.iov_len, lval, dval, false);
    efree(copy);
    return kind;
}

/* allocator: decode in the buffer allocated by zend_string_init. */
zend_string *swr_bytes_url_decode(struct iovec bytes) {
    zend_string *string = zend_string_init(bytes.iov_base, bytes.iov_len, 0);
    ZSTR_LEN(string) = php_url_decode(ZSTR_VAL(string), ZSTR_LEN(string));
    return string;
}

/* allocator: estrndup supplies Zend debug allocation metadata. */
char *swr_bytes_estrndup(struct iovec bytes) {
    return estrndup(bytes.iov_base, bytes.iov_len);
}

/* allocator: the scanner reads one byte past the length and takes only a NUL there as the
 * end, as upstream's std::string::c_str() guarantees; a reply buffer has none. */
void swr_bytes_json_decode(struct iovec text, zval *out) {
    zend_string *copy = zend_string_init(text.iov_base, text.iov_len, 0);
    if (php_json_decode(out, ZSTR_VAL(copy), text.iov_len, 1, 0) == FAILURE) {
        ZVAL_FALSE(out);
    }
    zend_string_release(copy);
}
