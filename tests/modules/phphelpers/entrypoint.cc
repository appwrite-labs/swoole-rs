// Test-only PHP extension: invokes the unchanged public C++ helper entry in the
// already loaded immutable Swoole DSO. No production handler or ABI is modified.
#include "php_swoole_cxx.h"

ZEND_BEGIN_ARG_INFO_EX(arginfo_m26_named, 0, 0, 1)
    ZEND_ARG_TYPE_INFO(0, name, IS_STRING, 0)
    ZEND_ARG_VARIADIC_INFO(0, arguments)
ZEND_END_ARG_INFO()

PHP_FUNCTION(m26_named_call) {
    zend_string *name;
    zval *arguments = nullptr;
    int count = 0;
    ZEND_PARSE_PARAMETERS_START(1, -1)
        Z_PARAM_STR(name)
        Z_PARAM_VARIADIC('*', arguments, count)
    ZEND_PARSE_PARAMETERS_END();
    auto result = zend::function::call(std::string(ZSTR_VAL(name), ZSTR_LEN(name)), count, arguments);
    ZVAL_COPY(return_value, &result.value);
}

ZEND_BEGIN_ARG_INFO_EX(arginfo_m26_known, 0, 0, 0)
ZEND_END_ARG_INFO()

PHP_FUNCTION(m26_known_cache) {
    ZEND_PARSE_PARAMETERS_NONE();
    static const char *names[] = {
#define M26_KNOWN_NAME(id, name) name,
        SW_ZEND_KNOWN_STRINGS(M26_KNOWN_NAME)
#undef M26_KNOWN_NAME
    };
    size_t matches = 0, interned = 0;
    for (size_t i = 0; i < SW_ZEND_STR_LAST_KNOWN; ++i) {
        auto value = sw_zend_known_strings[i];
        const size_t length = strlen(names[i]);
        matches += ZSTR_LEN(value) == length && memcmp(ZSTR_VAL(value), names[i], length) == 0;
        interned += ZSTR_IS_INTERNED(value) != 0;
    }
    array_init(return_value);
    add_assoc_long(return_value, "count", SW_ZEND_STR_LAST_KNOWN);
    add_assoc_long(return_value, "matches", matches);
    add_assoc_long(return_value, "interned", interned);
}

PHP_FUNCTION(m26_owned_callable) {
    zend_string *name;
    ZEND_PARSE_PARAMETERS_START(1, 1)
        Z_PARAM_STR(name)
    ZEND_PARSE_PARAMETERS_END();
    auto owned = zend::function::call(std::string(ZSTR_VAL(name), ZSTR_LEN(name)), 0, nullptr);
    {
        zend::Callable callable(&owned.value);
        zval_ptr_dtor(&owned.value);
        ZVAL_UNDEF(&owned.value);
        // Callable holds the sole remaining reference. Its native value dtor
        // executes PHP __destruct/reentry before the original name is freed.
    }
    RETURN_TRUE;
}

ZEND_BEGIN_ARG_INFO_EX(arginfo_m26_bool_call, 0, 0, 3)
    ZEND_ARG_CALLABLE_INFO(0, callback, 0)
    ZEND_ARG_TYPE_INFO(0, coroutine, _IS_BOOL, 0)
    ZEND_ARG_TYPE_INFO(0, capture, _IS_BOOL, 0)
ZEND_END_ARG_INFO()

PHP_FUNCTION(m26_bool_call) {
    zval *callback;
    bool coroutine, capture;
    ZEND_PARSE_PARAMETERS_START(3, 3)
        Z_PARAM_ZVAL(callback)
        Z_PARAM_BOOL(coroutine)
        Z_PARAM_BOOL(capture)
    ZEND_PARSE_PARAMETERS_END();
    zend::Callable callable(callback);
    if (!callable.ready()) { RETURN_FALSE; }
    zval value;
    ZVAL_UNDEF(&value);
    const bool success = zend::function::call(callable.ptr(), 0, nullptr, capture ? &value : nullptr, coroutine);
    array_init(return_value);
    add_assoc_bool(return_value, "success", success);
    add_assoc_bool(return_value, "capture", capture);
    add_assoc_bool(return_value, "value_defined", !Z_ISUNDEF(value));
    if (capture) {
        if (Z_ISUNDEF(value)) { add_assoc_null(return_value, "value"); }
        else { add_assoc_zval(return_value, "value", &value); }
    }
}

static const zend_function_entry functions[] = {
    PHP_FE(m26_named_call, arginfo_m26_named)
    PHP_FE(m26_known_cache, arginfo_m26_known)
    PHP_FE(m26_owned_callable, arginfo_m26_named)
    PHP_FE(m26_bool_call, arginfo_m26_bool_call)
    PHP_FE_END
};

extern "C" {
zend_module_entry m26_helpers_module_entry = {
    STANDARD_MODULE_HEADER, "m26_helpers", functions,
    nullptr, nullptr, nullptr, nullptr, nullptr, "0", STANDARD_MODULE_PROPERTIES
};
ZEND_GET_MODULE(m26_helpers)
}
