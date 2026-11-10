/* Core shims. Stays C (docs/zend.md, What stays C): bailout (swr_zend_try and
 * swr_zend_try_call own the setjmp), allocator (zend_object_alloc, the emalloc
 * family, zend_string_*), version (INIT_CLASS_ENTRY_EX, ce->create_object in an
 * anonymous union, zend_parse_arg_func's signature) and budget (Z_TRY_ADDREF_P,
 * Z_TRY_DELREF_P, ZVAL_STR). */
#include "shim.h"

void *swr_zend_object_alloc(size_t size, zend_class_entry *ce) {
    return zend_object_alloc(size, ce);
}

zend_class_entry *swr_zend_register_class(const char *name, size_t length,
                                          const zend_function_entry *functions,
                                          zend_class_entry *parent) {
    zend_class_entry ce;
    INIT_CLASS_ENTRY_EX(ce, name, length, functions);
    return zend_register_internal_class_ex(&ce, parent);
}

void swr_zend_class_set_create_object(zend_class_entry *ce,
                                      zend_object *(*create)(zend_class_entry *ce)) {
    ce->create_object = create;
}

void swr_zend_array_init(zval *target) {
    array_init(target);
}

void *swr_zend_emalloc(size_t size) {
    return emalloc(size);
}

void swr_zend_efree(void *ptr) {
    efree(ptr);
}

void *swr_zend_ecalloc(size_t count, size_t size) {
    return ecalloc(count, size);
}

void *swr_zend_erealloc(void *ptr, size_t size) {
    return erealloc(ptr, size);
}

void swr_zval_try_addref(zval *value) {
    Z_TRY_ADDREF_P(value);
}

void swr_zval_try_delref(zval *value) {
    Z_TRY_DELREF_P(value);
}

zend_string *swr_zend_string_init(const char *text, size_t length) {
    return zend_string_init(text, length, 0);
}

zend_string *swr_zend_string_persistent(const char *text, size_t length) {
    return zend_string_init(text, length, 1);
}

zend_string *swr_zend_string_copy(zend_string *string) {
    return zend_string_copy(string);
}

void swr_zend_string_release(zend_string *string) {
    zend_string_release(string);
}

void swr_zval_str(zval *value, zend_string *string) {
    ZVAL_STR(value, string);
}

int swr_zend_try(void (*body)(void *), void *arg) {
    volatile int bailed = 0;
    SWR_ZEND_TRY {
        body(arg);
    }
    SWR_ZEND_CATCH {
        bailed = 1;
    }
    SWR_ZEND_END_TRY();
    return bailed ? (SWR_ZEND_EXIT_PENDING() ? SWR_ZEND_THREAD_EXIT : SWR_ZEND_BAILOUT) : SWR_ZEND_RETURN;
}

int swr_zend_try_call(zend_fcall_info *fci, zend_fcall_info_cache *fcc, int *result) {
    volatile int bailed = 0;
    *result = 0;
    SWR_ZEND_TRY {
        *result = zend_call_function(fci, fcc) == SUCCESS;
    }
    SWR_ZEND_CATCH {
        bailed = 1;
    }
    SWR_ZEND_END_TRY();
    return bailed;
}

int swr_zend_parse_func(zval *arg, uint32_t number, zend_fcall_info *fci, zend_fcall_info_cache *fcc) {
    char *error = NULL;
#if PHP_VERSION_ID >= 80400
    if (zend_parse_arg_func(arg, fci, fcc, 0, &error, 1)) {
#else
    if (zend_parse_arg_func(arg, fci, fcc, 0, &error)) {
#endif
        return 1;
    }
    zend_wrong_parameter_error(error ? ZPP_ERROR_WRONG_CALLBACK : ZPP_ERROR_WRONG_ARG, number, error,
                               Z_EXPECTED_FUNC, arg);
    return 0;
}
