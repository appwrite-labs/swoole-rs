/* Swoole\Runtime's function and class table edits (crates/phpruntime): the
 * executor-global tables, version-dependent arginfo types and the class-entry
 * parent union. */
#ifndef SWOOLE_RS_ZEND_SHIM_RUNTIME_H
#define SWOOLE_RS_ZEND_SHIM_RUNTIME_H
#include "php.h"

/* PHP_VERSION_ID. */
int swr_zend_runtime_version(void);
/* Swoole's copy_arginfo: a persistent copy of a function's arginfo (and the
 * return slot before it) with its class-name types duplicated. */
zend_internal_arg_info *swr_zend_runtime_copy_arginfo(const zend_internal_function *function,
                                                      zend_internal_arg_info *arg_info);
/* Swoole's free_arg_info of such a copy. */
void swr_zend_runtime_free_arginfo(const zend_internal_function *function, zend_internal_arg_info *copy);
/* The lowercased name in EG(class_table), without autoloading. */
zend_class_entry *swr_zend_runtime_find_class(const char *name, size_t length);
/* ce->parent, a member of an anonymous union. */
zend_class_entry *swr_zend_runtime_parent(const zend_class_entry *ce);
void swr_zend_runtime_set_parent(zend_class_entry *ce, zend_class_entry *parent);
/* instanceof_function (inline). */
bool swr_zend_runtime_instanceof(const zend_class_entry *child, const zend_class_entry *parent);
/* handler(execute_data, return_value) inside zend_try: 1 if it bailed out. */
int swr_zend_runtime_call_handler(zif_handler handler, zend_execute_data *execute_data, zval *return_value);
/* The handlers of functions Swoole disables in coroutines: "%s() has been
 * disabled for security reasons" and "%s() is not supported in Swoole
 * coroutines" warnings. */
void ZEND_FASTCALL swr_zend_runtime_disabled(INTERNAL_FUNCTION_PARAMETERS);
void ZEND_FASTCALL swr_zend_runtime_unsupported(INTERNAL_FUNCTION_PARAMETERS);
#endif
