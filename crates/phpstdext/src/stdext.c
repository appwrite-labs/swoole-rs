/* SPDX-License-Identifier: Apache-2.0
 * What crates/phpstdext needs from C (see README.md): Zend VM primitives whose
 * bodies are static in zend_execute.c (Swoole's copy, which upstream's
 * swoole_stdext.cc includes the same way), PCRE's cache, and JSON's decoder,
 * each inside zend_try. build.rs compiles it with the `php` and `active`
 * features. */
#include "php.h"
#include "zend_exceptions.h"
#include "ext/json/php_json.h"
#include "ext/pcre/php_pcre.h"
#include "thirdparty/php/zend/zend_execute.c"

extern PHP_JSON_API zend_class_entry *php_json_exception_ce;

uint8_t swr_phpstdext_undefined_cv(zend_execute_data *execute_data, uint32_t var, zval **result) {
    volatile uint8_t bailed = 0;
    zend_try { *result = zval_undefined_cv(var EXECUTE_DATA_CC); }
    zend_catch { bailed = 1; } zend_end_try();
    return bailed;
}

uint8_t swr_phpstdext_dimension(zend_execute_data *execute_data, HashTable *table, const zval *key, bool constant,
                                zval **result) {
    volatile uint8_t bailed = 0;
    zend_try {
        *result = constant ? zend_fetch_dimension_address_inner_W_CONST(table, key EXECUTE_DATA_CC)
                           : zend_fetch_dimension_address_inner_W(table, key EXECUTE_DATA_CC);
    } zend_catch { bailed = 1; } zend_end_try();
    return bailed;
}

uint8_t swr_phpstdext_assign(zval *slot, zval *value, uint8_t kind, bool strict, zval **result) {
    volatile uint8_t bailed = 0;
    zend_try { *result = zend_assign_to_variable(slot, value, kind, strict); }
    zend_catch { bailed = 1; } zend_end_try();
    return bailed;
}

uint8_t swr_phpstdext_binary(zval *slot, zval *value, const zend_op *opline) {
    volatile uint8_t bailed = 0;
    zend_try { zend_binary_op(slot, slot, value OPLINE_CC); }
    zend_catch { bailed = 1; } zend_end_try();
    return bailed;
}

uint8_t swr_phpstdext_typed_ref(zend_execute_data *execute_data, zend_reference *ref, zval *value,
                                const zend_op *opline) {
    volatile uint8_t bailed = 0;
    zend_try { zend_binary_assign_op_typed_ref(ref, value OPLINE_CC EXECUTE_DATA_CC); }
    zend_catch { bailed = 1; } zend_end_try();
    return bailed;
}

/* Swoole's zend::json_decode (thirdparty/php/standard/var_decoder.cc), which is C++. */
static void json_decode(zval *return_value, const char *str, size_t str_len, zend_long options, zend_long depth) {
    if (!(options & PHP_JSON_THROW_ON_ERROR)) {
        JSON_G(error_code) = PHP_JSON_ERROR_NONE;
    }
    if (!str_len) {
        if (!(options & PHP_JSON_THROW_ON_ERROR)) {
            JSON_G(error_code) = PHP_JSON_ERROR_SYNTAX;
        } else {
            zend_throw_exception(php_json_exception_ce, "Syntax error", PHP_JSON_ERROR_SYNTAX);
        }
        RETURN_NULL();
    }
    if (depth <= 0) {
        php_error_docref(NULL, E_WARNING, "Depth must be greater than zero");
        RETURN_NULL();
    }
    if (depth > INT_MAX) {
        php_error_docref(NULL, E_WARNING, "Depth must be lower than %d", INT_MAX);
        RETURN_NULL();
    }
    php_json_decode_ex(return_value, str, str_len, options, depth);
}

uint8_t swr_phpstdext_json_decode(zval *value, const char *text, size_t length, int64_t options, int64_t depth) {
    volatile uint8_t bailed = 0;
    zend_try { json_decode(value, text, length, options, depth); }
    zend_catch { bailed = 1; } zend_end_try();
    return bailed;
}

uint8_t swr_phpstdext_pcre_cache(zend_string *regex, void **result) {
    volatile uint8_t bailed = 0;
    zend_try { *result = pcre_get_compiled_regex_cache(regex); }
    zend_catch { bailed = 1; } zend_end_try();
    return bailed;
}

void swr_phpstdext_pcre_retain(void *entry) {
    php_pcre_pce_incref(entry);
}

void swr_phpstdext_pcre_release(void *entry) {
    php_pcre_pce_decref(entry);
}

uint8_t swr_phpstdext_pcre_match(void *entry, zend_string *subject, zval *count, zval *result, bool global,
                                 bool flags_passed, int64_t flags, int64_t offset) {
    volatile uint8_t bailed = 0;
    zend_try {
#if PHP_VERSION_ID >= 80400
        (void) flags_passed;
        php_pcre_match_impl(entry, subject, count, result, global, flags, offset);
#else
        php_pcre_match_impl(entry, subject, count, result, global, flags_passed, flags, offset);
#endif
    } zend_catch { bailed = 1; } zend_end_try();
    return bailed;
}
