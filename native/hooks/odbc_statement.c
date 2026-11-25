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
  | Author: Wez Furlong <wez@php.net>                                    |
  +----------------------------------------------------------------------+
*/

/* pdo_odbc's statement side. crates/odbc/src/statement.rs has the operations. One copy
 * serves every supported PHP version; each supplies the diagnostic locations of its own
 * ext/pdo_odbc/odbc_stmt.c.
 *
 * Stays C (docs/zend.md, What stays C): allocator (column and parameter buffers, column
 * names and result strings are Zend's to allocate and free), bailout (converting a bound
 * value to a string, releasing the value an output parameter replaces and a stream's stat
 * can run PHP code) and budget (pdo_stmt_t's columns and bound parameters, read as PDO's
 * own drivers read them). The method table is C because PDO declares each entry over
 * pdo_stmt_t pointers, which the C compiler checks against the functions here. */
#include "php_swoole_odbc.h"

#ifdef SW_USE_ODBC
#include "php.h"
#include "main/php_memory_streams.h"
#include "ext/pdo/php_pdo.h"
#include "ext/pdo/php_pdo_driver.h"

#if PHP_VERSION_ID >= 80500
static const char stmt_file[] = "thirdparty/php85/pdo_odbc/odbc_stmt.c";
static const int bind_lines[] = {388, 404};
static const int execute_lines[] = {261, 265, 227};
static const int fetch_lines[] = {549, 558};
static const int describe_lines[] = {595, 606, 630};
static const int attr_lines[] = {801, 831};
/* Buffer size; bigger columns than this become a "long column" */
#define LONG_COLUMN_BUFFER_SIZE (ZEND_MM_PAGE_SIZE - ZSTR_MAX_OVERHEAD)
#else
#if PHP_VERSION_ID >= 80400
static const char stmt_file[] = "thirdparty/php84/pdo_odbc/odbc_stmt.c";
static const int attr_lines[] = {774, 802};
#elif PHP_VERSION_ID >= 80300
static const char stmt_file[] = "thirdparty/php83/pdo_odbc/odbc_stmt.c";
static const int attr_lines[] = {774, 802};
#else
static const char stmt_file[] = "thirdparty/php82/pdo_odbc/odbc_stmt.c";
static const int attr_lines[] = {775, 803};
#endif
static const int bind_lines[] = {373, 391};
static const int execute_lines[] = {250, 254, 216};
static const int fetch_lines[] = {543, 552};
static const int describe_lines[] = {591, 600, 626};
#define LONG_COLUMN_BUFFER_SIZE 256
#endif

static int pdo_odbc_sqltype_is_unicode(pdo_odbc_stmt *S, SQLSMALLINT sqltype) {
    if (!S->assume_utf8) return 0;
    switch (sqltype) {
#ifdef SQL_WCHAR
    case SQL_WCHAR:
        return 1;
#endif
#ifdef SQL_WLONGVARCHAR
    case SQL_WLONGVARCHAR:
        return 1;
#endif
#ifdef SQL_WVARCHAR
    case SQL_WVARCHAR:
        return 1;
#endif
    default:
        return 0;
    }
}

static void free_cols(pdo_stmt_t *stmt, pdo_odbc_stmt *S) {
    if (S->cols) {
        int i;

        for (i = 0; i < S->col_count; i++) {
            if (S->cols[i].data) {
                efree(S->cols[i].data);
            }
        }
        efree(S->cols);
        S->cols = NULL;
        S->col_count = 0;
    }
}

static int odbc_stmt_dtor(pdo_stmt_t *stmt) {
    pdo_odbc_stmt *S = (pdo_odbc_stmt *) stmt->driver_data;

    if (S->stmt != SQL_NULL_HANDLE) {
        if (stmt->executed) {
            SQLCloseCursor(S->stmt);
        }
        SQLFreeHandle(SQL_HANDLE_STMT, S->stmt);
        S->stmt = SQL_NULL_HANDLE;
    }

    free_cols(stmt, S);
    efree(S);

    return 1;
}

extern bool swr_odbc_allocate_parameter(pdo_dbh_t *, pdo_stmt_t *, struct pdo_bound_param_data *,
    SQLUSMALLINT, enum pdo_param_type, zend_long, bool, bool, const char *, const int (*)[2]);

