#include "config.h"
#include "php.h"

#ifdef SW_USE_PGSQL
#include "pgsql_statement.h"
int pdo_pgsql_stmt_fetch(pdo_stmt_t *stmt, enum pdo_fetch_orientation ori, zend_long offset) {
    pdo_pgsql_stmt *S = stmt->driver_data;
#if PHP_VERSION_ID >= 80500
    bool *running = &S->is_running_unbuffered;
#else
    bool *running = NULL;
#endif
    return pdo_pgsql_fetch_row(stmt->dbh, stmt, &S->result, S->cursor_name, ori, offset, &S->current_row, running);
}

int pdo_pgsql_stmt_describe(pdo_stmt_t *stmt, int colno) {
    pdo_pgsql_stmt *S = stmt->driver_data;
    return S->result && pdo_pgsql_describe_column(S->result, colno, &stmt->columns[colno]);
}

int pdo_pgsql_stmt_get_column_meta(pdo_stmt_t *stmt, zend_long colno, zval *output) {
    pdo_pgsql_stmt *S = stmt->driver_data;
    if (!S->result || colno < 0 || colno >= stmt->column_count) {
        return FAILURE;
    }
    return pdo_pgsql_column_meta(S->H, S->result, (int)colno, output);
}

int pdo_pgsql_stmt_cursor_closer(pdo_stmt_t *stmt) {
    return 1;
}

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
