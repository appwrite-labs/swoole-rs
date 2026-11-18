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

#include "php_swoole_odbc.h"

#if PHP_VERSION_ID >= 80300 && PHP_VERSION_ID < 80400
#include "php.h"
#include "php_ini.h"
#include "ext/standard/info.h"
#include "pdo/php_pdo.h"
#include "pdo/php_pdo_driver.h"

enum pdo_odbc_conv_result { PDO_ODBC_CONV_NOT_REQUIRED, PDO_ODBC_CONV_OK, PDO_ODBC_CONV_FAIL };

static int pdo_odbc_sqltype_is_unicode(pdo_odbc_stmt *S, SWORD sqltype) {
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

static int pdo_odbc_utf82ucs2(
    pdo_stmt_t *stmt, int is_unicode, const char *buf, zend_ulong buflen, zend_ulong *outlen) {
#ifdef PHP_WIN32
    if (is_unicode && buflen) {
        pdo_odbc_stmt *S = (pdo_odbc_stmt *) stmt->driver_data;
        DWORD ret;

        ret = MultiByteToWideChar(CP_UTF8, 0, buf, buflen, NULL, 0);
        if (ret == 0) {
            /*printf("%s:%d %d [%d] %.*s\n", __FILE__, __LINE__, GetLastError(), buflen, buflen, buf);*/
            return PDO_ODBC_CONV_FAIL;
        }

        ret *= sizeof(WCHAR);

        if (S->convbufsize <= ret) {
            S->convbufsize = ret + sizeof(WCHAR);
            S->convbuf = erealloc(S->convbuf, S->convbufsize);
        }

        ret = MultiByteToWideChar(CP_UTF8, 0, buf, buflen, (LPWSTR) S->convbuf, S->convbufsize / sizeof(WCHAR));
        if (ret == 0) {
            /*printf("%s:%d %d [%d] %.*s\n", __FILE__, __LINE__, GetLastError(), buflen, buflen, buf);*/
            return PDO_ODBC_CONV_FAIL;
        }

        ret *= sizeof(WCHAR);
        *outlen = ret;
        return PDO_ODBC_CONV_OK;
    }
#endif
    return PDO_ODBC_CONV_NOT_REQUIRED;
}

static int pdo_odbc_ucs22utf8(pdo_stmt_t *stmt, int is_unicode, zval *result) {
#ifdef PHP_WIN32
    ZEND_ASSERT(Z_TYPE_P(result) == IS_STRING);
    if (is_unicode && Z_STRLEN_P(result) != 0) {
        pdo_odbc_stmt *S = (pdo_odbc_stmt *) stmt->driver_data;
        DWORD ret;

        ret = WideCharToMultiByte(
            CP_UTF8, 0, (LPCWSTR) Z_STRVAL_P(result), Z_STRLEN_P(result) / sizeof(WCHAR), NULL, 0, NULL, NULL);
        if (ret == 0) {
            return PDO_ODBC_CONV_FAIL;
        }

        zend_string *str = zend_string_alloc(ret, 0);
        ret = WideCharToMultiByte(CP_UTF8,
                                  0,
                                  (LPCWSTR) Z_STRVAL_P(result),
                                  Z_STRLEN_P(result) / sizeof(WCHAR),
                                  ZSTR_VAL(str),
                                  ZSTR_LEN(str),
                                  NULL,
                                  NULL);
        if (ret == 0) {
            return PDO_ODBC_CONV_FAIL;
        }

        ZSTR_VAL(str)[ret] = '\0';
        zval_ptr_dtor_str(result);
        ZVAL_STR(result, str);
        return PDO_ODBC_CONV_OK;
    }
#endif
    return PDO_ODBC_CONV_NOT_REQUIRED;
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
    if (S->convbuf) {
        efree(S->convbuf);
    }
    efree(S);

    return 1;
}

static const int bind_lines[] = {373, 391};
static const char stmt_file[] = __FILE__;
#define ODBC_CONVERT_RESULT(s, u, z) pdo_odbc_ucs22utf8(s, u, z)
static const int execute_lines[] = {250, 254, 216};
static const int fetch_lines[] = {543, 552};
static const int describe_lines[] = {591, 600, 626};
static const int attr_lines[] = {774, 802};
#define LONG_COLUMN_BUFFER_SIZE 256
#include "odbc_statement_abi.h"

const struct pdo_stmt_methods odbc_stmt_methods = {odbc_stmt_dtor,
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