SQLRETURN swr_odbc_bind_parameter(pdo_stmt_t *stmt, struct pdo_bound_param_data *param, SQLUSMALLINT index,
    SQLSMALLINT direction, SQLSMALLINT ctype, SQLSMALLINT sqltype, SQLULEN precision, SQLSMALLINT scale,
    bool unicode, bool output, size_t units) {
    size_t width = unicode ? 2 : 1;
    pdo_odbc_param *P = safe_emalloc(units, width, sizeof(*P) + width);
    param->driver_data = P;
    P->paramtype = direction;
    P->is_unicode = unicode;
    P->capacity = units * width;
    P->len = P->capacity;
    P->outbuf = output ? (char *) (P + 1) : NULL;
    if (output) {
        memset(P->outbuf, 0, P->capacity + width);
    }
    return SQLBindParameter(((pdo_odbc_stmt *) stmt->driver_data)->stmt, index, direction, ctype, sqltype,
        precision, scale, direction == SQL_PARAM_INPUT ? (SQLPOINTER) param : P->outbuf, P->len, &P->len);
}

static int odbc_stmt_param_hook(pdo_stmt_t *stmt, struct pdo_bound_param_data *param, enum pdo_param_event event_type) {
    pdo_odbc_stmt *S = (pdo_odbc_stmt *) stmt->driver_data;
    pdo_odbc_param *P;
    zval *parameter;

    /* we're only interested in parameters for prepared SQL right now */
    if (param->is_param) {
        switch (event_type) {
        case PDO_PARAM_EVT_FETCH_PRE:
        case PDO_PARAM_EVT_FETCH_POST:
        case PDO_PARAM_EVT_NORMALIZE:
            /* Do nothing */
            break;

        case PDO_PARAM_EVT_FREE:
            P = param->driver_data;
            if (P) {
                efree(P);
            }
            break;

        case PDO_PARAM_EVT_ALLOC:
            if (param->paramno < 0 || param->paramno >= UINT16_MAX) {
                memcpy(stmt->error_code, "HY093", sizeof("HY093"));
                return 0;
            }
            return swr_odbc_allocate_parameter(stmt->dbh, stmt, param, param->paramno + 1,
                PDO_PARAM_TYPE(param->param_type), param->max_value_len,
                (param->param_type & PDO_PARAM_INPUT_OUTPUT) != 0, S->assume_utf8, stmt_file, &bind_lines);

        case PDO_PARAM_EVT_EXEC_PRE:
            P = param->driver_data;
            if (!Z_ISREF(param->parameter)) {
                parameter = &param->parameter;
            } else {
                parameter = Z_REFVAL(param->parameter);
            }

            if (PDO_PARAM_TYPE(param->param_type) == PDO_PARAM_LOB) {
                if (Z_TYPE_P(parameter) == IS_RESOURCE) {
                    php_stream *stm;
                    php_stream_statbuf sb;

                    php_stream_from_zval_no_verify(stm, parameter);

                    if (!stm) {
                        return 0;
                    }

                    P->len = SQL_LEN_DATA_AT_EXEC(php_stream_stat(stm, &sb) == 0 ? sb.sb.st_size : 0);
                } else {
                    convert_to_string(parameter);
                    P->len = SQL_LEN_DATA_AT_EXEC(Z_STRLEN_P(parameter));
                }
            } else if (Z_TYPE_P(parameter) == IS_NULL || PDO_PARAM_TYPE(param->param_type) == PDO_PARAM_NULL) {
                P->len = SQL_NULL_DATA;
            } else {
                convert_to_string(parameter);
                if (P->outbuf) {
                    P->len = MIN(P->capacity, Z_STRLEN_P(parameter));
                    memcpy(P->outbuf, Z_STRVAL_P(parameter), P->len);
                } else {
                    P->len = SQL_LEN_DATA_AT_EXEC(Z_STRLEN_P(parameter));
                }
            }
            return 1;

        case PDO_PARAM_EVT_EXEC_POST:
            P = param->driver_data;

            if (P->outbuf) {
                if (Z_ISREF(param->parameter)) {
                    parameter = Z_REFVAL(param->parameter);
                } else {
                    parameter = &param->parameter;
                }
                zval_ptr_dtor(parameter);

                if (P->len >= 0) {
                    ZVAL_STRINGL(parameter, P->outbuf, MIN(P->capacity, (size_t) P->len));
                } else {
                    ZVAL_NULL(parameter);
                }
            }
            return 1;
        }
    }
    return 1;
}

bool swr_odbc_has_columns(pdo_stmt_t *stmt) {
    return ((pdo_odbc_stmt *) stmt->driver_data)->cols != NULL;
}

