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

bool pdo_pgsql_end_copy_in(pdo_dbh_t *dbh);

typedef struct pdo_pgsql_db_handle pdo_pgsql_db_handle;

bool pdo_pgsql_send_copy_data(pdo_pgsql_db_handle *handle, struct iovec bytes, bool array_row);
void pdo_pgsql_drain_copy_results(pdo_pgsql_db_handle *handle);
bool pdo_pgsql_start_copy(pdo_dbh_t *dbh, const char *table, const char *fields, unsigned char delimiter,
                          const char *null_marker, ExecStatusType direction);

typedef struct pdo_pgsql_stmt {
    pdo_pgsql_db_handle *H;
    PGresult *result;
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

PGconn *pdo_pgsql_server(pdo_pgsql_db_handle *H);
#if PHP_VERSION_ID >= 80500
php_stream *pdo_pgsql_create_lob_stream(zend_object *dbh, int lfd, Oid oid);
#else
php_stream *pdo_pgsql_create_lob_stream(zval *dbh, int lfd, Oid oid);
#endif

extern const php_stream_ops pdo_pgsql_lob_stream_ops;

bool pdo_pgsql_bind_oid(void *params, size_t index, Oid oid);
bool pdo_pgsql_bind_value(void *params, size_t index, enum pdo_param_type kind, zval *value);
bool pdo_pgsql_emulated_parameter(enum pdo_param_type *kind, zval *value);
bool pdo_pgsql_normalize_parameter(pdo_dbh_t *dbh, pdo_stmt_t *stmt, zend_string *name, zend_long *index);
bool pdo_pgsql_check_parameter(pdo_dbh_t *dbh, pdo_stmt_t *stmt, HashTable *map, zend_long index);
bool pdo_pgsql_execution_result(pdo_dbh_t *dbh, pdo_stmt_t *stmt, PGresult *result);
void pdo_pgsql_finish_result(PGconn *active, PGresult **result, bool abort_query);
void pdo_pgsql_close_statement(pdo_pgsql_db_handle *H, const char *name);
bool pdo_pgsql_send_statement(pdo_dbh_t *dbh, pdo_stmt_t *stmt, PGresult **result, const char *name,
                              const char *query, const void *params, unsigned int count, bool *prepared,
                              bool named, bool unbuffered);
void pdo_pgsql_close_cursor(pdo_pgsql_db_handle *H, const char *cursor);
bool pdo_pgsql_open_cursor(pdo_dbh_t *dbh, pdo_stmt_t *stmt, PGresult **result, const char *cursor, bool *prepared, const char *sql);
bool pdo_pgsql_fetch_row(pdo_dbh_t *dbh, pdo_stmt_t *stmt, PGresult **result, const char *cursor, unsigned int orientation, zend_long offset, int *row, bool *running);
void pdo_pgsql_scalar_column(Oid oid, struct iovec bytes, zval *output);
struct pdo_column_data;
bool pdo_pgsql_describe_column(PGresult *result, int colno, struct pdo_column_data *column);
int pdo_pgsql_column_meta(pdo_pgsql_db_handle *H, PGresult *result, int colno, zval *output);
void pdo_pgsql_raise(pdo_dbh_t *dbh, pdo_stmt_t *stmt, int errcode, const char *sqlstate, const char *msg);
#endif
