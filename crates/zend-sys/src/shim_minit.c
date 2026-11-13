/* Stays C (docs/zend.md, What stays C): globals (EG(zend_constants)), allocator (the persistent strings Zend makes of names and
 * values) and private (zend_add_parameter_attribute is a static inline). */
#include "shim_minit.h"
#include "zend_attributes.h"
#include "zend_constants.h"

/* zend_register_*_constant: a name that is defined already is refused with Zend's
 * warning, which at MINIT reaches no PHP error handler. */
#define SWR_MINIT_CONSTANT(suffix, type)                                                  \
    void swr_zend_minit_##suffix##_constant(const char *name, type value, int module) {   \
        zend_register_##suffix##_constant(name, strlen(name), value, CONST_PERSISTENT, module); \
    }
SWR_MINIT_CONSTANT(long, zend_long)
SWR_MINIT_CONSTANT(double, double)
SWR_MINIT_CONSTANT(bool, bool)
SWR_MINIT_CONSTANT(string, const char *)

/* Whether zend_declare_class_constant_ex would declare `name` rather than raise
 * a fatal error. */
static bool class_constant_free(zend_class_entry *ce, const char *name, size_t length) {
    return !zend_hash_str_exists(&ce->constants_table, name, length) &&
           !(length == sizeof("class") - 1 && zend_binary_strcasecmp(name, length, "class", length) == 0);
}

bool swr_zend_minit_class_constant_long(zend_class_entry *ce, const char *name, zend_long value) {
    size_t length = strlen(name);
    if (!class_constant_free(ce, name, length)) {
        return false;
    }
    zend_declare_class_constant_long(ce, name, length, value);
    return true;
}

bool swr_zend_minit_class_constant_string(zend_class_entry *ce, const char *name, const char *value) {
    size_t length = strlen(name);
    if (!class_constant_free(ce, name, length)) {
        return false;
    }
    zend_declare_class_constant_string(ce, name, length, value);
    return true;
}

bool swr_zend_minit_property(zend_class_entry *ce, const char *name, const zval *value, int access) {
    if (Z_REFCOUNTED_P(value)) {
        return false;
    }
    zval copy;
    ZVAL_COPY_VALUE(&copy, value);
    zend_declare_property(ce, name, strlen(name), &copy, access);
    return true;
}

void swr_zend_minit_property_string(zend_class_entry *ce, const char *name, const char *value, int access) {
    zend_declare_property_string(ce, name, strlen(name), value, access);
}

bool swr_zend_minit_sensitive_parameter(zend_class_entry *ce, const char *lcname, uint32_t offset) {
    zend_function *method = zend_hash_str_find_ptr(&ce->function_table, lcname, strlen(lcname));
    if (!method || method->type != ZEND_INTERNAL_FUNCTION || offset >= method->common.num_args) {
        return false;
    }
    zend_add_parameter_attribute(method, offset, ZSTR_KNOWN(ZEND_STR_SENSITIVEPARAMETER), 0);
    return true;
}