/* Hold the input values independently of their PHP references while stream
 * wrappers run. Only SQL_PARAM_INPUT records are passed back by SQLParamData. */
static bool prepare_inputs(pdo_stmt_t *stmt, zend_array *inputs) {
    struct pdo_bound_param_data *param;
    ZEND_HASH_FOREACH_PTR(stmt->bound_params, param) {
        pdo_odbc_param *binding = param->driver_data;
        if (binding && binding->paramtype == SQL_PARAM_INPUT && binding->len != SQL_NULL_DATA) {
            zval snapshot;
            ZVAL_COPY_DEREF(&snapshot, &param->parameter);
            zend_hash_index_update(inputs, (zend_ulong)(uintptr_t)param, &snapshot);
        }
    } ZEND_HASH_FOREACH_END();

    zval *value;
    ZEND_HASH_FOREACH_VAL(inputs, value) {
        if (Z_TYPE_P(value) == IS_STRING) {
            continue;
        }
        php_stream *source = NULL;
        if (Z_TYPE_P(value) == IS_RESOURCE) {
            php_stream_from_zval_no_verify(source, value);
        }
        if (!source) {
            return false;
        }
        php_stream *copy = php_stream_temp_new();
        if (!copy) {
            return false;
        }
        char page[8192];
        ssize_t n;
        while (source && (n = php_stream_read(source, page, sizeof(page))) > 0 && !EG(exception)) {
            if (php_stream_write(copy, page, n) != n) {
                php_stream_close(copy);
                return false;
            }
            php_stream_from_zval_no_verify(source, value);
        }
        if (!source || n < 0 || EG(exception) || php_stream_rewind(copy) != 0) {
            php_stream_close(copy);
            return false;
        }
        zval_ptr_dtor(value);
        php_stream_to_zval(copy, value);
    } ZEND_HASH_FOREACH_END();
    return true;
}

extern bool swr_odbc_execute(pdo_dbh_t *, pdo_stmt_t *, bool, zend_array *, const char *, const int (*)[3]);
static int odbc_stmt_execute(pdo_stmt_t *stmt) {
    zend_array *inputs = stmt->bound_params ? zend_new_array(zend_hash_num_elements(stmt->bound_params)) : NULL;
    if (inputs && !prepare_inputs(stmt, inputs)) {
        zend_array_destroy(inputs);
        if (!EG(exception)) {
            pdo_odbc_stmt *record = stmt->driver_data;
            record->einfo = (pdo_odbc_errinfo){.last_state = "HY000", .what = "could not read input LOB stream",
                                              .file = stmt_file, .line = execute_lines[2]};
            memcpy(stmt->error_code, "HY000", sizeof("HY000"));
        }
        return false;
    }
    bool ok = swr_odbc_execute(stmt->dbh, stmt, stmt->executed, inputs, stmt_file, &execute_lines);
    if (inputs) {
        zend_array_destroy(inputs);
    }
    return ok;
}


extern bool swr_odbc_fetch(pdo_dbh_t *, pdo_stmt_t *, enum pdo_fetch_orientation, zend_long, const char *, int, int);

static int odbc_stmt_fetch(pdo_stmt_t *stmt, enum pdo_fetch_orientation ori, zend_long offset) {
    return swr_odbc_fetch(stmt->dbh, stmt, ori, offset, stmt_file, fetch_lines[0], fetch_lines[1]);
}

extern bool swr_odbc_describe(pdo_dbh_t *, pdo_stmt_t *, int, bool, size_t, const char *, const int (*)[3]);

void swr_odbc_column_info(pdo_stmt_t *stmt, int index, const char (*name)[128], size_t length,
                          SQLSMALLINT type, size_t display_size, bool unbound) {
    pdo_odbc_stmt *S = (pdo_odbc_stmt *) stmt->driver_data;
    pdo_odbc_column *column = &S->cols[index];
    stmt->columns[index].name = zend_string_init(*name, length, 0);
    stmt->columns[index].maxlen = column->datalen = display_size;
    column->coltype = type;
    column->is_unicode = pdo_odbc_sqltype_is_unicode(S, type);
    column->is_long = unbound;
    if (unbound) {
        S->going_long = 1;
    }
}

