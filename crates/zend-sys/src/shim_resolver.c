/* Stays C (docs/zend.md, What stays C): bailout (methods run PHP). */
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
