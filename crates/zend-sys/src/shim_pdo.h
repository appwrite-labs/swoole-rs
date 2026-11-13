/* PDO's side of Swoole's bundled drivers (crates/pgsql, crates/odbc, crates/firebird; see shim.h). */
#ifndef SWOOLE_RS_ZEND_SHIM_PDO_H
#define SWOOLE_RS_ZEND_SHIM_PDO_H
#include "php.h"

/* Whether PDO's MINIT has registered the PDO class. */
bool swr_zend_pdo_loaded(void);

/* Whether the PDO class declares constant `name`; false before PDO registers it. */
bool swr_zend_pdo_declared(const char *name);

/* REGISTER_PDO_CLASS_CONST_LONG; from PHP 8.5, REGISTER_PDO_CLASS_CONST_LONG_DEPRECATED_85.
 * False, declaring nothing, before PDO registers its class and where Zend would raise
 * a fatal error: `name` is declared already, or is "class". */
bool swr_zend_pdo_declare_long(const char *name, zend_long value);

/* A pdo_dbh_t's driver_data. */
void *swr_zend_pdo_driver_data(void *dbh);

/* PDO's persistent pool forgets `dbh` (a pdo_dbh_t) if it holds it; live
 * PDO and statement objects keep theirs. */
void swr_zend_pdo_forget_persistent(void *dbh);

#endif