SQLRETURN swr_odbc_bind_column(pdo_stmt_t *stmt, int index) {
    pdo_odbc_stmt *S = (pdo_odbc_stmt *) stmt->driver_data;
    pdo_odbc_column *column = &S->cols[index];
    column->data = ecalloc(column->datalen + 1, 1);
    return SQLBindCol(S->stmt, index + 1, column->is_unicode ? SQL_C_BINARY : SQL_C_CHAR,
                      column->data, column->datalen + 1, &column->fetched_len);
}

static int odbc_stmt_describe(pdo_stmt_t *stmt, int colno) {
    pdo_odbc_stmt *S = (pdo_odbc_stmt *) stmt->driver_data;
    return swr_odbc_describe(stmt->dbh, stmt, colno, S->going_long, LONG_COLUMN_BUFFER_SIZE, stmt_file, &describe_lines);
}

static int odbc_stmt_get_column_meta(pdo_stmt_t *stmt, zend_long colno, zval *return_value) {
    array_init(return_value);
    add_assoc_long(return_value, "pdo_type", PDO_PARAM_STR);
    return 1;
}

extern bool swr_odbc_read_long(pdo_stmt_t *, SQLUSMALLINT, bool, size_t, size_t, zval *);

static int odbc_stmt_get_col(pdo_stmt_t *stmt, int colno, zval *result, enum pdo_param_type *type) {
    pdo_odbc_stmt *S = (pdo_odbc_stmt *) stmt->driver_data;
    pdo_odbc_column *column = &S->cols[colno];
    if (column->is_long) {
        if (!swr_odbc_read_long(stmt, colno + 1, column->is_unicode, LONG_COLUMN_BUFFER_SIZE, column->datalen, result)) {
            return 1;
        }
    } else if (column->fetched_len < 0) {
        ZVAL_NULL(result);
        return 1;
    } else {
        size_t length = (size_t) column->fetched_len;
        ZVAL_STRINGL(result, column->data, length <= column->datalen ? length : column->datalen);
    }
    return 1;
}

extern int swr_odbc_stmt_set_attr(pdo_dbh_t *, pdo_stmt_t *, zend_long, zval *, const char *, int);
extern int swr_odbc_stmt_get_attr(pdo_dbh_t *, pdo_stmt_t *, zend_long, zval *, bool, const char *, int);

void swr_odbc_stmt_set_utf8(pdo_stmt_t *stmt, bool enabled) {
    ((pdo_odbc_stmt *) stmt->driver_data)->assume_utf8 = enabled;
}

static int odbc_stmt_set_param(pdo_stmt_t *stmt, zend_long attr, zval *val) {
    if (attr == PDO_ATTR_CURSOR_NAME) {
        convert_to_string(val);
    }
    return swr_odbc_stmt_set_attr(stmt->dbh, stmt, attr, val, stmt_file, attr_lines[0]);
}

static int odbc_stmt_get_attr(pdo_stmt_t *stmt, zend_long attr, zval *val) {
    pdo_odbc_stmt *S = (pdo_odbc_stmt *) stmt->driver_data;
    return swr_odbc_stmt_get_attr(stmt->dbh, stmt, attr, val, S->assume_utf8, stmt_file, attr_lines[1]);
}

extern bool swr_odbc_next_rowset(pdo_stmt_t *);
extern bool swr_odbc_close_cursor(pdo_stmt_t *);

void swr_odbc_replace_columns(pdo_stmt_t *stmt, SQLSMALLINT count) {
    pdo_odbc_stmt *S = (pdo_odbc_stmt *) stmt->driver_data;
    free_cols(stmt, S);
    stmt->column_count = S->col_count = count;
    S->cols = ecalloc(count, sizeof(pdo_odbc_column));
    S->going_long = 0;
}

static int odbc_stmt_next_rowset(pdo_stmt_t *stmt) {
    return swr_odbc_next_rowset(stmt);
}

static int odbc_stmt_close_cursor(pdo_stmt_t *stmt) {
    return swr_odbc_close_cursor(stmt);
}

const struct pdo_stmt_methods swoole_odbc_stmt_methods = {odbc_stmt_dtor,
                                                          odbc_stmt_execute,
                                                          odbc_stmt_fetch,
                                                          odbc_stmt_describe,
                                                          odbc_stmt_get_col,
                                                          odbc_stmt_param_hook,
                                                          odbc_stmt_set_param,
                                                          odbc_stmt_get_attr,
                                                          odbc_stmt_get_column_meta,
                                                          odbc_stmt_next_rowset,
                                                          odbc_stmt_close_cursor};
#endif
