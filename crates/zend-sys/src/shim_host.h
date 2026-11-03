/* Shims crates/phpswoole uses (the module host; see shim.h). */
#ifndef SWOOLE_RS_ZEND_SHIM_HOST_H
#define SWOOLE_RS_ZEND_SHIM_HOST_H
#include "shim.h"

/* Swoole's SW_FUNCTION_ALIAS(CG(function_table), origin, CG(function_table), alias,
 * arg_info): the global function `alias` runs `origin`'s handler. */
int swr_zend_register_global_alias(const char *origin, size_t origin_length, const char *alias,
                                   size_t alias_length, const zend_internal_arg_info *arg_info);

#endif
