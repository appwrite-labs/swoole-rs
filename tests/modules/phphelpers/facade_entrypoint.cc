// Test-only consumer of the actual public native/M26 facade header.
#include "php_swoole_cxx.h"
ZEND_BEGIN_ARG_INFO_EX(arginfo_m26_facade,0,0,2)
    ZEND_ARG_TYPE_INFO(0, stringable, IS_OBJECT, 0)
    ZEND_ARG_INFO(0, callback)
ZEND_END_ARG_INFO()
PHP_FUNCTION(m26_facade_custody) {
    zval *stringable,*callback;
    ZEND_PARSE_PARAMETERS_START(2,2)
        Z_PARAM_OBJECT(stringable)
        Z_PARAM_ZVAL(callback)
    ZEND_PARSE_PARAMETERS_END();
    array_init(return_value);
    (void) stringable;
    {
        auto original=sw_callable_create(callback);
        if(!original) {add_assoc_bool(return_value,"callable_ready",false);return;}
        auto duplicate=original->dup();
        zval called;ZVAL_UNDEF(&called);
        bool success=duplicate->call(0,nullptr,&called);
        add_assoc_bool(return_value,"callable_ready",original->ready() && duplicate->ready() && success);
        if(Z_ISUNDEF(called))add_assoc_null(return_value,"called");else add_assoc_zval(return_value,"called",&called);
        delete duplicate;delete original;
    }
}
PHP_FUNCTION(m26_private_custody) {
    zval *object, *callback;
    ZEND_PARSE_PARAMETERS_START(2,2)
        Z_PARAM_OBJECT(object)
        Z_PARAM_ZVAL(callback)
    ZEND_PARSE_PARAMETERS_END();
    array_init(return_value);
    auto *scope = Z_OBJCE_P(object);
    auto *array = sw_zend_read_and_convert_property_array(scope, object, ZEND_STRL("container"), 1);
    add_assoc_long(return_value, "converted_count", zend_hash_num_elements(Z_ARRVAL_P(array)));
    auto *heap = static_cast<zval *>(emalloc(sizeof(zval))); ZVAL_STRING(heap, "heap");
    auto *copy = sw_zval_dup(heap);
    add_assoc_bool(return_value, "shallow_copy", Z_STR_P(copy) == Z_STR_P(heap) && Z_REFCOUNT_P(heap) == 1);
    zval_ptr_dtor(heap); efree(heap); efree(copy);
    (void) callback;
}

static int class_alias_registered;
ZEND_BEGIN_ARG_INFO_EX(arginfo_m26_utility,0,0,2)
    ZEND_ARG_INFO(0, callback)
    ZEND_ARG_TYPE_INFO(0, data, IS_OBJECT, 0)
ZEND_END_ARG_INFO()
PHP_FUNCTION(m26_utility_custody) {
    zval *callback, *data;
    ZEND_PARSE_PARAMETERS_START(2,2) Z_PARAM_ZVAL(callback) Z_PARAM_OBJECT(data) ZEND_PARSE_PARAMETERS_END();
    array_init(return_value);
    (void) callback; (void) data;
    add_assoc_long(return_value,"class_alias_registered",class_alias_registered);
}
PHP_MINIT_FUNCTION(m26_facade) {
    (void) type; (void) module_number;
    zend_class_entry entry;
    INIT_CLASS_ENTRY(entry,"M26Guard",nullptr);
    auto *guard=zend_register_internal_class(&entry);
    class_alias_registered=sw_zend_register_class_alias(ZEND_STRL("M26GuArDAlias"),guard);
    return SUCCESS;
}
static const zend_function_entry functions[]={PHP_FE(m26_facade_custody,arginfo_m26_facade) PHP_FE(m26_private_custody,arginfo_m26_facade) PHP_FE(m26_utility_custody,arginfo_m26_utility) PHP_FE_END};
extern "C" {
zend_module_entry m26_facade_module_entry={STANDARD_MODULE_HEADER,"m26_facade",functions,PHP_MINIT(m26_facade),nullptr,nullptr,nullptr,nullptr,"0",STANDARD_MODULE_PROPERTIES};
ZEND_GET_MODULE(m26_facade)
}
