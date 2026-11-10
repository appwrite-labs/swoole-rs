/* Stays C (docs/zend.md, What stays C): bailout (property hooks, user calls,
 * allocation failures), allocator, globals (EG(function_table), EG(exception),
 * CG(function_table)) and private (ZEND_CLOSURE_OBJECT). */
#include "shim.h"
#include "shim_private.h"
int swr_zend_try_facade_read(zend_class_entry *scope, zend_object *object, const char *name, size_t length,
                             bool silent, zval *rv, zval **result) {
    volatile int bailed = 0;
    SWR_ZEND_TRY { *result = zend_read_property(scope, object, name, length, silent, rv); }
    SWR_ZEND_CATCH { bailed = 1; } SWR_ZEND_END_TRY();
    return bailed;
}
int swr_zend_try_facade_update(zend_class_entry *scope, zend_object *object, const char *name, size_t length, zval *value) {
    volatile int bailed = 0;
    SWR_ZEND_TRY { zend_update_property(scope, object, name, length, value); }
    SWR_ZEND_CATCH { bailed = 1; } SWR_ZEND_END_TRY();
    return bailed;
}
int swr_zend_try_facade_null(zend_class_entry *scope, zend_object *object, const char *name, size_t length) {
    volatile int bailed = 0;
    SWR_ZEND_TRY { zend_update_property_null(scope, object, name, length); }
    SWR_ZEND_CATCH { bailed = 1; } SWR_ZEND_END_TRY();
    return bailed;
}
zend_object *swr_zend_facade_closure(zend_function *function) { return ZEND_CLOSURE_OBJECT(function); }
void swr_zend_facade_object_release(void *object) { OBJ_RELEASE((zend_object *) object); }
zend_object *swr_zend_facade_exception(void) { return EG(exception); }
void swr_zend_facade_exception_error(void *exception) { zend_exception_error(exception, E_ERROR); }
int swr_zend_try_facade_user_call(zval *name, uint32_t argc, zval *argv, zval *retval, int *result) {
    volatile int bailed = 0;
    SWR_ZEND_TRY { *result = call_user_function(EG(function_table), NULL, name, retval, argc, argv); }
    SWR_ZEND_CATCH { bailed = 1; } SWR_ZEND_END_TRY();
    return bailed;
}
int swr_zend_try_facade_coroutine(long (*create)(zend_fcall_info_cache *, uint32_t, zval *, zval *, uint8_t *),
                                  zend_fcall_info_cache *cache, uint32_t argc, zval *argv, zval *callable,
                                  long *result) {
    volatile int bailed = 0;
    uint8_t raised = 0; /* read only when nothing jumped */
    SWR_ZEND_TRY { *result = create(cache, argc, argv, callable, &raised); }
    SWR_ZEND_CATCH { bailed = 1; } SWR_ZEND_END_TRY();
    return bailed || raised;
}
int swr_zend_try_facade_palloc(size_t size, void **result) {
    volatile int bailed = 0;
    SWR_ZEND_TRY { *result = pemalloc(size, 1); }
    SWR_ZEND_CATCH { bailed = 1; } SWR_ZEND_END_TRY();
    return bailed;
}
void swr_zend_facade_pfree(void *allocation) { pefree(allocation, 1); }
int swr_zend_try_facade_pstring(const char *text, size_t length, zend_string **result) {
    volatile int bailed = 0;
    SWR_ZEND_TRY { *result = zend_string_init(text, length, 1); }
    SWR_ZEND_CATCH { bailed = 1; } SWR_ZEND_END_TRY();
    return bailed;
}
int swr_zend_try_facade_intern(zend_string *string, zend_string **result) {
    volatile int bailed = 0;
    SWR_ZEND_TRY { *result = zend_new_interned_string(string); }
    SWR_ZEND_CATCH { bailed = 1; } SWR_ZEND_END_TRY();
    return bailed;
}
HashTable *swr_zend_facade_function_table(void) { return EG(function_table); }

