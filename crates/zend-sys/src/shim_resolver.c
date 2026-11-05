#include "shim.h"
#include "shim_resolver.h"

int swr_zend_try_call_method(zend_object *object, const char *name, size_t length, zval *retval, zval *arg) {
    int bailed = 0;
    SWR_ZEND_TRY {
        zend_call_method(object, NULL, NULL, name, length, retval, arg ? 1 : 0, arg, NULL);
    } SWR_ZEND_CATCH {
        bailed = 1;
    } SWR_ZEND_END_TRY();
    return bailed;
}

int swr_zend_try_zval_dtor(zval *value) {
    int bailed = 0;
    SWR_ZEND_TRY {
        zval_ptr_dtor_nogc(value);
    } SWR_ZEND_CATCH {
        bailed = 1;
    } SWR_ZEND_END_TRY();
    return bailed;
}
