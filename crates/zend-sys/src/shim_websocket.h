/* Shims the WebSocket classes (crates/phphttp/src/websocket.rs) need. */
#ifndef SWOOLE_RS_ZEND_SHIM_WEBSOCKET_H
#define SWOOLE_RS_ZEND_SHIM_WEBSOCKET_H
#include "shim.h"

/* sw_zend_read_property_not_null_ex(scope, object, name, 1) inside zend_try (an
 * unset property of a subclass runs its __get): the property, or NULL when it
 * is null or undefined; 1 in *bailed if __get bailed out. */
zval *swr_zend_ws_property_not_null(zend_class_entry *scope, zend_object *object, const char *name, size_t length,
                                    zval *rv, int *bailed);

#endif
