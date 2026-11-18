/*
  +----------------------------------------------------------------------+
  | Copyright (c) The PHP Group                                          |
  +----------------------------------------------------------------------+
  | This source file is subject to version 3.01 of the PHP license,      |
  | that is bundled with this package in the file LICENSE, and is        |
  | available through the world-wide-web at the following url:           |
  | https://www.php.net/license/3_01.txt                                 |
  | If you did not receive a copy of the PHP license and are unable to   |
  | obtain it through the world-wide-web, please send a note to          |
  | license@php.net so we can mail you a copy immediately.               |
  +----------------------------------------------------------------------+
  | Authors: Edin Kadribasic <edink@emini.dk>                            |
  |          Ilia Alshanestsky <ilia@prohost.org>                        |
  |          Wez Furlong <wez@php.net>                                   |
  +----------------------------------------------------------------------+
*/

#include "config.h"
#include "php.h"

#ifdef SW_USE_PGSQL
#include "php_pdo_pgsql_int.h"
#include <libpq/libpq-fs.h>
#include "main/php_memory_streams.h"
static bool pdo_pgsql_dispatch(pdo_stmt_t *stmt, pdo_pgsql_stmt *S, bool unbuffered) {
    bool named = stmt->supports_placeholders == PDO_PLACEHOLDER_NAMED;
    zend_string *query = named || S->stmt_name ? S->query : stmt->active_query_string;
    unsigned int count = stmt->bound_params ? zend_hash_num_elements(stmt->bound_params) : 0;
    return pdo_pgsql_send_statement(stmt->dbh, stmt, &S->result, S->stmt_name, ZSTR_VAL(query),
                                    S->params, count, &S->is_prepared, named, unbuffered);
}

static int pdo_pgsql_stmt_fetch(pdo_stmt_t *stmt, enum pdo_fetch_orientation ori, zend_long offset) {
    pdo_pgsql_stmt *S = stmt->driver_data;
#if PHP_VERSION_ID >= 80500
    bool *running = &S->is_running_unbuffered;
#else
    bool *running = NULL;
#endif
    return pdo_pgsql_fetch_row(stmt->dbh, stmt, &S->result, S->cursor_name, ori, offset, &S->current_row, running);
}

/* libpq owns the decoded bytes; Zend's memory stream retains the copied string. */
bool pdo_pgsql_bytea_stream(const char *text, zval *output) {
    size_t len;
    unsigned char *decoded = PQunescapeBytea((const unsigned char *)text, &len);
    if (!decoded) {
        return false;
    }
    zend_string *bytes = zend_string_init((char *)decoded, len, 0);
    PQfreemem(decoded);
    php_stream *stream = php_stream_memory_open(TEMP_STREAM_READONLY, bytes);
    zend_string_release(bytes);
    if (!stream) {
        return false;
    }
    php_stream_to_zval(stream, output);
    return true;
}

static int pdo_pgsql_stmt_get_col(pdo_stmt_t *stmt, int colno, zval *output, enum pdo_param_type *type) {
    pdo_pgsql_stmt *S = stmt->driver_data;
    return pdo_pgsql_read_column(stmt, S->H, S->result, S->current_row - 1, colno, type, output);
}

struct iovec pdo_pgsql_column_bytes(PGresult *result, int row, int column) {
    return (struct iovec){PQgetvalue(result, row, column), PQgetlength(result, row, column)};
}

static int pdo_pgsql_stmt_describe(pdo_stmt_t *stmt, int colno) {
    pdo_pgsql_stmt *S = stmt->driver_data;
    return S->result && pdo_pgsql_describe_column(S->result, colno, &stmt->columns[colno]);
}

static int pdo_pgsql_stmt_get_column_meta(pdo_stmt_t *stmt, zend_long colno, zval *output) {
    pdo_pgsql_stmt *S = stmt->driver_data;
    if (!S->result || colno < 0 || colno >= stmt->column_count) {
        return FAILURE;
    }
    return pdo_pgsql_column_meta(S->H, S->result, (int)colno, output);
}

