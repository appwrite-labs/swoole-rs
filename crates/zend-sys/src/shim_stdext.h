/* Shims crates/phpstdext uses (Swoole's experimental stdext; see shim.h). */
#ifndef SWOOLE_RS_ZEND_SHIM_STDEXT_H
#define SWOOLE_RS_ZEND_SHIM_STDEXT_H
#include "php.h"

/* zend_hash_str_find_ptr(EG(function_table), name) and, of the NUL-terminated
 * `name`, of CG(function_table): null for no such function. */
zend_function *swr_zend_function_find(const char *name, size_t length);
zend_function *swr_zend_compiled_function_find(const char *name);

/* zend_call_known_function(fn, NULL, NULL, retval, argc, argv, NULL) inside
 * zend_try: 1 if it bailed out. */
int swr_zend_try_call_known(zend_function *fn, zval *retval, uint32_t argc, zval *argv);

/* zend_lookup_class(name) inside zend_try (autoloaders run PHP): 1 if it
 * bailed out, else *ce is the class or NULL. */
int swr_zend_try_lookup_class(zend_string *name, zend_class_entry **ce);

/* zend_vm_stack_push_call_frame(ZEND_CALL_NESTED_FUNCTION, fn, num_args, object). */
zend_execute_data *swr_zend_push_nested_call(zend_function *fn, uint32_t num_args, void *object);

/* sapi_module.treat_data(PARSE_STRING, text, array): `text` is emalloc'd and
 * the SAPI frees it. */
void swr_zend_treat_string(char *text, zval *array);

int swr_zend_try_stdext_nogc_dtor(zval *);
int swr_zend_try_stdext_cannot_add(void);
#endif
