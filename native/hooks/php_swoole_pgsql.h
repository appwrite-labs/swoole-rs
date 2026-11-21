/* The coroutine libpq crates/pgsql exports (--enable-swoole-pgsql), and the
 * macros that point the bundled pdo_pgsql driver at it. The driver includes
 * this first, after defining SW_USE_PGSQL_HOOK. */
#ifndef PHP_SWOOLE_PGSQL_H
#define PHP_SWOOLE_PGSQL_H
#include "php_swoole.h"
#ifdef SW_USE_PGSQL
#include <libpq-fe.h>
BEGIN_EXTERN_C()
struct _pdo_dbh_t;
PGconn *swoole_pgsql_connectdb(const char *conninfo);
PGresult *swoole_pgsql_prepare(PGconn *conn, const char *stmt_name, const char *query, int n_params, const Oid *param_types);
PGresult *swoole_pgsql_exec_prepared(PGconn *conn, const char *stmt_name, int n_params, const char *const *param_values,
                                     const int *param_lengths, const int *param_formats, int result_format);
PGresult *swoole_pgsql_exec(PGconn *conn, const char *query);
PGresult *swoole_pgsql_exec_params(PGconn *conn, const char *command, int n_params, const Oid *param_types,
                                   const char *const *param_values, const int *param_lengths, const int *param_formats,
                                   int result_format);
#ifdef HAVE_PQCLOSEPREPARED
PGresult *swoole_pgsql_close_prepared(PGconn *conn, const char *stmt_name);
#endif
void swoole_pgsql_finish(PGconn *conn);
bool php_swoole_pgsql_handle_io_error(struct _pdo_dbh_t *dbh, const char **message);
zend_result php_swoole_pgsql_check_liveness(struct _pdo_dbh_t *dbh);
END_EXTERN_C()
#ifdef SW_USE_PGSQL_HOOK
#define PQconnectdb swoole_pgsql_connectdb
#define PQfinish swoole_pgsql_finish
#define PQprepare swoole_pgsql_prepare
#define PQexecPrepared swoole_pgsql_exec_prepared
#define PQexec swoole_pgsql_exec
#define PQexecParams swoole_pgsql_exec_params
#ifdef HAVE_PQCLOSEPREPARED
#define PQclosePrepared swoole_pgsql_close_prepared
#endif
#endif
#endif
#endif
