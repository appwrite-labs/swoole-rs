/* The module lifecycle crates/phpswoole drives (php_swoole.cc's MINIT, RINIT,
 * RSHUTDOWN and MSHUTDOWN): the PHP-version and build-dependent pieces, and
 * the C/C++ hooks it runs inside zend_try. */
#ifndef SWOOLE_RS_ZEND_SHIM_MODULE_H
#define SWOOLE_RS_ZEND_SHIM_MODULE_H
#include "shim.h"

/* hook() or hook(module_number) inside zend_try: 1 if it bailed out. */
int swr_zend_try_hook(void (*hook)(void));
int swr_zend_try_minit(void (*hook)(int module_number), int module_number);

/* sapi_module.name. */
const char *swr_zend_sapi_name(void);
/* php_get_temporary_directory(). */
const char *swr_zend_temporary_directory(void);
/* Whether CG(compiler_options) has ZEND_COMPILE_PRELOAD (RINIT of the preloader). */
int swr_zend_compiling_preload(void);
/* zend_execute_ex = execute_ex, returning the hook it replaced (NULL if none),
 * and the restore. */
void *swr_zend_execute_ex_reset(void);
void swr_zend_execute_ex_restore(void *hook);
/* SIGG(check) = false, where ZEND_SIGNALS is built. */
void swr_zend_signals_uncheck(void);
/* Under LeakSanitizer (tools/asan.py), its check, at MSHUTDOWN rather than at exit:
 * a worker or child exits without shutting PHP down. Nothing in other builds, or in
 * a process forked from the one that loaded the extension, where the check hangs (#2345). */
void swr_zend_leak_check(void);
/* Register the global function `name` as a user shutdown function. */
void swr_zend_register_shutdown_function(const char *name);
/* Set PHP_STREAM_FLAG_NO_CLOSE on the stream of the constant `name` (STDOUT,
 * STDERR), where PHP has the flag. */
void swr_zend_stream_constant_no_close(const char *name, size_t length);

int swr_zend_try_module_ini(const zend_ini_entry_def *, int, int);
int swr_zend_try_info_start(void);
int swr_zend_try_info_header(const char *, const char *);
int swr_zend_try_info_row(const char *, const char *);
int swr_zend_try_info_end(void);
int swr_zend_try_info_ini(void *);
#endif
