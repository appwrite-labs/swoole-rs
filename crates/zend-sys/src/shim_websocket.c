#include "shim_websocket.h"

zval *swr_zend_ws_property_not_null(zend_class_entry *scope, zend_object *object, const char *name, size_t length,
                                    zval *rv, int *bailed) {
    zval *volatile property = NULL;
    *bailed = 0;
    zend_try {
        property = zend_read_property(scope, object, name, length, 1, rv);
    }
    zend_catch {
        *bailed = 1;
    }
    zend_end_try();
    if (*bailed) {
        return NULL;
    }
    uint8_t type = Z_TYPE_P(property);
    return (type == IS_NULL || UNEXPECTED(type == IS_UNDEF)) ? NULL : property;
}
