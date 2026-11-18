#include "config.h"
#include "php.h"

#ifdef SW_USE_PGSQL
#include "pgsql_statement.h"
/* PDO stores zend_string pointers under the original parameter names. */
const zend_string *pdo_pgsql_rewritten_name(pdo_stmt_t *stmt, zend_string *name) {
    return stmt->bound_param_map ? zend_hash_find_ptr(stmt->bound_param_map, name) : NULL;
}

/* Zend exposes PostgreSQL's large-object handle through a stream's private data. */
bool pdo_pgsql_bind_stream(pdo_dbh_t *dbh, pdo_stmt_t *stmt, void *params, size_t index, zval *value) {
    php_stream *stream;
    php_stream_from_zval_no_verify(stream, value);
    if (!stream) {
        pdo_pgsql_raise(dbh, stmt, PGRES_FATAL_ERROR, "HY105", NULL);
        return false;
    }
    if (php_stream_is(stream, &pdo_pgsql_lob_stream_ops)) {
        struct pdo_pgsql_lob_self *lob = stream->abstract;
        return pdo_pgsql_bind_oid(params, index, lob->oid);
    }
    zend_string *bytes = php_stream_copy_to_mem(stream, PHP_STREAM_COPY_ALL, 0);
    if (bytes) {
        ZVAL_STR(value, bytes);
    } else {
        ZVAL_EMPTY_STRING(value);
    }
    return pdo_pgsql_bind_value(params, index, PDO_PARAM_LOB, value);
}

#if PHP_VERSION_ID >= 80400
#include <sys/uio.h>

extern int pdo_pgsql_scan_token(struct iovec, size_t *);

/* PDO includes the terminal NUL in end. Rust receives only the query bytes. */
int pdo_pgsql_scanner(pdo_scanner_t *s) {
    size_t consumed;
    s->tok = s->cur;
    int token = pdo_pgsql_scan_token((struct iovec){(void *)s->cur, s->end - s->cur - 1}, &consumed);
    s->cur += consumed;
    return token;
}
#endif
#endif