static int pdo_pgsql_stmt_cursor_closer(pdo_stmt_t *stmt) {
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

/* Zend owns the bound record. The normalized value may be a PHP reference. */
static int pdo_pgsql_stmt_param_hook(pdo_stmt_t *stmt, struct pdo_bound_param_data *param, enum pdo_param_event event) {
    if (!param->is_param) {
        return 1;
    }
    if (event == PDO_PARAM_EVT_NORMALIZE) {
        return stmt->supports_placeholders == PDO_PLACEHOLDER_NAMED
            ? pdo_pgsql_normalize_parameter(stmt->dbh, stmt, param->name, &param->paramno)
            : pdo_pgsql_emulated_parameter(&param->param_type, &param->parameter);
    }
    if (stmt->supports_placeholders != PDO_PLACEHOLDER_NAMED) {
        return 1;
    }
    if (event == PDO_PARAM_EVT_ALLOC) {
        return pdo_pgsql_check_parameter(stmt->dbh, stmt, stmt->bound_param_map, param->paramno);
    }
    if (event != PDO_PARAM_EVT_EXEC_PRE) {
        return 1;
    }
    zval *value = &param->parameter;
    ZVAL_DEREF(value);
    return pdo_pgsql_bind_parameter(stmt->dbh, stmt, stmt->bound_param_map,
                                    param->paramno, PDO_PARAM_TYPE(param->param_type), value);
}

void *pdo_pgsql_stmt_parameters(pdo_stmt_t *stmt, size_t count) {
    pdo_pgsql_stmt *state = stmt->driver_data;
    if (!state->params) {
        state->params = pdo_pgsql_allocate_parameters(count);
    }
    return state->params;
}

static void finish_statement(pdo_pgsql_stmt *S, bool close, bool abort_query) {
    PGconn *active = NULL;
#if PHP_VERSION_ID >= 80500
    if (S->is_running_unbuffered) {
        active = pdo_pgsql_server(S->H);
        S->is_running_unbuffered = false;
    }
#endif
    pdo_pgsql_finish_result(active, &S->result, abort_query);
    if (close && S->stmt_name && S->is_prepared) {
        pdo_pgsql_close_statement(S->H, S->stmt_name);
        S->is_prepared = false;
    }
#if PHP_VERSION_ID >= 80500
    if (close && *pdo_pgsql_running_stmt(S->H) == S) {
        *pdo_pgsql_running_stmt(S->H) = NULL;
    }
#endif
}

static int pgsql_stmt_dtor(pdo_stmt_t *stmt) {
    pdo_pgsql_stmt *S = stmt->driver_data;
#if PHP_VERSION_ID >= 80500
    bool live = php_pdo_stmt_valid_db_obj_handle(stmt);
#else
    bool live = !Z_ISUNDEF(stmt->database_object_handle)
        && IS_OBJ_VALID(EG(objects_store).object_buckets[Z_OBJ_HANDLE(stmt->database_object_handle)])
        && !(OBJ_FLAGS(Z_OBJ(stmt->database_object_handle)) & IS_OBJ_FREE_CALLED);
#endif
    finish_statement(S, live, live);
    if (S->stmt_name) {
        efree(S->stmt_name);
    }
    pdo_pgsql_free_parameters(S->params);
    if (S->query) {
        zend_string_release(S->query);
    }
    if (S->cursor_name) {
        if (live) {
            pdo_pgsql_close_cursor(S->H, S->cursor_name);
        }
        efree(S->cursor_name);
    }
    efree(S);
    stmt->driver_data = NULL;
    return 1;
}

static int pgsql_stmt_execute(pdo_stmt_t *stmt) {
    pdo_pgsql_stmt *S = stmt->driver_data;
    bool in_transaction = stmt->dbh->methods->in_transaction(stmt->dbh);
    bool unbuffered = false;
#if PHP_VERSION_ID >= 80500
    unbuffered = S->is_unbuffered && !S->cursor_name;
    pdo_pgsql_stmt **previous = pdo_pgsql_running_stmt(S->H);
    if (*previous && (unbuffered || (*previous)->is_running_unbuffered)) {
        finish_statement(*previous, true, false);
    }
#endif
    finish_statement(S, false, false);
    S->current_row = 0;
    if (S->cursor_name) {
        if (!pdo_pgsql_open_cursor(stmt->dbh, stmt, &S->result, S->cursor_name, &S->is_prepared,
                                   ZSTR_VAL(stmt->active_query_string))) {
            return 0;
        }
    } else if (!pdo_pgsql_dispatch(stmt, S, unbuffered)) {
        return 0;
    }
#if PHP_VERSION_ID >= 80500
    if (unbuffered) {
        *previous = S;
        S->is_running_unbuffered = true;
        PGconn *conn = pdo_pgsql_server(S->H);
        PQsetSingleRowMode(conn);
        S->result = PQgetResult(conn);
    }
#endif
    if (!pdo_pgsql_execution_result(stmt->dbh, stmt, S->result)) {
        return 0;
    }
    if (in_transaction && !stmt->dbh->methods->in_transaction(stmt->dbh)) {
        pdo_pgsql_close_lob_streams(stmt->dbh);
    }
    return 1;
}

#if PHP_VERSION_ID >= 80400
static int pgsql_stmt_get_attr(pdo_stmt_t *stmt, zend_long attr, zval *value) {
#ifdef HAVE_PG_RESULT_MEMORY_SIZE
    if (attr == PDO_PGSQL_ATTR_RESULT_MEMORY_SIZE) {
        pdo_pgsql_stmt *S = stmt->driver_data;
        if (stmt->executed) {
            ZVAL_LONG(value, PQresultMemorySize(S->result));
        } else {
            char *message;
            spprintf(&message, 0, "statement '%s' has not been executed yet", S->stmt_name);
            pdo_pgsql_error_stmt_msg(stmt, 0, "HY000", message);
            efree(message);
            ZVAL_NULL(value);
        }
        return 1;
    }
#endif
    return 0;
}
#endif

const struct pdo_stmt_methods swoole_pgsql_stmt_methods = {
    .dtor = pgsql_stmt_dtor,
    .executer = pgsql_stmt_execute,
    .fetcher = pdo_pgsql_stmt_fetch,
    .describer = pdo_pgsql_stmt_describe,
    .get_col = pdo_pgsql_stmt_get_col,
    .param_hook = pdo_pgsql_stmt_param_hook,
#if PHP_VERSION_ID >= 80400
    .get_attribute = pgsql_stmt_get_attr,
#endif
    .get_column_meta = pdo_pgsql_stmt_get_column_meta,
    .cursor_closer = pdo_pgsql_stmt_cursor_closer,
};

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
