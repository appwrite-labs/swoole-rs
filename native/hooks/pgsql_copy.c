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

/* Transfer counted COPY bytes to a Zend string before freeing libpq's buffer. */
int pdo_pgsql_copy_row(PGconn *conn, zval *row)
{
    char *data = NULL;
    int count = PQgetCopyData(conn, &data, 0);
    if (count > 0) {
        ZVAL_STRINGL(row, data, count);
        PQfreemem(data);
    }
    return count;
}

php_stream *pdo_pgsql_copy_open(const char *path, bool writing)
{
    return php_stream_open_wrapper_ex(path, writing ? "wb" : "rb", 0, NULL, FG(default_context));
}

bool pdo_pgsql_copy_write(php_stream *stream, zend_string *row)
{
    ssize_t size = php_stream_write(stream, ZSTR_VAL(row), ZSTR_LEN(row));
    return size >= 0 && (size_t)size == ZSTR_LEN(row);
}

void pdo_pgsql_copy_close(php_stream *stream)
{
    php_stream_close(stream);
}

bool pdo_pgsql_copy_line(php_stream *stream, zval *row)
{
    size_t length;
    char *line = php_stream_get_line(stream, NULL, 0, &length);
    if (!line) {
        return false;
    }
    ZVAL_STRINGL(row, line, length);
    efree(line);
    return true;
}

bool pdo_pgsql_check_rows(zval *rows)
{
#if PHP_VERSION_ID >= 80500
    if (!zend_is_iterable(rows)) {
        zend_wrong_parameter_type_error(2, Z_EXPECTED_ITERABLE, rows);
        return false;
    }
#endif
    return true;
}

static bool send_row(pdo_pgsql_db_handle *handle, zval *row)
{
    return row && try_convert_to_string(row)
        && pdo_pgsql_send_copy_data(handle, (struct iovec){Z_STRVAL_P(row), Z_STRLEN_P(row)}, true);
}

bool pdo_pgsql_send_rows(pdo_pgsql_db_handle *handle, zval *rows)
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

#endif
