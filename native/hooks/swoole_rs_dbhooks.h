#ifndef SWOOLE_RS_DBHOOKS_H
#define SWOOLE_RS_DBHOOKS_H
#include <stdbool.h>
#ifdef __cplusplus
extern "C" {
#endif
/* The database hooks (Rust, crates/pgsql and crates/odbc): the coroutine
 * libpq and ODBC calls of Swoole's bundled PDO drivers. Rust exports them
 * under upstream's names (php_swoole_pgsql.h, php_swoole_odbc.h); C++ lends
 * the coroutine waits at MINIT. */

#ifdef SW_USE_PGSQL
struct pg_conn;
struct pg_result;
struct swr_v1_pgsql_ops {
    /* Park the coroutine until `event` (SW_EVENT_READ/WRITE) on `fd` or
     * `timeout`: 1 ready, 0 timed out, -1 failed. */
    int (*wait)(int fd, int event, double timeout);
    bool (*in_coroutine)(void);
};
void swr_v1_pgsql_init(const struct swr_v1_pgsql_ops *ops);
/* Wait for a sent request's result and keep the last one. */
struct pg_result *swr_v1_pgsql_get_result(struct pg_conn *conn);
#endif

#ifdef SW_USE_ODBC
struct swr_v1_odbc_ops {
    /* swoole::coroutine::async: run job(context) on the thread pool while the
     * running coroutine waits. */
    void (*run_async)(void (*job)(void *), void *context);
    bool (*in_coroutine)(void);
};
void swr_v1_odbc_init(const struct swr_v1_odbc_ops *ops);
#endif
#ifdef __cplusplus
}
#endif
#endif
