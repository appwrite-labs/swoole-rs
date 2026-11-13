/* Declarations for shim_values.c; included by shim.h. */
#ifndef SWOOLE_RS_ZEND_SHIM_VALUES_H
#define SWOOLE_RS_ZEND_SHIM_VALUES_H
/* array_init / OBJ_RELEASE: an inline function and a macro. */
void swr_zend_array_init(zval *target);
void swr_zend_object_release(zend_object *object);
/* zend_get_type_by_const of a value's type (IS_NULL to IS_RESOURCE); NULL for
 * any other code, which Zend's switch does not name. */
const char *swr_zend_type_name(uint32_t kind);
#endif
