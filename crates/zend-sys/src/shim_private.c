#include "shim.h"
#include "shim_private.h"
int swr_zend_try_facade_read(zend_class_entry *scope, zend_object *object, const char *name, size_t length,
                             bool silent, zval *rv, zval **result) {
    int bailed = 0;
    SWR_ZEND_TRY { *result = zend_read_property(scope, object, name, length, silent, rv); }
    SWR_ZEND_CATCH { bailed = 1; } SWR_ZEND_END_TRY();
    return bailed;
}
int swr_zend_try_facade_update(zend_class_entry *scope, zend_object *object, const char *name, size_t length, zval *value) {
    int bailed = 0;
    SWR_ZEND_TRY { zend_update_property(scope, object, name, length, value); }
    SWR_ZEND_CATCH { bailed = 1; } SWR_ZEND_END_TRY();
    return bailed;
}
int swr_zend_try_facade_null(zend_class_entry *scope, zend_object *object, const char *name, size_t length) {
    int bailed = 0;
    SWR_ZEND_TRY { zend_update_property_null(scope, object, name, length); }
    SWR_ZEND_CATCH { bailed = 1; } SWR_ZEND_END_TRY();
    return bailed;
}
zend_object *swr_zend_facade_closure(zend_function *function) { return ZEND_CLOSURE_OBJECT(function); }
void swr_zend_facade_object_release(void *object) { OBJ_RELEASE((zend_object *) object); }
zend_object *swr_zend_facade_exception(void) { return EG(exception); }
void swr_zend_facade_exception_error(void *exception) { zend_exception_error(exception, E_ERROR); }
int swr_zend_try_facade_user_call(zval *name, uint32_t argc, zval *argv, zval *retval, int *result) {
    int bailed = 0;
    SWR_ZEND_TRY { *result = call_user_function(EG(function_table), NULL, name, retval, argc, argv); }
    SWR_ZEND_CATCH { bailed = 1; } SWR_ZEND_END_TRY();
    return bailed;
}
int swr_zend_try_facade_coroutine(int64_t (*create)(zend_fcall_info_cache *, uint32_t, zval *),
                                  zend_fcall_info_cache *cache, uint32_t argc, zval *argv, int64_t *result) {
    int bailed = 0;
    SWR_ZEND_TRY { *result = create(cache, argc, argv); }
    SWR_ZEND_CATCH { bailed = 1; } SWR_ZEND_END_TRY();
    return bailed;
}
int swr_zend_try_facade_append(zval *array, zval *value) {
    int bailed = 0;
    SWR_ZEND_TRY { add_next_index_zval(array, value); }
    SWR_ZEND_CATCH { bailed = 1; } SWR_ZEND_END_TRY();
    return bailed;
}
int swr_zend_try_facade_array_two(zval *value) {
    int bailed = 0;
    SWR_ZEND_TRY { array_init_size(value, 2); }
    SWR_ZEND_CATCH { bailed = 1; } SWR_ZEND_END_TRY();
    return bailed;
}
int swr_zend_try_facade_add_index(HashTable *table, zend_ulong index, zval *value, zval **result) {
    int bailed = 0;
    SWR_ZEND_TRY { *result = zend_hash_index_add(table, index, value); }
    SWR_ZEND_CATCH { bailed = 1; } SWR_ZEND_END_TRY();
    return bailed;
}
int swr_zend_try_facade_add_bytes(HashTable *table, const char *key, size_t length, zval *value, zval **result) {
    int bailed = 0;
    SWR_ZEND_TRY { *result = zend_hash_str_add(table, key, length, value); }
    SWR_ZEND_CATCH { bailed = 1; } SWR_ZEND_END_TRY();
    return bailed;
}
int swr_zend_try_facade_update_index(HashTable *table, zend_ulong index, zval *value, zval **result) {
    int bailed = 0;
    SWR_ZEND_TRY { *result = zend_hash_index_update(table, index, value); }
    SWR_ZEND_CATCH { bailed = 1; } SWR_ZEND_END_TRY();
    return bailed;
}
int swr_zend_try_facade_update_bytes(HashTable *table, const char *key, size_t length, zval *value, zval **result) {
    int bailed = 0;
    SWR_ZEND_TRY { *result = zend_hash_str_update(table, key, length, value); }
    SWR_ZEND_CATCH { bailed = 1; } SWR_ZEND_END_TRY();
    return bailed;
}
bool swr_zend_facade_numeric_key(const char *key, size_t length, zend_ulong *index) {
    return ZEND_HANDLE_NUMERIC_STR(key, length, *index);
}
int swr_zend_try_facade_palloc(size_t size, void **result) {
    int bailed = 0;
    SWR_ZEND_TRY { *result = pemalloc(size, 1); }
    SWR_ZEND_CATCH { bailed = 1; } SWR_ZEND_END_TRY();
    return bailed;
}
void swr_zend_facade_pfree(void *allocation) { pefree(allocation, 1); }
int swr_zend_try_facade_pstring(const char *text, size_t length, zend_string **result) {
    int bailed = 0;
    SWR_ZEND_TRY { *result = zend_string_init(text, length, 1); }
    SWR_ZEND_CATCH { bailed = 1; } SWR_ZEND_END_TRY();
    return bailed;
}
int swr_zend_try_facade_intern(zend_string *string, zend_string **result) {
    int bailed = 0;
    SWR_ZEND_TRY { *result = zend_new_interned_string(string); }
    SWR_ZEND_CATCH { bailed = 1; } SWR_ZEND_END_TRY();
    return bailed;
}
HashTable *swr_zend_facade_function_table(void) { return EG(function_table); }

