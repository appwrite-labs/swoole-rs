/* Stays C (docs/zend.md, What stays C): bailout (property hooks, user calls),
 * globals (EG(function_table), EG(exception), CG(function_table)). */
#include "shim.h"
#include "shim_private.h"
zend_object *swr_zend_facade_exception(void) { return EG(exception); }
static void report_exception(void *exception) { zend_exception_error(exception, E_ERROR); }
int swr_zend_try_uncaught_exception(bool *unwind_exit) {
    zend_object *exception = EG(exception);
    *unwind_exit = exception && zend_is_unwind_exit(exception);
    return exception && !*unwind_exit ? swr_zend_try(report_exception, exception) : SWR_ZEND_RETURN;
}
int swr_zend_try_facade_user_call(zval *name, swr_zvals args, zval *retval, int *result) {
    volatile int bailed = 0;
    SWR_ZEND_TRY { *result = call_user_function(EG(function_table), NULL, name, retval, args.argc, args.argv); }
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
HashTable *swr_zend_facade_function_table(void) { return EG(function_table); }

zend_object *swr_zend_object_deny(zend_class_entry *ce, bool *bailed) {
    zend_object *object = zend_objects_new(ce);
    for (int i = 0; i < ce->default_properties_count; i++) {
        ZVAL_UNDEF(OBJ_PROP_NUM(object, i));
    }
    volatile bool trapped = false;
    SWR_ZEND_TRY { zend_throw_error(NULL, "The object of %s can not be created for security reasons", ZSTR_VAL(ce->name)); }
    SWR_ZEND_CATCH { trapped = true; } SWR_ZEND_END_TRY();
    *bailed = trapped;
    return object;
}

int swr_zend_try_facade_callable_frame(zval *value, zend_execute_data *frame, zend_fcall_info_cache *cache,
                                       bool *result) {
    volatile int bailed = 0;
    SWR_ZEND_TRY {
        zend_object *object = Z_TYPE(frame->This) == IS_OBJECT ? Z_OBJ(frame->This) : NULL;
        *result = zend_is_callable_at_frame(value, object, frame, 0, cache, NULL);
    }
    SWR_ZEND_CATCH { bailed = 1; } SWR_ZEND_END_TRY();
    return bailed;
}

int swr_zend_try_facade_register_functions(const zend_function_entry *entries, HashTable *table, int type, int *result) {
    volatile int bailed = 0;
    SWR_ZEND_TRY { *result = zend_register_functions(NULL, entries, table, type); }
    SWR_ZEND_CATCH { bailed = 1; } SWR_ZEND_END_TRY();
    return bailed;
}
HashTable *swr_zend_facade_compiler_functions(void) { return CG(function_table); }
