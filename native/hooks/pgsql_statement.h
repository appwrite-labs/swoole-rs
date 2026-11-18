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

#ifndef SWOOLE_PGSQL_STATEMENT_H
#define SWOOLE_PGSQL_STATEMENT_H
#include "php_swoole_pgsql.h"
#include "ext/pdo/php_pdo_driver.h"
#include <sys/uio.h>

typedef struct pdo_pgsql_db_handle pdo_pgsql_db_handle;
typedef struct {
    Oid pgsql_type;
} pdo_pgsql_column;

typedef struct pdo_pgsql_stmt {
    pdo_pgsql_db_handle *H;
    PGresult *result;
    pdo_pgsql_column *cols;
    char *cursor_name;
    char *stmt_name;
    zend_string *query;
    pdo_pgsql_parameter_arrays params;
    int current_row;
    bool is_prepared;
#if PHP_VERSION_ID >= 80500
    bool is_unbuffered;
    bool is_running_unbuffered;
#endif
} pdo_pgsql_stmt;

struct pdo_pgsql_lob_self {
    zval dbh;
    PGconn *conn;
    int lfd;
    Oid oid;
};

extern const php_stream_ops pdo_pgsql_lob_stream_ops;

bool pdo_pgsql_bind_oid(void *params, size_t index, Oid oid);
bool pdo_pgsql_bind_value(void *params, size_t index, enum pdo_param_type kind, zval *value);
bool pdo_pgsql_emulated_parameter(enum pdo_param_type *kind, zval *value);
bool pdo_pgsql_normalize_parameter(pdo_dbh_t *dbh, pdo_stmt_t *stmt, zend_string *name, zend_long *index);
bool pdo_pgsql_check_parameter(pdo_dbh_t *dbh, pdo_stmt_t *stmt, HashTable *map, zend_long index);
bool pdo_pgsql_execution_result(pdo_dbh_t *dbh, pdo_stmt_t *stmt, PGresult *result);
void pdo_pgsql_close_statement(pdo_pgsql_db_handle *H, const char *name);
bool pdo_pgsql_prepare_statement(pdo_dbh_t *dbh, pdo_stmt_t *stmt, PGresult **result, const char *name, const char *sql, int count, const Oid *types);
void pdo_pgsql_close_cursor(pdo_pgsql_db_handle *H, const char *cursor);
bool pdo_pgsql_open_cursor(pdo_dbh_t *dbh, pdo_stmt_t *stmt, PGresult **result, const char *cursor, bool *prepared, const char *sql);
bool pdo_pgsql_fetch_cursor(pdo_dbh_t *dbh, pdo_stmt_t *stmt, PGresult **result, const char *cursor, unsigned int orientation, zend_long offset);
void pdo_pgsql_scalar_column(Oid oid, struct iovec bytes, zval *output);
struct pdo_column_data;
bool pdo_pgsql_describe_column(PGresult *result, int colno, struct pdo_column_data *column, Oid *oid);
int pdo_pgsql_column_meta(pdo_pgsql_db_handle *H, Oid oid, Oid table, zval *output);
bool pdo_pgsql_bind_stream(pdo_dbh_t *dbh, pdo_stmt_t *stmt, void *params, size_t index, zval *value);
void pdo_pgsql_raise(pdo_dbh_t *dbh, pdo_stmt_t *stmt, int errcode, const char *sqlstate, const char *msg);
#endif
