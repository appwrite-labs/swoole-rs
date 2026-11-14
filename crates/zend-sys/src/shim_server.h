/* Shims crates/phpserver uses (see shim.h). */
#ifndef SWOOLE_RS_ZEND_SHIM_SERVER_H
#define SWOOLE_RS_ZEND_SHIM_SERVER_H
#include "shim.h"

/* php_var_serialize(value) inside zend_try (__serialize and __sleep run PHP):
 * the new string, or NULL when nothing was written; *bailed when it bailed out. */
zend_string *swr_zend_serialize(zval *value, bool *bailed);

/* php_var_unserialize(out, data, length) inside zend_try: whether it succeeded,
 * with *offset where it stopped; *bailed when it bailed out. */
bool swr_zend_unserialize(zval *out, const char *data, size_t length, size_t *offset, bool *bailed);

/* SEPARATE_ARRAY(dest); php_array_merge(Z_ARRVAL_P(dest), Z_ARRVAL_P(src)) inside
 * zend_try (a replaced value's destructor runs PHP): 1 if it bailed out. */
int swr_zend_try_array_merge(zval *dest, zval *src);

/* HOOK_PHP_CALL_STACK (php_swoole_call_stack.h): clear EG(stack_limit) and
 * EG(stack_base) into saved[0..1] for a call made on a reactor thread's stack,
 * and put them back. Nothing without ZEND_CHECK_STACK_LIMIT. */
void swr_zend_stack_unlimit(void **saved);
void swr_zend_stack_restore(void *const *saved);

/* php_stream_mkdir(path, 0777, PHP_STREAM_MKDIR_RECURSIVE | REPORT_ERRORS, NULL). */
bool swr_zend_mkdir_recursive(const char *path);

/* php_swoole_is_fatal_error(): the last error's message, file ("-" without one)
 * and line when it was fatal (E_FATAL_ERRORS), else false. */
bool swr_zend_last_fatal_error(const char **message, const char **file, int *line);
#endif
