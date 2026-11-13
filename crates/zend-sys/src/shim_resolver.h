/* Method calls inside zend_try (bailout::call_method, Swoole\NameResolver's lookup). */
#ifndef SWOOLE_RS_ZEND_SHIM_RESOLVER_H
#define SWOOLE_RS_ZEND_SHIM_RESOLVER_H

#include "shim.h"

/* zend_call_method(object, scope, NULL, name, length, retval, argc, arg1, arg2)
 * inside zend_try: 1 if it bailed out. A null scope is the object's class. */
int swr_zend_try_call_method(zend_object *object, zend_class_entry *scope, const char *name, size_t length,
                             zval *retval, uint32_t argc, zval *arg1, zval *arg2);
#endif
