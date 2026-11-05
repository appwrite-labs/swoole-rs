/* The module host's C pieces (crates/phpswoole): what depends on the PHP version
 * or build mode, and the C-variadic SwooleG.fatal_error. */
#ifndef SWOOLE_RS_ZEND_SHIM_HOST_H
#define SWOOLE_RS_ZEND_SHIM_HOST_H
#include "shim.h"

/* STANDARD_MODULE_HEADER_EX and STANDARD_MODULE_PROPERTIES, every other field zero. */
void swr_zend_module_template(zend_module_entry *entry);
/* The after-fork hook: refresh_memory_manager() (PHP 8.5+) and
 * zend_max_execution_timer_init() where PHP has the timers. */
void swr_zend_after_fork(void *unused);
/* SwooleG.print_backtrace: "Stack trace:" to stderr, then the PHP backtrace. */
void swr_zend_print_stack_trace(void);
/* SwooleG.fatal_error: throw Swoole\Error(message, code) and print it as an
 * E_ERROR, then swr_v1_phpswoole_fatal_exit(code). */
void swr_zend_fatal_error(int code, const char *format, ...);
#endif
