#ifndef SWOOLE_RS_ZEND_SHIM_PRIVATE_H
#define SWOOLE_RS_ZEND_SHIM_PRIVATE_H
#include "php.h"
#include "zend_closures.h"
#include "zend_exceptions.h"
/* Individual Zend property effects, each containing possible PHP magic-method bailouts. */
int swr_zend_try_facade_read(zend_class_entry *, zend_object *, const char *, size_t, bool, zval *, zval **);
int swr_zend_try_facade_update(zend_class_entry *, zend_object *, const char *, size_t, zval *);
int swr_zend_try_facade_null(zend_class_entry *, zend_object *, const char *, size_t);
zend_object *swr_zend_facade_closure(zend_function *);
void swr_zend_facade_object_release(void *);
zend_object *swr_zend_facade_exception(void);
void swr_zend_facade_exception_error(void *);
int swr_zend_try_facade_user_call(zval *, uint32_t, zval *, zval *, int *);
int swr_zend_try_facade_coroutine(int64_t (*)(zend_fcall_info_cache *, uint32_t, zval *),
                                  zend_fcall_info_cache *, uint32_t, zval *, int64_t *);
int swr_zend_try_facade_append(zval *, zval *);
int swr_zend_try_facade_array_two(zval *);
int swr_zend_try_facade_add_index(HashTable *, zend_ulong, zval *, zval **);
int swr_zend_try_facade_add_bytes(HashTable *, const char *, size_t, zval *, zval **);
int swr_zend_try_facade_update_index(HashTable *, zend_ulong, zval *, zval **);
int swr_zend_try_facade_update_bytes(HashTable *, const char *, size_t, zval *, zval **);
bool swr_zend_facade_numeric_key(const char *, size_t, zend_ulong *);
int swr_zend_try_facade_palloc(size_t, void **);
void swr_zend_facade_pfree(void *);
int swr_zend_try_facade_pstring(const char *, size_t, zend_string **);
int swr_zend_try_facade_intern(zend_string *, zend_string **);
HashTable *swr_zend_facade_function_table(void);
void swr_zval_facade_addref(zval *);
int swr_zend_try_facade_alloc(size_t, void **);
int swr_zend_try_facade_string(const char *, size_t, zend_string **);
int swr_zend_try_facade_value_string(zval *, const char *, size_t);
int swr_zend_try_facade_realloc(zend_string *, size_t, zend_string **);
int swr_zend_try_facade_object_alloc(size_t, zend_class_entry *, void **);
int swr_zend_try_facade_array_init(zval *);
int swr_zend_try_facade_objects_new(zend_class_entry *, zend_object **);
int swr_zend_try_facade_creation_error(zend_class_entry *);
int swr_zend_try_facade_object_init(zend_object *, zend_class_entry *);
int swr_zend_try_facade_properties_init(zend_object *, zend_class_entry *);
int swr_zend_try_facade_object_dtor(zend_object *);
#endif