void swr_zval_facade_addref(zval *value) { Z_ADDREF_P(value); }
int swr_zend_try_facade_alloc(size_t size, void **result) {
    int bailed = 0;
    SWR_ZEND_TRY { *result = emalloc(size); }
    SWR_ZEND_CATCH { bailed = 1; } SWR_ZEND_END_TRY();
    return bailed;
}
int swr_zend_try_facade_string(const char *text, size_t length, zend_string **result) {
    int bailed = 0;
    SWR_ZEND_TRY { *result = zend_string_init(text, length, 0); }
    SWR_ZEND_CATCH { bailed = 1; } SWR_ZEND_END_TRY();
    return bailed;
}
int swr_zend_try_facade_value_string(zval *value, const char *text, size_t length) {
    int bailed = 0;
    SWR_ZEND_TRY { ZVAL_STRINGL(value, text, length); }
    SWR_ZEND_CATCH { bailed = 1; } SWR_ZEND_END_TRY();
    return bailed;
}
int swr_zend_try_facade_realloc(zend_string *string, size_t length, zend_string **result) {
    int bailed = 0;
    SWR_ZEND_TRY { *result = zend_string_realloc(string, length, 0); }
    SWR_ZEND_CATCH { bailed = 1; } SWR_ZEND_END_TRY();
    return bailed;
}
int swr_zend_try_facade_object_alloc(size_t size, zend_class_entry *ce, void **result) {
    int bailed = 0;
    SWR_ZEND_TRY { *result = zend_object_alloc(size, ce); }
    SWR_ZEND_CATCH { bailed = 1; } SWR_ZEND_END_TRY();
    return bailed;
}
int swr_zend_try_facade_array_init(zval *value) {
    int bailed = 0;
    SWR_ZEND_TRY { array_init(value); }
    SWR_ZEND_CATCH { bailed = 1; } SWR_ZEND_END_TRY();
    return bailed;
}
int swr_zend_try_facade_objects_new(zend_class_entry *ce, zend_object **result) {
    int bailed = 0;
    SWR_ZEND_TRY { *result = zend_objects_new(ce); }
    SWR_ZEND_CATCH { bailed = 1; } SWR_ZEND_END_TRY();
    return bailed;
}
int swr_zend_try_facade_creation_error(zend_class_entry *ce) {
    int bailed = 0;
    SWR_ZEND_TRY { zend_throw_error(NULL, "The object of %s can not be created for security reasons", ZSTR_VAL(ce->name)); }
    SWR_ZEND_CATCH { bailed = 1; } SWR_ZEND_END_TRY();
    return bailed;
}

