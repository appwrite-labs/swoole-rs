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

/* pdo_odbc's connection side that stays C: PDO's method signatures, Zend allocation and
 * the native connection record. crates/odbc/src/driver.rs has the operations. One copy
 * serves every supported PHP version; each supplies the diagnostic locations of its own
 * ext/pdo_odbc/odbc_driver.c. Nothing after the tables below reads __LINE__. */
#include "php_swoole_odbc.h"

#ifdef SW_USE_ODBC
#include "php.h"
#include "ext/pdo/php_pdo.h"
#include "ext/pdo/php_pdo_driver.h"
#include "zend_exceptions.h"

#if PHP_VERSION_ID >= 80500
static const char driver_file[] = "thirdparty/php85/pdo_odbc/odbc_driver.c";
static const int doer_lines[] = {229, 244, 250};
static const int begin_line = 279;
static const int commit_lines[] = {294, 305};
static const int rollback_lines[] = {320, 330};
static const int setup_lines[] = {484, 491, 499, 507, 514, 522};
static const int connect_line = 610;
static const int prepare_lines[] = {176, 186, 203};
/* The line the original's multiline macro call reports, on both GCC and Clang. */
static int attribute_line(void) {
#define SWR_ODBC_LINE() __LINE__
#line 367
    return SWR_ODBC_LINE(

    );
#undef SWR_ODBC_LINE
}
#elif PHP_VERSION_ID >= 80400
static const char driver_file[] = "thirdparty/php84/pdo_odbc/odbc_driver.c";
static const int doer_lines[] = {229, 244, 250};
static const int begin_line = 279;
static const int commit_lines[] = {293, 304};
static const int rollback_lines[] = {318, 328};
static const int setup_lines[] = {476, 483, 491, 499, 508, 516};
static const int connect_line = 619;
static const int prepare_lines[] = {177, 187, 204};
/* The line the original's multiline macro call reports, on both GCC and Clang. */
static int attribute_line(void) {
#define SWR_ODBC_LINE() __LINE__
#line 364
    return SWR_ODBC_LINE(
    );
#undef SWR_ODBC_LINE
}
#elif PHP_VERSION_ID >= 80300
static const char driver_file[] = "thirdparty/php83/pdo_odbc/odbc_driver.c";
static const int doer_lines[] = {231, 246, 252};
static const int begin_line = 281;
static const int commit_lines[] = {295, 306};
static const int rollback_lines[] = {320, 330};
static const int setup_lines[] = {477, 484, 492, 500, 509, 517};
static const int connect_line = 586;
static const int prepare_lines[] = {179, 189, 206};
/* The line the original's multiline macro call reports, on both GCC and Clang. */
static int attribute_line(void) {
#define SWR_ODBC_LINE() __LINE__
#line 366
    return SWR_ODBC_LINE(
    );
#undef SWR_ODBC_LINE
}
#else
static const char driver_file[] = "thirdparty/php82/pdo_odbc/odbc_driver.c";
static const int doer_lines[] = {231, 246, 252};
static const int begin_line = 281;
static const int commit_lines[] = {295, 306};
static const int rollback_lines[] = {320, 330};
static const int setup_lines[] = {477, 484, 492, 500, 509, 517};
static const int connect_line = 586;
static const int prepare_lines[] = {179, 189, 206};
/* The line the original's multiline macro call reports, on both GCC and Clang. */
static int attribute_line(void) {
#define SWR_ODBC_LINE() __LINE__
#line 366
    return SWR_ODBC_LINE(
    );
#undef SWR_ODBC_LINE
}
#endif

extern void swr_odbc_fetch_error(pdo_dbh_t *, pdo_stmt_t *, zval *);

/* The native statement still owns its record. Rust reads the same error
 * layout for connection and statement diagnostics. */
