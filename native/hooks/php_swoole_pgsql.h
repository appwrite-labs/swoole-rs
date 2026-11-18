#ifndef PHP_SWOOLE_PGSQL_H
#define PHP_SWOOLE_PGSQL_H
#include "php_swoole.h"
#ifdef SW_USE_PGSQL
#include <libpq-fe.h>
BEGIN_EXTERN_C()
void *pdo_pgsql_allocate_parameters(size_t count);
void pdo_pgsql_free_parameters(void *owner);

END_EXTERN_C()
#endif
#endif
