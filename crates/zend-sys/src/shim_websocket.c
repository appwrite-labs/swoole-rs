/* Stays C (docs/zend.md, What stays C): bailout (a subclass's __get). */
#include "shim.h"
#include "shim_websocket.h"

zval *swr_zend_ws_property_not_null(zend_class_entry *scope, zend_object *object, const char *name, size_t length,
                                    zval *rv, int *bailed) {
    zval *volatile property = NULL;
    *bailed = 0;
    SWR_ZEND_TRY {
        property = zend_read_property(scope, object, name, length, 1, rv);
    }
    SWR_ZEND_CATCH {
        *bailed = 1;
    }
    SWR_ZEND_END_TRY();
    if (*bailed) {
        return NULL;
    }
    uint8_t type = Z_TYPE_P(property);
    return (type == IS_NULL || UNEXPECTED(type == IS_UNDEF)) ? NULL : property;
}