_Static_assert(SQL_MAX_MESSAGE_LENGTH == 512, "ODBC diagnostic buffer layout");
pdo_odbc_errinfo *swr_odbc_error_record(pdo_dbh_t *dbh, pdo_stmt_t *stmt) {
    if (stmt) {
        return &((pdo_odbc_stmt *) stmt->driver_data)->einfo;
    }
    return &((pdo_odbc_db_handle *) dbh->driver_data)->einfo;
}

SQLHENV swr_odbc_environment(pdo_dbh_t *dbh) {
    return ((pdo_odbc_db_handle *) dbh->driver_data)->env;
}

SQLHSTMT swr_odbc_statement(pdo_stmt_t *stmt) {
    return ((pdo_odbc_stmt *) stmt->driver_data)->stmt;
}

void swr_odbc_connect_exception(pdo_dbh_t *dbh) {
    const pdo_odbc_errinfo *error = swr_odbc_error_record(dbh, NULL);
    zend_throw_exception_ex(php_pdo_get_exception(), error->last_error, "SQLSTATE[%s] %s: %d %s",
                            dbh->error_code, error->what, error->last_error, error->last_err_msg);
}

extern void swr_odbc_close(pdo_dbh_t *, SQLHENV);

void swr_odbc_free_record(pdo_dbh_t *dbh) {
    pefree(dbh->driver_data, dbh->is_persistent);
    dbh->driver_data = NULL;
}

static void odbc_handle_closer(pdo_dbh_t *dbh) {
    pdo_odbc_db_handle *handle = (pdo_odbc_db_handle *) dbh->driver_data;
    swr_odbc_close(dbh, handle->env);
}

extern zend_long swr_odbc_doer(pdo_dbh_t *, const zend_string *, const char *, int, int, int);

static zend_long odbc_handle_doer(pdo_dbh_t *dbh, const zend_string *sql) {
    return swr_odbc_doer(dbh, sql, driver_file, doer_lines[0], doer_lines[1], doer_lines[2]);
}

SQLHDBC swr_odbc_connection(pdo_dbh_t *dbh) {
    return ((pdo_odbc_db_handle *)dbh->driver_data)->dbc;
}

extern bool swr_odbc_begin(pdo_dbh_t *, const char *, int);
extern bool swr_odbc_finish(pdo_dbh_t *, bool, const char *, int, int);

static bool odbc_handle_begin(pdo_dbh_t *dbh) {
    return swr_odbc_begin(dbh, driver_file, begin_line);
}
static bool odbc_handle_commit(pdo_dbh_t *dbh) {
    return swr_odbc_finish(dbh, true, driver_file, commit_lines[0], commit_lines[1]);
}
static bool odbc_handle_rollback(pdo_dbh_t *dbh) {
    return swr_odbc_finish(dbh, false, driver_file, rollback_lines[0], rollback_lines[1]);
}

void swr_odbc_set_utf8(pdo_dbh_t *dbh, bool on) {
    ((pdo_odbc_db_handle *) dbh->driver_data)->assume_utf8 = on;
}

extern bool swr_odbc_set_attr(pdo_dbh_t *, zend_long, zval *, const char *, int);
static bool odbc_handle_set_attr(pdo_dbh_t *dbh, zend_long attr, zval *value) {
    return swr_odbc_set_attr(dbh, attr, value, driver_file, attribute_line());
}

extern int swr_odbc_get_attr(pdo_dbh_t *, zend_long, zval *, bool, const char *);
extern zend_result swr_odbc_check_liveness(pdo_dbh_t *);

static int odbc_handle_get_attr(pdo_dbh_t *dbh, zend_long attr, zval *value) {
    pdo_odbc_db_handle *handle = (pdo_odbc_db_handle *) dbh->driver_data;
    return swr_odbc_get_attr(dbh, attr, value, handle->assume_utf8, "ODBC-" PDO_ODBC_TYPE);
}

void swr_odbc_publish_statement(pdo_stmt_t *stmt, SQLHSTMT handle) {
    ((pdo_odbc_stmt *) stmt->driver_data)->stmt = handle;
}

