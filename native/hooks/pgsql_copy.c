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
#include "ext/pdo/php_pdo_error.h"

#include "ext/standard/file.h"

zend_string *pdo_pgsql_copy_query(const char *table, const char *fields, unsigned char delimiter,
                                  const char *null_marker, ExecStatusType direction);

bool pdo_pgsql_read_copy_row(pdo_dbh_t *dbh, char **row, size_t *length);

static bool receive_copy_rows(pdo_dbh_t *dbh, php_stream *stream, zval *rows)
{
    for (;;) {
        char *row = NULL;
        size_t length = 0;
        if (!pdo_pgsql_read_copy_row(dbh, &row, &length)) {
            return false;
        }
        if (!row) {
            return true;
        }
        if (stream) {
            ssize_t written = php_stream_write(stream, row, length);
            PQfreemem(row);
            if (written < 0 || (size_t)written != length) {
                pdo_pgsql_error_msg(dbh, PGRES_FATAL_ERROR, "Unable to write to file");
                return false;
            }
        } else {
            add_next_index_stringl(rows, row, length);
            PQfreemem(row);
        }
    }
}

static bool start_copy(pdo_dbh_t *dbh, const char *table, const char *fields, unsigned char delimiter,
                       const char *null_marker, ExecStatusType direction)
{
    zend_string *sql = pdo_pgsql_copy_query(table, fields, delimiter, null_marker, direction);
    bool started = pdo_pgsql_start_copy(dbh, ZSTR_VAL(sql), direction);
    zend_string_release(sql);
    return started;
}

void pdo_pgsql_abort_copy_in(pdo_pgsql_db_handle *handle);

static bool send_row(pdo_pgsql_db_handle *handle, zval *row)
{
    return row && try_convert_to_string(row)
        && pdo_pgsql_send_copy_data(handle, (struct iovec){Z_STRVAL_P(row), Z_STRLEN_P(row)}, true);
}

static bool send_rows(pdo_pgsql_db_handle *handle, zval *rows)
{
    zval *row;
#if PHP_VERSION_ID >= 80500
    if (Z_TYPE_P(rows) != IS_ARRAY) {
        zend_object_iterator *iter = Z_OBJCE_P(rows)->get_iterator(Z_OBJCE_P(rows), rows, 0);
        if (!iter) {
            return false;
        }
        if (!EG(exception) && iter->funcs->rewind) {
            iter->funcs->rewind(iter);
        }
        bool sent = true;
        while (!EG(exception) && iter->funcs->valid(iter) == SUCCESS && !EG(exception)) {
            row = iter->funcs->get_current_data(iter);
            if (EG(exception) || !send_row(handle, row)) {
                sent = false;
                break;
            }
            iter->funcs->move_forward(iter);
        }
        zend_iterator_dtor(iter);
        return sent && !EG(exception);
    }
#endif
    ZEND_HASH_FOREACH_VAL(Z_ARRVAL_P(rows), row) {
        if (!send_row(handle, row)) {
            return false;
        }
    } ZEND_HASH_FOREACH_END();
    return true;
}

