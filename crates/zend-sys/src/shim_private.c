#include "shim_private.h"
int swr_zend_try_facade_read(zend_class_entry *scope, zend_object *object, const char *name, size_t length,
                             bool silent, zval *rv, zval **result) {
    int bailed = 0;
    zend_try { *result = zend_read_property(scope, object, name, length, silent, rv); }
    zend_catch { bailed = 1; } zend_end_try();
    return bailed;
}
int swr_zend_try_facade_update(zend_class_entry *scope, zend_object *object, const char *name, size_t length, zval *value) {
    int bailed = 0;
    zend_try { zend_update_property(scope, object, name, length, value); }
    zend_catch { bailed = 1; } zend_end_try();
    return bailed;
}
int swr_zend_try_facade_null(zend_class_entry *scope, zend_object *object, const char *name, size_t length) {
    int bailed = 0;
    zend_try { zend_update_property_null(scope, object, name, length); }
    zend_catch { bailed = 1; } zend_end_try();
    return bailed;
}
zend_object *swr_zend_facade_closure(zend_function *function) { return ZEND_CLOSURE_OBJECT(function); }
void swr_zend_facade_object_release(void *object) { OBJ_RELEASE((zend_object *) object); }
zend_object *swr_zend_facade_exception(void) { return EG(exception); }
void swr_zend_facade_exception_error(void *exception) { zend_exception_error(exception, E_ERROR); }
int swr_zend_try_facade_user_call(zval *name, uint32_t argc, zval *argv, zval *retval, int *result) {
    int bailed = 0;
    zend_try { *result = call_user_function(EG(function_table), NULL, name, retval, argc, argv); }
    zend_catch { bailed = 1; } zend_end_try();
    return bailed;
}
int swr_zend_try_facade_coroutine(int64_t (*create)(zend_fcall_info_cache *, uint32_t, zval *),
                                  zend_fcall_info_cache *cache, uint32_t argc, zval *argv, int64_t *result) {
    int bailed = 0;
    zend_try { *result = create(cache, argc, argv); }
    zend_catch { bailed = 1; } zend_end_try();
    return bailed;
}
int swr_zend_try_facade_append(zval *array, zval *value) {
    int bailed = 0;
    zend_try { add_next_index_zval(array, value); }
    zend_catch { bailed = 1; } zend_end_try();
    return bailed;
}
int swr_zend_try_facade_array_two(zval *value) {
    int bailed = 0;
    zend_try { array_init_size(value, 2); }
    zend_catch { bailed = 1; } zend_end_try();
    return bailed;
}
int swr_zend_try_facade_add_index(HashTable *table, zend_ulong index, zval *value, zval **result) {
    int bailed = 0;
    zend_try { *result = zend_hash_index_add(table, index, value); }
    zend_catch { bailed = 1; } zend_end_try();
    return bailed;
}
int swr_zend_try_facade_add_bytes(HashTable *table, const char *key, size_t length, zval *value, zval **result) {
    int bailed = 0;
    zend_try { *result = zend_hash_str_add(table, key, length, value); }
    zend_catch { bailed = 1; } zend_end_try();
    return bailed;
}
int swr_zend_try_facade_update_index(HashTable *table, zend_ulong index, zval *value, zval **result) {
    int bailed = 0;
    zend_try { *result = zend_hash_index_update(table, index, value); }
    zend_catch { bailed = 1; } zend_end_try();
    return bailed;
}
int swr_zend_try_facade_update_bytes(HashTable *table, const char *key, size_t length, zval *value, zval **result) {
    int bailed = 0;
    zend_try { *result = zend_hash_str_update(table, key, length, value); }
    zend_catch { bailed = 1; } zend_end_try();
    return bailed;
}
bool swr_zend_facade_numeric_key(const char *key, size_t length, zend_ulong *index) {
    return ZEND_HANDLE_NUMERIC_STR(key, length, *index);
}
int swr_zend_try_facade_palloc(size_t size, void **result) {
    int bailed = 0;
    zend_try { *result = pemalloc(size, 1); }
    zend_catch { bailed = 1; } zend_end_try();
    return bailed;
}
void swr_zend_facade_pfree(void *allocation) { pefree(allocation, 1); }
int swr_zend_try_facade_pstring(const char *text, size_t length, zend_string **result) {
    int bailed = 0;
    zend_try { *result = zend_string_init(text, length, 1); }
    zend_catch { bailed = 1; } zend_end_try();
    return bailed;
}
int swr_zend_try_facade_intern(zend_string *string, zend_string **result) {
    int bailed = 0;
    zend_try { *result = zend_new_interned_string(string); }
    zend_catch { bailed = 1; } zend_end_try();
    return bailed;
}
HashTable *swr_zend_facade_function_table(void) { return EG(function_table); }

void swr_zval_facade_addref(zval *value) { Z_ADDREF_P(value); }
int swr_zend_try_facade_alloc(size_t size, void **result) {
    int bailed = 0;
    zend_try { *result = emalloc(size); }
    zend_catch { bailed = 1; } zend_end_try();
    return bailed;
}
int swr_zend_try_facade_string(const char *text, size_t length, zend_string **result) {
    int bailed = 0;
    zend_try { *result = zend_string_init(text, length, 0); }
    zend_catch { bailed = 1; } zend_end_try();
    return bailed;
}
int swr_zend_try_facade_value_string(zval *value, const char *text, size_t length) {
    int bailed = 0;
    zend_try { ZVAL_STRINGL(value, text, length); }
    zend_catch { bailed = 1; } zend_end_try();
    return bailed;
}
int swr_zend_try_facade_realloc(zend_string *string, size_t length, zend_string **result) {
    int bailed = 0;
    zend_try { *result = zend_string_realloc(string, length, 0); }
    zend_catch { bailed = 1; } zend_end_try();
    return bailed;
}
int swr_zend_try_facade_object_alloc(size_t size, zend_class_entry *ce, void **result) {
    int bailed = 0;
    zend_try { *result = zend_object_alloc(size, ce); }
    zend_catch { bailed = 1; } zend_end_try();
    return bailed;
}
int swr_zend_try_facade_array_init(zval *value) {
    int bailed = 0;
    zend_try { array_init(value); }
    zend_catch { bailed = 1; } zend_end_try();
    return bailed;
}
int swr_zend_try_facade_objects_new(zend_class_entry *ce, zend_object **result) {
    int bailed = 0;
    zend_try { *result = zend_objects_new(ce); }
    zend_catch { bailed = 1; } zend_end_try();
    return bailed;
}
int swr_zend_try_facade_creation_error(zend_class_entry *ce) {
    int bailed = 0;
    zend_try { zend_throw_error(NULL, "The object of %s can not be created for security reasons", ZSTR_VAL(ce->name)); }
    zend_catch { bailed = 1; } zend_end_try();
    return bailed;
}
