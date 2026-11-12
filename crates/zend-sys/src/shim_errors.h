/* Errors raised from Rust, each inside zend_try. A user error handler
 * (set_error_handler) runs PHP code, and that code can bail out (a fatal
 * error, exit() with no PHP frame below): the bailout is caught here and
 * reported as 1, so it never longjmps over a Rust frame. */
#ifndef SWOOLE_RS_ZEND_SHIM_ERRORS_H
#define SWOOLE_RS_ZEND_SHIM_ERRORS_H
#include "shim.h"

/* php_error_docref(NULL, level, "%s", message), and zend_error(level, "%s", message). */
int swr_zend_try_error(int level, const char *message);
int swr_zend_try_zend_error(int level, const char *message);

/* A C or C++ reporter (Swoole's php_swoole_*error macros live in C++). */
int swr_zend_try_report(void (*report)(const char *message), const char *message);
int swr_zend_try_report_code(void (*report)(int code, int flag, const char *message), int code,
                             int flag, const char *message);
int swr_zend_try_report_flag(void (*report)(int level, bool flag, const char *message), int level,
                             bool flag, const char *message);

/* Swoole's sw_callable_warning, php_swoole_fatal_error(E_WARNING, "%s", message): a
 * reporter for the calls above and swoole_rs_zend::host::function_call, which run it in a trap. */
void swr_zend_callable_warning(const char *message);

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
 * __destruct, and a possible root can start the cycle collector. */
int swr_zend_try_ptr_dtor(zval *value);
int swr_zend_try_hash_update(HashTable *table, zend_string *key, zend_ulong index, zval *value);
int swr_zend_try_hash_clean(HashTable *table);
int swr_zend_try_symtable_update(zval *array, const char *key, size_t length, zval *value);
/* zend_read_property(scope, object, name, length, 1, rv): a subclass's __get
 * runs PHP. *out is the property, or rv; NULL after a bailout. */
int swr_zend_try_read_property(zend_class_entry *scope, zend_object *object, const char *name,
                               size_t length, zval *rv, zval **out);
#endif
