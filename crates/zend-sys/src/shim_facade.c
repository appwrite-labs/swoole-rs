/* Stays C (docs/zend.md, What stays C): bailout (truthiness, callable resolution
 * and exceptions run user code) and budget (zval_refcount_p). */
#include "shim.h"
#include "shim_facade.h"
int swr_zend_try_facade_truth(zval *value, bool *result) {
    volatile int bailed = 0;
    SWR_ZEND_TRY { *result = zend_is_true(value); }
    SWR_ZEND_CATCH { bailed = 1; } SWR_ZEND_END_TRY();
    return bailed;
}
int swr_zend_try_facade_callable(zval *value, zend_fcall_info_cache *cache, bool *result) {
    volatile int bailed = 0;
    SWR_ZEND_TRY { *result = zend_is_callable_at_frame(value, NULL, NULL, 0, cache, NULL); }
    SWR_ZEND_CATCH { bailed = 1; } SWR_ZEND_END_TRY();
    return bailed;
}
int swr_zend_try_facade_name(zval *value, zval *name) {
    volatile int bailed = 0;
    SWR_ZEND_TRY { ZVAL_STR(name, zend_get_callable_name_ex(value, NULL)); }
    SWR_ZEND_CATCH { bailed = 1; } SWR_ZEND_END_TRY();
    return bailed;
}
uint32_t swr_zval_facade_refcount(const zval *value) {
    uint8_t type = Z_TYPE_P(value);
    return type >= IS_STRING && type <= IS_REFERENCE ? GC_REFCOUNT(Z_COUNTED_P(value)) : 0;
}

int swr_zend_try_facade_throw(zend_class_entry *ce, const char *message, zend_long code) {
    volatile int bailed = 0;
    SWR_ZEND_TRY { zend_throw_exception(ce, message, code); }
    SWR_ZEND_CATCH { bailed = 1; } SWR_ZEND_END_TRY();
    return bailed;
}
int swr_zend_try_facade_throw_error(const char *message) {
    volatile int bailed = 0;
    SWR_ZEND_TRY { zend_throw_error(NULL, "%s", message); }
    SWR_ZEND_CATCH { bailed = 1; } SWR_ZEND_END_TRY();
    return bailed;
}
int swr_zend_try_facade_object_init(zval *target, zend_class_entry *ce) {
    volatile int bailed = 0;
    SWR_ZEND_TRY { object_init_ex(target, ce); }
    SWR_ZEND_CATCH { bailed = 1; } SWR_ZEND_END_TRY();
    return bailed;
}

/* bailout: an object's clone handler may invoke PHP __clone. */
int swr_zend_try_facade_clone(const zval *source, zval *result) {
    volatile int bailed = 0;
    SWR_ZEND_TRY {
        if (Z_TYPE_P(source) == IS_OBJECT && Z_OBJ_HT_P(source)->clone_obj) {
            zend_object *copy = Z_OBJ_HT_P(source)->clone_obj(Z_OBJ_P(source));
            if (copy) {
                ZVAL_OBJ(result, copy);
            }
        }
    }
    SWR_ZEND_CATCH { bailed = 1; } SWR_ZEND_END_TRY();
    return bailed;
}