void swr_odbc_prepare_error(pdo_dbh_t *dbh, pdo_stmt_t *stmt) {
    pdo_odbc_errinfo *source = swr_odbc_error_record(dbh, stmt);
    pdo_odbc_errinfo *dest = swr_odbc_error_record(dbh, NULL);
    strcpy(dest->last_err_msg, source->last_err_msg);
    dest->file = source->file;
    dest->line = source->line;
    dest->what = source->what;
    strcpy(dbh->error_code, stmt->error_code);
}

extern bool swr_odbc_prepare(pdo_dbh_t *, pdo_stmt_t *, const zend_string *, zval *, const char *, const int (*)[3]);
static bool odbc_handle_preparer(pdo_dbh_t *dbh, zend_string *sql, pdo_stmt_t *stmt, zval *options) {
    zend_string *rewritten = NULL;
    stmt->supports_placeholders = PDO_PLACEHOLDER_POSITIONAL;
    if (pdo_parse_params(stmt, sql, &rewritten) == -1) {
        strcpy(dbh->error_code, stmt->error_code);
        return false;
    }
    pdo_odbc_stmt *record = ecalloc(1, sizeof(*record));
    record->H = dbh->driver_data;
    record->assume_utf8 = record->H->assume_utf8;
    stmt->driver_data = record;
    bool ok = swr_odbc_prepare(dbh, stmt, rewritten ? rewritten : sql, options, driver_file, &prepare_lines);
    if (rewritten) {
        zend_string_release(rewritten);
    }
    if (!record->stmt) {
        efree(record);
        stmt->driver_data = NULL;
        return false;
    }
    stmt->methods = &swoole_odbc_stmt_methods;
    return ok;
}

static const struct pdo_dbh_methods odbc_methods = {
    .closer = odbc_handle_closer,
    .preparer = odbc_handle_preparer,
    .doer = odbc_handle_doer,
    .begin = odbc_handle_begin,
    .commit = odbc_handle_commit,
    .rollback = odbc_handle_rollback,
    .set_attribute = odbc_handle_set_attr,
    .fetch_err = swr_odbc_fetch_error,
    .get_attribute = odbc_handle_get_attr,
    .check_liveness = swr_odbc_check_liveness,
};

extern bool swr_odbc_connect(pdo_dbh_t *, const char *, size_t, const char *, const char *, const char *, int);
void swr_odbc_replace_source(pdo_dbh_t *dbh, const char *source) {
    size_t length = strlen(source);
    char *copy = pestrndup(source, length, dbh->is_persistent);
    pefree((char *) dbh->data_source, dbh->is_persistent);
    dbh->data_source = copy;
#if PHP_VERSION_ID >= 80500
    dbh->data_source_len = length;
#endif
}

extern bool swr_odbc_setup(pdo_dbh_t *, zval *, SQLHENV *, SQLHDBC *, const char *, const int (*)[6]);

static int swr_odbc_factory(pdo_dbh_t *dbh, zval *driver_options) {
    pdo_odbc_db_handle *H = pecalloc(1, sizeof(*H), dbh->is_persistent);
    dbh->driver_data = H;
    bool connected = swr_odbc_setup(dbh, driver_options, &H->env, &H->dbc, driver_file, &setup_lines)
        && swr_odbc_connect(dbh, dbh->data_source, dbh->data_source_len + 1,
                           dbh->username, dbh->password, driver_file, connect_line);
    dbh->methods = &odbc_methods;
    if (connected) {
        dbh->alloc_own_columns = 1;
    }
    return connected;
}

/* PDO_DRIVER_HEADER takes the API version from the PHP headers used to build this module. */
const pdo_driver_t *swr_odbc_descriptor(void) {
    static const pdo_driver_t driver = {PDO_DRIVER_HEADER(odbc), swr_odbc_factory};
    return &driver;
}
#endif
