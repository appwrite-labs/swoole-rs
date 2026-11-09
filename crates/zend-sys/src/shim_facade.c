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
int swr_zend_try_facade_name(zval *value, zend_string **result) {
    volatile int bailed = 0;
    SWR_ZEND_TRY { *result = zend_get_callable_name_ex(value, NULL); }
    SWR_ZEND_CATCH { bailed = 1; } SWR_ZEND_END_TRY();
    return bailed;
}
uint32_t swr_zval_facade_refcount(const zval *value) { return zval_refcount_p(value); }

int swr_zend_try_facade_throw(zend_class_entry *ce, const char *message, zend_long code) {
    volatile int bailed = 0;
    SWR_ZEND_TRY { zend_throw_exception(ce, message, code); }
    SWR_ZEND_CATCH { bailed = 1; } SWR_ZEND_END_TRY();
    return bailed;
}