int swr_zend_try_facade_alloc(size_t size, void **result) {
    volatile int bailed = 0;
    SWR_ZEND_TRY { *result = emalloc(size); }
    SWR_ZEND_CATCH { bailed = 1; } SWR_ZEND_END_TRY();
    return bailed;
}
int swr_zend_try_facade_string(const char *text, size_t length, zend_string **result) {
    volatile int bailed = 0;
    SWR_ZEND_TRY { *result = zend_string_init(text, length, 0); }
    SWR_ZEND_CATCH { bailed = 1; } SWR_ZEND_END_TRY();
    return bailed;
}
int swr_zend_try_facade_value_string(zval *value, const char *text, size_t length) {
    volatile int bailed = 0;
    SWR_ZEND_TRY { ZVAL_STRINGL(value, text, length); }
    SWR_ZEND_CATCH { bailed = 1; } SWR_ZEND_END_TRY();
    return bailed;
}
int swr_zend_try_facade_object_alloc(size_t size, zend_class_entry *ce, void **result) {
    volatile int bailed = 0;
    SWR_ZEND_TRY { *result = zend_object_alloc(size, ce); }
    SWR_ZEND_CATCH { bailed = 1; } SWR_ZEND_END_TRY();
    return bailed;
}
int swr_zend_try_facade_array_init(zval *value) {
    volatile int bailed = 0;
    SWR_ZEND_TRY { array_init(value); }
    SWR_ZEND_CATCH { bailed = 1; } SWR_ZEND_END_TRY();
    return bailed;
}
int swr_zend_try_facade_objects_new(zend_class_entry *ce, zend_object **result) {
    volatile int bailed = 0;
    SWR_ZEND_TRY { *result = zend_objects_new(ce); }
    SWR_ZEND_CATCH { bailed = 1; } SWR_ZEND_END_TRY();
    return bailed;
}
int swr_zend_try_facade_creation_error(zend_class_entry *ce) {
    volatile int bailed = 0;
    SWR_ZEND_TRY { zend_throw_error(NULL, "The object of %s can not be created for security reasons", ZSTR_VAL(ce->name)); }
    SWR_ZEND_CATCH { bailed = 1; } SWR_ZEND_END_TRY();
    return bailed;
}

int swr_zend_try_facade_object_init(zend_object *object, zend_class_entry *ce) {
    volatile int bailed = 0;
    SWR_ZEND_TRY { zend_object_std_init(object, ce); }
    SWR_ZEND_CATCH { bailed = 1; } SWR_ZEND_END_TRY();
    return bailed;
}
int swr_zend_try_facade_properties_init(zend_object *object, zend_class_entry *ce) {
    volatile int bailed = 0;
    SWR_ZEND_TRY { object_properties_init(object, ce); }
    SWR_ZEND_CATCH { bailed = 1; } SWR_ZEND_END_TRY();
    return bailed;
}
int swr_zend_try_facade_object_dtor(zend_object *object) {
    volatile int bailed = 0;
    SWR_ZEND_TRY { zend_object_std_dtor(object); }
    SWR_ZEND_CATCH { bailed = 1; } SWR_ZEND_END_TRY();
    return bailed;
}

int swr_zend_try_facade_callable_frame(zval *value, zend_object *object, zend_execute_data *frame,
                                      uint32_t flags, zend_fcall_info_cache *cache, char **error, bool *result) {
    volatile int bailed = 0;
    SWR_ZEND_TRY { *result = zend_is_callable_at_frame(value, object, frame, flags, cache, error); }
    SWR_ZEND_CATCH { bailed = 1; } SWR_ZEND_END_TRY();
    return bailed;
}
int swr_zend_try_facade_callable_name(zval *value, zend_object *object, zend_string **result) {
    volatile int bailed = 0;
    SWR_ZEND_TRY { *result = zend_get_callable_name_ex(value, object); }
    SWR_ZEND_CATCH { bailed = 1; } SWR_ZEND_END_TRY();
    return bailed;
}

int swr_zend_try_facade_register_functions(const zend_function_entry *entries, HashTable *table, int type, int *result) {
    volatile int bailed = 0;
    SWR_ZEND_TRY { *result = zend_register_functions(NULL, entries, table, type); }
    SWR_ZEND_CATCH { bailed = 1; } SWR_ZEND_END_TRY();
    return bailed;
}
int swr_zend_try_facade_class_alias(const char *name, size_t length, zend_class_entry *entry, int *result) {
    volatile int bailed = 0;
    SWR_ZEND_TRY { *result = zend_register_class_alias_ex(name, length, entry, true); }
    SWR_ZEND_CATCH { bailed = 1; } SWR_ZEND_END_TRY();
    return bailed;
}
HashTable *swr_zend_facade_compiler_functions(void) { return CG(function_table); }
