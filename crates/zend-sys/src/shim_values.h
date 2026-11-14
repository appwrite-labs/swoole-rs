/* Declarations for shim_values.c; included by shim.h. */
#ifndef SWOOLE_RS_ZEND_SHIM_VALUES_H
#define SWOOLE_RS_ZEND_SHIM_VALUES_H
/* array_init / OBJ_RELEASE / GC_ADDREF: inline functions and macros. */
void swr_zend_array_init(zval *target);
void swr_zend_object_release(zend_object *object);
void swr_zend_object_addref(zend_object *object);

/* zval_get_long, whose slow path's signature depends on the PHP version. */
zend_long swr_zval_get_long(const zval *value);


/* zend_is_true, whose return type depends on the PHP version. */
bool swr_zend_is_true(const zval *value);
#endif
