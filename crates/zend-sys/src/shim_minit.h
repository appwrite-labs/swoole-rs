/* MINIT's registrations (swoole_rs_zend::Minit, crates/zend/src/minit.rs), over
 * NUL-terminated names. Each is total: where Zend would warn or raise a fatal
 * error it registers nothing and returns false, so none can bail out. */
#ifndef SWOOLE_RS_ZEND_SHIM_MINIT_H
#define SWOOLE_RS_ZEND_SHIM_MINIT_H
#include "shim.h"

/* REGISTER_{LONG,DOUBLE,BOOL,STRING}_CONSTANT, persistent, for module `module`.
 * False when a constant is defined as `name` already, where Zend warns. */
bool swr_zend_minit_long_constant(const char *name, zend_long value, int module);
bool swr_zend_minit_double_constant(const char *name, double value, int module);
bool swr_zend_minit_bool_constant(const char *name, bool value, int module);
bool swr_zend_minit_string_constant(const char *name, const char *value, int module);

/* zend_declare_class_constant_{long,string}. False when `ce` declares `name`
 * already or `name` is "class", where Zend raises a fatal error. */
bool swr_zend_minit_class_constant_long(zend_class_entry *ce, const char *name, zend_long value);
bool swr_zend_minit_class_constant_string(zend_class_entry *ce, const char *name, const char *value);

/* zend_declare_property of a default that is not refcounted (false for one that
 * is, where Zend raises a fatal error), and zend_declare_property_string. */
bool swr_zend_minit_property(zend_class_entry *ce, const char *name, const zval *value, int access);
void swr_zend_minit_property_string(zend_class_entry *ce, const char *name, const char *value, int access);

/* zend_add_parameter_attribute(method `lcname` of `ce`, `offset`, SensitiveParameter).
 * False when `ce` has no such internal method or it takes no such parameter. */
bool swr_zend_minit_sensitive_parameter(zend_class_entry *ce, const char *lcname, uint32_t offset);
#endif