int swr_zend_try_facade_object_init(zend_object *object, zend_class_entry *ce) {
    int bailed = 0;
    SWR_ZEND_TRY { zend_object_std_init(object, ce); }
    SWR_ZEND_CATCH { bailed = 1; } SWR_ZEND_END_TRY();
    return bailed;
}
int swr_zend_try_facade_properties_init(zend_object *object, zend_class_entry *ce) {
    int bailed = 0;
    SWR_ZEND_TRY { object_properties_init(object, ce); }
    SWR_ZEND_CATCH { bailed = 1; } SWR_ZEND_END_TRY();
    return bailed;
}
int swr_zend_try_facade_object_dtor(zend_object *object) {
    int bailed = 0;
    SWR_ZEND_TRY { zend_object_std_dtor(object); }
    SWR_ZEND_CATCH { bailed = 1; } SWR_ZEND_END_TRY();
    return bailed;
}

#include "ext/standard/url.h"
#include "ext/standard/php_http.h"
int swr_zend_try_facade_callable_frame(zval *value, zend_object *object, zend_execute_data *frame,
                                      uint32_t flags, zend_fcall_info_cache *cache, char **error, bool *result) {
    int bailed = 0;
    SWR_ZEND_TRY { *result = zend_is_callable_at_frame(value, object, frame, flags, cache, error); }
    SWR_ZEND_CATCH { bailed = 1; } SWR_ZEND_END_TRY();
    return bailed;
}
int swr_zend_try_facade_callable_name(zval *value, zend_object *object, zend_string **result) {
    int bailed = 0;
    SWR_ZEND_TRY { *result = zend_get_callable_name_ex(value, object); }
    SWR_ZEND_CATCH { bailed = 1; } SWR_ZEND_END_TRY();
    return bailed;
}
int swr_zend_try_facade_url(const char *value, size_t length, zend_string **result) {
    int bailed = 0;
    SWR_ZEND_TRY { *result = php_url_encode(value, length); }
    SWR_ZEND_CATCH { bailed = 1; } SWR_ZEND_END_TRY();
    return bailed;
}
int swr_zend_try_facade_hash(zval *value, HashTable **result) {
    int bailed = 0;
    SWR_ZEND_TRY { *result = HASH_OF(value); }
    SWR_ZEND_CATCH { bailed = 1; } SWR_ZEND_END_TRY();
    return bailed;
}
int swr_zend_try_facade_query(HashTable *table, smart_str *buffer) {
    int bailed = 0;
    SWR_ZEND_TRY {
#if PHP_VERSION_ID < 80300
        php_url_encode_hash_ex(table, buffer, NULL, 0, NULL, 0, NULL, 0, NULL, NULL, PHP_QUERY_RFC1738);
#else
        php_url_encode_hash_ex(table, buffer, NULL, 0, NULL, NULL, NULL, PHP_QUERY_RFC1738);
#endif
    } SWR_ZEND_CATCH { bailed = 1; } SWR_ZEND_END_TRY();
    return bailed;
}
int swr_zend_try_facade_smart_zero(smart_str *buffer) {
    int bailed = 0;
    SWR_ZEND_TRY { smart_str_0(buffer); } SWR_ZEND_CATCH { bailed = 1; } SWR_ZEND_END_TRY();
    return bailed;
}
void swr_zend_facade_smart_free(smart_str *buffer) { smart_str_free(buffer); }

int swr_zend_try_facade_register_functions(const zend_function_entry *entries, HashTable *table, int type, int *result) {
    int bailed = 0;
    SWR_ZEND_TRY { *result = zend_register_functions(NULL, entries, table, type); }
    SWR_ZEND_CATCH { bailed = 1; } SWR_ZEND_END_TRY();
    return bailed;
}
int swr_zend_try_facade_class_alias(const char *name, size_t length, zend_class_entry *entry, int *result) {
    int bailed = 0;
    SWR_ZEND_TRY { *result = zend_register_class_alias_ex(name, length, entry, true); }
    SWR_ZEND_CATCH { bailed = 1; } SWR_ZEND_END_TRY();
    return bailed;
}
HashTable *swr_zend_facade_compiler_functions(void) { return CG(function_table); }
