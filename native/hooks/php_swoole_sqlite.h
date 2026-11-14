/* The coroutine sqlite3 calls crates/sqlite exports (--enable-swoole-sqlite),
 * and the macros that point the bundled pdo_sqlite driver at them. The driver
 * includes this first, after defining SW_USE_SQLITE_HOOK. */
#ifndef PHP_SWOOLE_SQLITE_H
#define PHP_SWOOLE_SQLITE_H
#include "php_swoole.h"
#ifdef SW_USE_SQLITE
#include <sqlite3.h>
BEGIN_EXTERN_C()
int swoole_sqlite3_open_v2(const char *filename, sqlite3 **db, int flags, const char *vfs);
int swoole_sqlite3_prepare_v2(sqlite3 *db, const char *sql, int bytes, sqlite3_stmt **stmt, const char **tail);
int swoole_sqlite3_exec(sqlite3 *db, const char *sql, int (*callback)(void *, int, char **, char **), void *argument,
                        char **errmsg);
int swoole_sqlite3_close(sqlite3 *db);
int swoole_sqlite3_close_v2(sqlite3 *db);
int swoole_sqlite3_step(sqlite3_stmt *stmt);
END_EXTERN_C()
#if PHP_VERSION_ID < 80300
/* PHP 8.3's zend_zval_value_name, which the driver's statement calls. */
static inline const char *zend_zval_value_name(const zval *arg) {
    ZVAL_DEREF(arg);
    if (Z_ISUNDEF_P(arg)) {
        return "null";
    }
    if (Z_TYPE_P(arg) == IS_OBJECT) {
        return ZSTR_VAL(Z_OBJCE_P(arg)->name);
    }
    if (Z_TYPE_P(arg) == IS_FALSE) {
        return "false";
    }
    if (Z_TYPE_P(arg) == IS_TRUE) {
        return "true";
    }
    return zend_get_type_by_const(Z_TYPE_P(arg));
}
#endif
#ifdef SW_USE_SQLITE_HOOK
#define sqlite3_open_v2 swoole_sqlite3_open_v2
#define sqlite3_prepare_v2 swoole_sqlite3_prepare_v2
#define sqlite3_exec swoole_sqlite3_exec
#define sqlite3_close swoole_sqlite3_close
#define sqlite3_close_v2 swoole_sqlite3_close_v2
#define sqlite3_step swoole_sqlite3_step
#endif
#endif
#endif
