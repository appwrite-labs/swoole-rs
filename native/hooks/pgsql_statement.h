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
#include "php_swoole.h"
#include <libpq-fe.h>
#include "ext/pdo/php_pdo_driver.h"
#include <sys/uio.h>

typedef struct pdo_pgsql_db_handle pdo_pgsql_db_handle;

bool pdo_pgsql_send_copy_data(pdo_pgsql_db_handle *handle, struct iovec bytes, bool array_row);

struct pdo_pgsql_lob_self {
    zval dbh;
    PGconn *conn;
    int lfd;
    Oid oid;
};

PGconn *pdo_pgsql_server(pdo_pgsql_db_handle *H);
extern const php_stream_ops pdo_pgsql_lob_stream_ops;

bool pdo_pgsql_bind_oid(void *params, size_t index, Oid oid);
bool pdo_pgsql_bind_value(void *params, size_t index, enum pdo_param_type kind, zval *value);
bool pdo_pgsql_emulated_parameter(enum pdo_param_type *kind, zval *value);
bool pdo_pgsql_normalize_parameter(pdo_dbh_t *dbh, pdo_stmt_t *stmt, zend_string *name, zend_long *index);
bool pdo_pgsql_check_parameter(pdo_dbh_t *dbh, pdo_stmt_t *stmt, HashTable *map, zend_long index);
bool pdo_pgsql_bind_parameter(pdo_dbh_t *dbh, pdo_stmt_t *stmt, void *state, HashTable *map, zend_long index, enum pdo_param_type kind, zval *value);
bool pdo_pgsql_execute(pdo_dbh_t *dbh, pdo_stmt_t *stmt, void *state, zend_string *active, unsigned int count);
void pdo_pgsql_destroy_statement(void *state, pdo_pgsql_db_handle *live);
bool pdo_pgsql_fetch_row(pdo_dbh_t *dbh, pdo_stmt_t *stmt, void *state, unsigned int orientation, zend_long offset);
bool pdo_pgsql_read_column(pdo_stmt_t *stmt, pdo_pgsql_db_handle *H, void *state, PGresult *result, int column, enum pdo_param_type *type, zval *output);
struct pdo_column_data;
bool pdo_pgsql_describe_column(PGresult *result, int colno, struct pdo_column_data *column);
int pdo_pgsql_column_meta(pdo_pgsql_db_handle *H, PGresult *result, int colno, zval *output);
PGresult *pdo_pgsql_statement_result(void *state);
const char *pdo_pgsql_statement_name(void *state);
void pdo_pgsql_raise(pdo_dbh_t *dbh, pdo_stmt_t *stmt, int errcode, const char *sqlstate, const char *msg);
#endif
