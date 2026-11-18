/* Errors raised from Rust, each inside zend_try. A user error handler
 * (set_error_handler) runs PHP code, and that code can bail out (a fatal
 * error, exit() with no PHP frame below): the bailout is caught here and
 * reported as 1, so it never longjmps over a Rust frame. */
#ifndef SWOOLE_RS_ZEND_SHIM_ERRORS_H
#define SWOOLE_RS_ZEND_SHIM_ERRORS_H
#include "shim.h"
#include <sys/uio.h>

/* php_error_docref(NULL, level, "%s", message), and zend_error(level, "%s", message). */
int swr_zend_try_error(int level, const char *message);
int swr_zend_try_zend_error(int level, const char *message);
/* php_error_docref1(NULL, param, level, "%s", message): the error names `param`. */
int swr_zend_try_error1(const char *param, int level, const char *message);
/* php_error_docref2(NULL, param1, param2, level, "%s", message). */
int swr_zend_try_error2(const char *param1, const char *param2, int level, const char *message);
/* php_copy_file(source, target) into *copied: it opens both through their stream wrappers,
 * which may warn. */
int swr_zend_try_copy_file(const char *source, const char *target, bool *copied);
/* php_check_open_basedir_ex(path, warn) into *refused; with `warn`, a refusal is an E_WARNING. */
int swr_zend_try_open_basedir(const char *path, int warn, bool *refused);

/* Swoole's sw_callable_warning, php_swoole_fatal_error(E_WARNING, "%s", message), for
 * swoole_rs_zend::host::function_call, which runs it in the trap below. */
void swr_zend_callable_warning(const char *message);
/* swr_zend_callable_warning inside zend_try (nonzero for a bailout). */
int swr_zend_try_callable_warning(const char *message);

/* zend_parse_arg_*_slow: coercions raise deprecations (null to a scalar,
 * float to int), which run the user error handler. *parsed is the result. */
int swr_zend_try_parse_long(zval *arg, zend_long *out, uint32_t number, bool *parsed);
int swr_zend_try_parse_bool(zval *arg, bool *out, uint32_t number, bool *parsed);
int swr_zend_try_parse_double(zval *arg, double *out, uint32_t number, bool *parsed);
int swr_zend_try_parse_str(zval *arg, zend_string **out, uint32_t number, bool *parsed);

/* zval_get_long/double/string: an object or array warns (or runs
 * __toString), which runs the user error handler. A bailout leaves 0, 0.0 or
 * the empty string. */
int swr_zend_try_get_long(zval *value, zend_long *out);
int swr_zend_try_get_double(zval *value, double *out);
int swr_zend_try_get_string(zval *value, zend_string **out);
/* zval_ptr_dtor, and zend_hash_update, zend_hash_clean and add_assoc_zval_ex
 * (which release the old values): the last reference to an object runs
 * __destruct, and a possible root can start the cycle collector. The clean
 * separates the array a zval holds first, and leaves any other value alone. */
int swr_zend_try_ptr_dtor(zval *value);
int swr_zend_try_hash_update(HashTable *table, zend_string *key, zend_ulong index, zval *value);
int swr_zend_try_hash_index_del(HashTable *table, zend_ulong index);
int swr_zend_try_hash_clean(zval *array);
int swr_lent_symtable_update(zval *array, struct iovec key, zval *value);
#endif
