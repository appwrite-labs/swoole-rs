/* The coroutine libpq crates/pgsql exports (--enable-swoole-pgsql), and the
 * macros that point the bundled pdo_pgsql driver at it. The driver includes
 * this first, after defining SW_USE_PGSQL_HOOK. */
#ifndef PHP_SWOOLE_PGSQL_H
#define PHP_SWOOLE_PGSQL_H
#include "php_swoole.h"
#ifdef SW_USE_PGSQL
#include <libpq-fe.h>
BEGIN_EXTERN_C()
typedef struct {
    size_t count;
    void *owner;
} pdo_pgsql_parameter_arrays;

void *pdo_pgsql_allocate_parameters(size_t count);
void pdo_pgsql_free_parameters(void *owner);

PGresult *swoole_pgsql_exec(PGconn *conn, const char *query);
END_EXTERN_C()
#ifdef SW_USE_PGSQL_HOOK
#define PQexec swoole_pgsql_exec
#endif
#endif
#endif
