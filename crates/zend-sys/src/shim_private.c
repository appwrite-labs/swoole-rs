/* Stays C (docs/zend.md, What stays C): bailout (property hooks, user calls),
 * globals (EG(function_table), EG(exception), CG(function_table)). */
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
zend_object *swr_zend_facade_exception(void) { return EG(exception); }
void swr_zend_facade_exception_error(void *exception) { zend_exception_error(exception, E_ERROR); }
int swr_zend_try_facade_user_call(zval *name, uint32_t argc, zval *argv, zval *retval, int *result) {
    volatile int bailed = 0;
    SWR_ZEND_TRY { *result = call_user_function(EG(function_table), NULL, name, retval, argc, argv); }
    SWR_ZEND_CATCH { bailed = 1; } SWR_ZEND_END_TRY();
    return bailed;
}
/* A non-null `then` runs in the same zend_try once the coroutine runs or waits and no
 * exception is pending, and *ran says so: the caller's work after the call costs no
 * second setjmp (#3260). No bailout is pending there: a coroutine raises its own. */
int swr_zend_try_facade_coroutine(long (*create)(zend_fcall_info_cache *, uint32_t, zval *, zval *, uint8_t *),
                                  zend_fcall_info_cache *cache, uint32_t argc, zval *argv, zval *callable,
                                  long *result, swr_zend_then then, void *arg, bool *ran) {
    volatile int bailed = 0;
    uint8_t raised = 0; /* read only when nothing jumped */
    SWR_ZEND_TRY {
        *result = create(cache, argc, argv, callable, &raised);
        if (then && !raised && *result >= 0 && !EG(exception)) {
            *ran = true;
            then(arg);
        }
    }
    SWR_ZEND_CATCH { bailed = 1; } SWR_ZEND_END_TRY();
    return bailed || raised;
}
int swr_zend_try_facade_intern(zend_string *string, zend_string **result) {
    volatile int bailed = 0;
    SWR_ZEND_TRY { *result = zend_new_interned_string(string); }
    SWR_ZEND_CATCH { bailed = 1; } SWR_ZEND_END_TRY();
    return bailed;
}
HashTable *swr_zend_facade_function_table(void) { return EG(function_table); }

int swr_zend_try_facade_creation_error(zend_class_entry *ce) {
    volatile int bailed = 0;
    SWR_ZEND_TRY { zend_throw_error(NULL, "The object of %s can not be created for security reasons", ZSTR_VAL(ce->name)); }
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
