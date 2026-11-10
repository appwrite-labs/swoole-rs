/* Stays C (docs/zend.md, What stays C): bailout (methods and destructors run PHP). */
#include "shim.h"
#include "shim_resolver.h"

int swr_zend_try_call_method(zend_object *object, zend_class_entry *scope, const char *name, size_t length,
                             zval *retval, uint32_t argc, zval *arg1, zval *arg2) {
    volatile int bailed = 0;
    SWR_ZEND_TRY {
        zend_call_method(object, scope, NULL, name, length, retval, argc, arg1, arg2);
    } SWR_ZEND_CATCH {
        bailed = 1;
    } SWR_ZEND_END_TRY();
    return bailed;
}

int swr_zend_try_zval_dtor(zval *value) {
    volatile int bailed = 0;
    SWR_ZEND_TRY {
        zval_ptr_dtor_nogc(value);
    } SWR_ZEND_CATCH {
        bailed = 1;
    } SWR_ZEND_END_TRY();
    return bailed;
}
