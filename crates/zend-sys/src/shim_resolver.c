#include "shim_resolver.h"

int swr_zend_try_call_method(zend_object *object, const char *name, size_t length, zval *retval, zval *arg) {
    int bailed = 0;
    zend_try {
        zend_call_method(object, NULL, NULL, name, length, retval, arg ? 1 : 0, arg, NULL);
    } zend_catch {
        bailed = 1;
    } zend_end_try();
    return bailed;
}

int swr_zend_try_zval_dtor(zval *value) {
    int bailed = 0;
    zend_try {
        zval_ptr_dtor_nogc(value);
    } zend_catch {
        bailed = 1;
    } zend_end_try();
    return bailed;
}
