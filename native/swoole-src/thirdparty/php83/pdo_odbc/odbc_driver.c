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
#include "zend_exceptions.h"
#include <php_odbc_utils.h>

static const char driver_file[] = __FILE__;
static const int doer_lines[] = {231, 246, 252};
static const int begin_line = 281;
static const int commit_lines[] = {295, 306};
static const int rollback_lines[] = {320, 330};
static const int setup_lines[] = {477, 484, 492, 500, 509, 517};
static const int connect_line = 586;
/* Match the original multiline macro expansion on both GCC and Clang. */
static int attribute_line(void) {
#define SWR_ODBC_LINE() __LINE__
#line 366
    return SWR_ODBC_LINE(
    );
#undef SWR_ODBC_LINE
}

#line 146
static bool odbc_handle_preparer(pdo_dbh_t *dbh, zend_string *sql, pdo_stmt_t *stmt, zval *driver_options) {
    RETCODE rc;
    pdo_odbc_db_handle *H = (pdo_odbc_db_handle *) dbh->driver_data;
    pdo_odbc_stmt *S = ecalloc(1, sizeof(*S));
    enum pdo_cursor_type cursor_type = PDO_CURSOR_FWDONLY;
    int ret;
    zend_string *nsql = NULL;

    S->H = H;
    S->assume_utf8 = H->assume_utf8;

    /* before we prepare, we need to peek at the query; if it uses named parameters,
     * we want PDO to rewrite them for us */
    stmt->supports_placeholders = PDO_PLACEHOLDER_POSITIONAL;
    ret = pdo_parse_params(stmt, sql, &nsql);

    if (ret == 1) {
        /* query was re-written */
        sql = nsql;
    } else if (ret == -1) {
        /* couldn't grok it */
        strcpy(dbh->error_code, stmt->error_code);
        efree(S);
        return false;
    }

    rc = SQLAllocHandle(SQL_HANDLE_STMT, H->dbc, &S->stmt);

    if (rc == SQL_INVALID_HANDLE || rc == SQL_ERROR) {
        efree(S);
        if (nsql) {
            zend_string_release(nsql);
        }
        pdo_odbc_drv_error("SQLAllocStmt");
        return false;
    }

    stmt->driver_data = S;

    cursor_type = pdo_attr_lval(driver_options, PDO_ATTR_CURSOR, PDO_CURSOR_FWDONLY);
    if (cursor_type != PDO_CURSOR_FWDONLY) {
        rc = SQLSetStmtAttr(S->stmt, SQL_ATTR_CURSOR_SCROLLABLE, (void *) SQL_SCROLLABLE, 0);
        if (rc != SQL_SUCCESS && rc != SQL_SUCCESS_WITH_INFO) {
            pdo_odbc_stmt_error("SQLSetStmtAttr: SQL_ATTR_CURSOR_SCROLLABLE");
            SQLFreeHandle(SQL_HANDLE_STMT, S->stmt);
            if (nsql) {
                zend_string_release(nsql);
            }
            return false;
        }
    }

    rc = SQLPrepare(S->stmt, (SQLCHAR *) ZSTR_VAL(sql), SQL_NTS);
    if (nsql) {
        zend_string_release(nsql);
    }

    stmt->methods = &odbc_stmt_methods;

    if (rc != SQL_SUCCESS) {
        pdo_odbc_stmt_error("SQLPrepare");
        if (rc != SQL_SUCCESS_WITH_INFO) {
            /* clone error information into the db handle */
            strcpy(H->einfo.last_err_msg, S->einfo.last_err_msg);
            H->einfo.file = S->einfo.file;
            H->einfo.line = S->einfo.line;
            H->einfo.what = S->einfo.what;
            strcpy(dbh->error_code, stmt->error_code);
        }
    }

    if (rc != SQL_SUCCESS && rc != SQL_SUCCESS_WITH_INFO) {
        return false;
    }
    return true;
}

#include "odbc_driver_abi.h"
#endif
