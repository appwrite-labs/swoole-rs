/* Declarations for shim_values.c; included by shim.h. */
#ifndef SWOOLE_RS_ZEND_SHIM_VALUES_H
#define SWOOLE_RS_ZEND_SHIM_VALUES_H
#include "php.h"
/* A call's arguments, as Rust lends them (zend::Zvals): `argc` zvals from
 * `argv`, which the callee reads and copies, keeping no pointer to them. */
typedef struct {
    zval *argv;
    uint32_t argc;
} swr_zvals;
/* array_init / OBJ_RELEASE: an inline function and a macro. */
void swr_zend_array_init(zval *target);
void swr_zend_object_release(zend_object *object);
/* zend_get_type_by_const of a value's type (IS_NULL to IS_RESOURCE); NULL for
 * any other code, which Zend's switch does not name. */
const char *swr_zend_type_name(uint32_t kind);
/* zend_strpprintf's "%.*H" of `value` (`precision` significant digits, as PHP's
 * `precision` ini prints, without the locale), or with `fixed` its "%.*F"
 * (`precision` decimals, without the locale): a new string. */
zend_string *swr_zend_double_text(double value, int precision, bool fixed);
#endif