void pgsqlCopyFromArray_internal(INTERNAL_FUNCTION_PARAMETERS)
{
    pdo_dbh_t *dbh;
    pdo_pgsql_db_handle *H;

    zval *pg_rows;

    char *table_name, *pg_delim = NULL, *pg_null_as = NULL, *pg_fields = NULL;
    size_t table_name_len, pg_delim_len = 0, pg_null_as_len = 0, pg_fields_len;

    ZEND_PARSE_PARAMETERS_START(2, 5)
        Z_PARAM_STRING(table_name, table_name_len)
#if PHP_VERSION_ID >= 80500
        Z_PARAM_ITERABLE(pg_rows)
#else
        Z_PARAM_ARRAY(pg_rows)
#endif
        Z_PARAM_OPTIONAL
        Z_PARAM_STRING(pg_delim, pg_delim_len)
        Z_PARAM_STRING(pg_null_as, pg_null_as_len)
        Z_PARAM_STRING_OR_NULL(pg_fields, pg_fields_len)
    ZEND_PARSE_PARAMETERS_END();
#if PHP_VERSION_ID < 80500
    if (!zend_hash_num_elements(Z_ARRVAL_P(pg_rows))) {
        zend_argument_value_error(2, "cannot be empty");
        RETURN_THROWS();
    }
#endif

    dbh = Z_PDO_DBH_P(ZEND_THIS);
    PDO_CONSTRUCT_CHECK;
    PDO_DBH_CLEAR_ERR();

    H = (pdo_pgsql_db_handle *)dbh->driver_data;

    pdo_pgsql_drain_copy_results(H);
    bool started = start_copy(dbh, table_name, pg_fields, pg_delim_len ? *pg_delim : '\t',
                              pg_null_as_len ? pg_null_as : NULL, PGRES_COPY_IN);

    if (started && !send_rows(H, pg_rows)) {
        if (!EG(exception)) {
            pdo_pgsql_error(dbh, PGRES_FATAL_ERROR, NULL);
        }
        pdo_pgsql_abort_copy_in(H);
        if (EG(exception)) {
            RETURN_THROWS();
        }
        started = false;
    }
    bool complete = started && pdo_pgsql_end_copy_in(dbh);
    PDO_HANDLE_DBH_ERR();
    RETURN_BOOL(complete);
}

PHP_METHOD(PDO_PGSql_Ext, pgsqlCopyFromArray)
{
    pgsqlCopyFromArray_internal(INTERNAL_FUNCTION_PARAM_PASSTHRU);
}

void pgsqlCopyFromFile_internal(INTERNAL_FUNCTION_PARAMETERS)
{
    pdo_dbh_t *dbh;
    pdo_pgsql_db_handle *H;

    char *table_name, *filename, *pg_delim = NULL, *pg_null_as = NULL, *pg_fields = NULL;
    size_t  table_name_len, filename_len, pg_delim_len = 0, pg_null_as_len = 0, pg_fields_len;
    php_stream *stream;

    if (zend_parse_parameters(ZEND_NUM_ARGS(), "sp|sss!",
        &table_name, &table_name_len, &filename, &filename_len,
        &pg_delim, &pg_delim_len, &pg_null_as, &pg_null_as_len, &pg_fields, &pg_fields_len) == FAILURE) {
        RETURN_THROWS();
    }

    /* Obtain db Handler */
    dbh = Z_PDO_DBH_P(ZEND_THIS);
    PDO_CONSTRUCT_CHECK;
    PDO_DBH_CLEAR_ERR();

    stream = php_stream_open_wrapper_ex(filename, "rb", 0, NULL, FG(default_context));
    if (!stream) {
        pdo_pgsql_error_msg(dbh, PGRES_FATAL_ERROR, "Unable to open the file");
        PDO_HANDLE_DBH_ERR();
        RETURN_FALSE;
    }

    H = (pdo_pgsql_db_handle *)dbh->driver_data;

    pdo_pgsql_drain_copy_results(H);
    bool started = start_copy(dbh, table_name, pg_fields, pg_delim_len ? *pg_delim : '\t',
                              pg_null_as_len ? pg_null_as : NULL, PGRES_COPY_IN);

    bool sent = started;
    if (started) {
        size_t length;
        char *buf;
        while ((buf = php_stream_get_line(stream, NULL, 0, &length)) != NULL) {
            sent = pdo_pgsql_send_copy_data(H, (struct iovec){buf, length}, false);
            efree(buf);
            if (!sent) {
                pdo_pgsql_error(dbh, PGRES_FATAL_ERROR, NULL);
                break;
            }
        }
    }
    php_stream_close(stream);
    if (started && (!sent || EG(exception))) {
        pdo_pgsql_abort_copy_in(H);
        if (EG(exception)) {
            RETURN_THROWS();
        }
    }
    bool complete = sent && pdo_pgsql_end_copy_in(dbh);
    PDO_HANDLE_DBH_ERR();
    RETURN_BOOL(complete);
}

