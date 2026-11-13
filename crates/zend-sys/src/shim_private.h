#ifndef SWOOLE_RS_ZEND_SHIM_PRIVATE_H
#define SWOOLE_RS_ZEND_SHIM_PRIVATE_H
#include "php.h"
#include "zend_closures.h"
#include "zend_exceptions.h"
/* Individual Zend property effects, each containing possible PHP magic-method bailouts. */
int swr_zend_try_facade_read(zend_class_entry *, zend_object *, const char *, size_t, bool, zval *, zval **);
int swr_zend_try_facade_update(zend_class_entry *, zend_object *, const char *, size_t, zval *);
zend_object *swr_zend_facade_exception(void);
void swr_zend_facade_exception_error(void *);
int swr_zend_try_facade_user_call(zval *, uint32_t, zval *, zval *, int *);
/* Work after a coroutine call, inside its zend_try (zend::host::Then). */
typedef void (*swr_zend_then)(void *);
int swr_zend_try_facade_coroutine(long (*)(zend_fcall_info_cache *, uint32_t, zval *, zval *, uint8_t *),
                                  zend_fcall_info_cache *, uint32_t, zval *, zval *, long *,
                                  swr_zend_then, void *, bool *);
int swr_zend_try_facade_intern(zend_string *, zend_string **);
HashTable *swr_zend_facade_function_table(void);
int swr_zend_try_facade_creation_error(zend_class_entry *);
#include "zend_smart_str.h"
int swr_zend_try_facade_callable_frame(zval *, zend_object *, zend_execute_data *, uint32_t, zend_fcall_info_cache *, char **, bool *);
int swr_zend_try_facade_register_functions(const zend_function_entry *, HashTable *, int, int *);
int swr_zend_try_facade_class_alias(const char *, size_t, zend_class_entry *, int *);
HashTable *swr_zend_facade_compiler_functions(void);
#endif
