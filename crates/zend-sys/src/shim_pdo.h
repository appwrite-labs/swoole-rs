/* PDO's side of Swoole's bundled drivers (crates/odbc, crates/firebird; see shim.h). */
#ifndef SWOOLE_RS_ZEND_SHIM_PDO_H
#define SWOOLE_RS_ZEND_SHIM_PDO_H
#include "php.h"

/* Whether the PDO class declares constant `name`. */
bool swr_zend_pdo_declared(const char *name);

/* REGISTER_PDO_CLASS_CONST_LONG; from PHP 8.5, REGISTER_PDO_CLASS_CONST_LONG_DEPRECATED_85. */
void swr_zend_pdo_declare_long(const char *name, zend_long value);

#endif