PHP_METHOD(PDO_PGSql_Ext, pgsqlCopyFromFile)
{
    pgsqlCopyFromFile_internal(INTERNAL_FUNCTION_PARAM_PASSTHRU);
}

void pgsqlCopyToFile_internal(INTERNAL_FUNCTION_PARAMETERS)
{
    pdo_dbh_t *dbh;
    pdo_pgsql_db_handle *H;

    char *table_name, *pg_delim = NULL, *pg_null_as = NULL, *pg_fields = NULL, *filename = NULL;
    size_t table_name_len, pg_delim_len = 0, pg_null_as_len = 0, pg_fields_len, filename_len;

    php_stream *stream;

    if (zend_parse_parameters(ZEND_NUM_ARGS(), "sp|sss!",
                    &table_name, &table_name_len, &filename, &filename_len,
                    &pg_delim, &pg_delim_len, &pg_null_as, &pg_null_as_len, &pg_fields, &pg_fields_len) == FAILURE) {
        RETURN_THROWS();
    }

    dbh = Z_PDO_DBH_P(ZEND_THIS);
    PDO_CONSTRUCT_CHECK;
    PDO_DBH_CLEAR_ERR();

    H = (pdo_pgsql_db_handle *)dbh->driver_data;

    stream = php_stream_open_wrapper_ex(filename, "wb", 0, NULL, FG(default_context));
    if (!stream) {
        pdo_pgsql_error_msg(dbh, PGRES_FATAL_ERROR, "Unable to open the file for writing");
        PDO_HANDLE_DBH_ERR();
        RETURN_FALSE;
    }

    pdo_pgsql_drain_copy_results(H);

    bool started = start_copy(dbh, table_name, pg_fields, pg_delim_len ? *pg_delim : '\t',
                              pg_null_as_len ? pg_null_as : NULL, PGRES_COPY_OUT);

    bool complete = started && receive_copy_rows(dbh, stream, NULL);
    php_stream_close(stream);
    if (complete) {
        pdo_pgsql_drain_copy_results(H);
        RETURN_TRUE;
    }
    PDO_HANDLE_DBH_ERR();
    RETURN_FALSE;
}

PHP_METHOD(PDO_PGSql_Ext, pgsqlCopyToFile)
{
    pgsqlCopyToFile_internal(INTERNAL_FUNCTION_PARAM_PASSTHRU);
}

void pgsqlCopyToArray_internal(INTERNAL_FUNCTION_PARAMETERS)
{
    pdo_dbh_t *dbh;
    pdo_pgsql_db_handle *H;

    char *table_name, *pg_delim = NULL, *pg_null_as = NULL, *pg_fields = NULL;
    size_t table_name_len, pg_delim_len = 0, pg_null_as_len = 0, pg_fields_len;

    if (zend_parse_parameters(ZEND_NUM_ARGS(), "s|sss!",
        &table_name, &table_name_len,
        &pg_delim, &pg_delim_len, &pg_null_as, &pg_null_as_len, &pg_fields, &pg_fields_len) == FAILURE) {
        RETURN_THROWS();
    }

    dbh = Z_PDO_DBH_P(ZEND_THIS);
    PDO_CONSTRUCT_CHECK;
    PDO_DBH_CLEAR_ERR();

    H = (pdo_pgsql_db_handle *)dbh->driver_data;

    pdo_pgsql_drain_copy_results(H);

    bool started = start_copy(dbh, table_name, pg_fields, pg_delim_len ? *pg_delim : '\t',
                              pg_null_as_len ? pg_null_as : NULL, PGRES_COPY_OUT);

    if (started) {
        array_init(return_value);
        if (receive_copy_rows(dbh, NULL, return_value)) {
            pdo_pgsql_drain_copy_results(H);
            return;
        }
        zval_ptr_dtor(return_value);
        ZVAL_FALSE(return_value);
    }
    PDO_HANDLE_DBH_ERR();
    RETURN_FALSE;
}

PHP_METHOD(PDO_PGSql_Ext, pgsqlCopyToArray)
{
    pgsqlCopyToArray_internal(INTERNAL_FUNCTION_PARAM_PASSTHRU);
}


#endif
