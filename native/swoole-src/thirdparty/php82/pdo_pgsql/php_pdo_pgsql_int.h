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

#ifndef PHP_PDO_PGSQL_INT_H
#define PHP_PDO_PGSQL_INT_H

#include <sys/uio.h>
#include <libpq-fe.h>
#include <libpq/libpq-fs.h>
#include <php.h>

#define PHP_PDO_PGSQL_CONNECTION_FAILURE_SQLSTATE "08006"

/* The database handle is Rust's (crates/pgsql/src/driver.rs), which also implements
 * the handle's methods: the statement, COPY, LOB and notice code reach its state only
 * through these accessors. */
typedef struct pdo_pgsql_db_handle pdo_pgsql_db_handle;

bool pdo_pgsql_fetch_cursor(pdo_dbh_t *dbh, pdo_stmt_t *stmt, PGresult **result, const char *cursor, unsigned int orientation, zend_long offset);
void pdo_pgsql_scalar_column(Oid oid, struct iovec bytes, zval *output);
struct pdo_column_data;
bool pdo_pgsql_describe_column(PGresult *result, int colno, struct pdo_column_data *column, Oid *oid);
int pdo_pgsql_column_meta(pdo_pgsql_db_handle *H, Oid oid, Oid table, zval *output);
PGconn *pdo_pgsql_server(pdo_pgsql_db_handle *H);
unsigned int pdo_pgsql_next_stmt_id(pdo_pgsql_db_handle *H);
bool pdo_pgsql_emulate_prepares(pdo_pgsql_db_handle *H);
bool pdo_pgsql_disable_prepares(pdo_pgsql_db_handle *H);
HashTable **pdo_pgsql_lob_streams(pdo_pgsql_db_handle *H);

typedef struct {
	Oid          pgsql_type;
} pdo_pgsql_column;

typedef struct {
	pdo_pgsql_db_handle     *H;
	PGresult                *result;
	pdo_pgsql_column        *cols;
	char *cursor_name;
	char *stmt_name;
	zend_string *query;
	char **param_values;
	int *param_lengths;
	int *param_formats;
	Oid *param_types;
	int                     current_row;
	bool is_prepared;
} pdo_pgsql_stmt;

typedef struct {
	Oid     oid;
} pdo_pgsql_bound_param;

extern const pdo_driver_t pdo_pgsql_driver;

int pdo_pgsql_connect(pdo_dbh_t *dbh, const char *data_source, const char *username, const char *password,
                      zval *driver_options);
void pdo_pgsql_free(pdo_pgsql_db_handle *H);
zend_long pdo_pgsql_handle_doer(pdo_dbh_t *dbh, const zend_string *sql);
zend_string *pdo_pgsql_handle_quoter(pdo_dbh_t *dbh, const zend_string *unquoted, enum pdo_param_type paramtype);
bool pdo_pgsql_handle_begin(pdo_dbh_t *dbh);
bool pdo_pgsql_handle_commit(pdo_dbh_t *dbh);
bool pdo_pgsql_handle_rollback(pdo_dbh_t *dbh);
bool pdo_pgsql_set_attr(pdo_dbh_t *dbh, zend_long attr, zval *val);
zend_string *pdo_pgsql_last_insert_id(pdo_dbh_t *dbh, const zend_string *name);
void pdo_pgsql_fetch_error_func(pdo_dbh_t *dbh, pdo_stmt_t *stmt, zval *info);
int pdo_pgsql_get_attribute(pdo_dbh_t *dbh, zend_long attr, zval *return_value);
zend_result pdo_pgsql_check_liveness(pdo_dbh_t *dbh);
bool pdo_pgsql_handle_in_transaction(pdo_dbh_t *dbh);
extern const struct pdo_dbh_methods swoole_pdo_pgsql_methods;

/* Called by Rust: the notice processor, and the close on a failed connect. */
void pdo_pgsql_notice(void *context, const char *message);
void pdo_pgsql_handle_closer(pdo_dbh_t *dbh);

void pdo_pgsql_raise(pdo_dbh_t *dbh, pdo_stmt_t *stmt, int errcode, const char *sqlstate, const char *msg);
#define pdo_pgsql_error(d, e, z) pdo_pgsql_raise(d, NULL, e, z, NULL)
#define pdo_pgsql_error_msg(d, e, m) pdo_pgsql_raise(d, NULL, e, NULL, m)
#define pdo_pgsql_error_stmt(s, e, z) pdo_pgsql_raise(s->dbh, s, e, z, NULL)
#define pdo_pgsql_error_stmt_msg(stmt, e, sqlstate, msg) pdo_pgsql_raise(stmt->dbh, stmt, e, sqlstate, msg)

extern const struct pdo_stmt_methods swoole_pgsql_stmt_methods;

#define pdo_pgsql_sqlstate(r) PQresultErrorField(r, PG_DIAG_SQLSTATE)

enum {
	PDO_PGSQL_ATTR_DISABLE_PREPARES = PDO_ATTR_DRIVER_SPECIFIC,
};

struct pdo_pgsql_lob_self {
	zval dbh;
	PGconn *conn;
	int lfd;
	Oid oid;
};

enum pdo_pgsql_specific_constants {
	PGSQL_TRANSACTION_IDLE = PQTRANS_IDLE,
	PGSQL_TRANSACTION_ACTIVE = PQTRANS_ACTIVE,
	PGSQL_TRANSACTION_INTRANS = PQTRANS_INTRANS,
	PGSQL_TRANSACTION_INERROR = PQTRANS_INERROR,
	PGSQL_TRANSACTION_UNKNOWN = PQTRANS_UNKNOWN
};

php_stream *pdo_pgsql_create_lob_stream(zval *pdh, int lfd, Oid oid);
extern const php_stream_ops pdo_pgsql_lob_stream_ops;

void pdo_pgsql_close_lob_streams(pdo_dbh_t *dbh);

#endif /* PHP_PDO_PGSQL_INT_H */
