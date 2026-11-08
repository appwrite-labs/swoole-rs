// SPDX-License-Identifier: Apache-2.0
// What crates/phpstdext needs from C: Zend VM primitives whose inline bodies
// are private to zend_execute.c, and PCRE's cache, each inside zend_try.
#include "php_swoole_private.h"
#ifdef SW_STDEXT
#include "php_swoole_stdext.h"
#include "php_swoole_cxx.h"
#include "swoole_rs_phphelpers.h"
SW_EXTERN_C_BEGIN
#include "ext/pcre/php_pcre.h"
#include "thirdparty/php/zend/zend_execute.c"
SW_EXTERN_C_END

// Individual PHP VM primitives whose inline implementation is private to Zend.
extern "C" uint8_t swr_cpp_phpstdext_undefined_cv(zend_execute_data *execute_data, uint32_t var, zval **result) {
    uint8_t bailed = 0;
    zend_try { *result = zval_undefined_cv(var EXECUTE_DATA_CC); }
    zend_catch { bailed = 1; } zend_end_try();
    return bailed;
}
extern "C" uint8_t swr_cpp_phpstdext_dimension(zend_execute_data *execute_data, HashTable *table,
                                               const zval *key, bool constant, zval **result) {
    uint8_t bailed = 0;
    zend_try {
        *result = constant ? zend_fetch_dimension_address_inner_W_CONST(table, key EXECUTE_DATA_CC)
                           : zend_fetch_dimension_address_inner_W(table, key EXECUTE_DATA_CC);
    } zend_catch { bailed = 1; } zend_end_try();
    return bailed;
}
extern "C" uint8_t swr_cpp_phpstdext_assign(zval *slot, zval *value, uint8_t kind, bool strict, zval **result) {
    uint8_t bailed = 0;
    zend_try { *result = zend_assign_to_variable(slot, value, kind, strict); }
    zend_catch { bailed = 1; } zend_end_try();
    return bailed;
}
extern "C" uint8_t swr_cpp_phpstdext_binary(zval *slot, zval *value, const zend_op *opline) {
    uint8_t bailed = 0;
    zend_try { zend_binary_op(slot, slot, value OPLINE_CC); }
    zend_catch { bailed = 1; } zend_end_try();
    return bailed;
}
extern "C" uint8_t swr_cpp_phpstdext_typed_ref(zend_execute_data *execute_data, zend_reference *ref,
                                               zval *value, const zend_op *opline) {
    uint8_t bailed = 0;
    zend_try { zend_binary_assign_op_typed_ref(ref, value OPLINE_CC EXECUTE_DATA_CC); }
    zend_catch { bailed = 1; } zend_end_try();
    return bailed;
}
extern "C" uint8_t swr_cpp_phpstdext_json_decode(zval *value, const char *text, size_t length,
                                                 int64_t options, int64_t depth) {
    uint8_t bailed = 0;
    zend_try { zend::json_decode(value, text, length, options, depth); }
    zend_catch { bailed = 1; } zend_end_try();
    return bailed;
}
extern "C" uint8_t swr_cpp_phpstdext_pcre_cache(zend_string *regex, void **result) {
    uint8_t bailed = 0;
    zend_try { *result = pcre_get_compiled_regex_cache(regex); }
    zend_catch { bailed = 1; } zend_end_try();
    return bailed;
}
extern "C" void swr_cpp_phpstdext_pcre_retain(void *entry) {
    php_pcre_pce_incref(static_cast<pcre_cache_entry *>(entry));
}
extern "C" void swr_cpp_phpstdext_pcre_release(void *entry) {
    php_pcre_pce_decref(static_cast<pcre_cache_entry *>(entry));
}
extern "C" uint8_t swr_cpp_phpstdext_pcre_match(void *entry, zend_string *subject, zval *count,
                                               zval *result, bool global, bool flags_passed,
                                               int64_t flags, int64_t offset) {
    uint8_t bailed = 0;
    zend_try {
#if PHP_VERSION_ID >= 80400
        (void) flags_passed;
        php_pcre_match_impl(static_cast<pcre_cache_entry *>(entry), subject, count, result, global, flags, offset);
#else
        php_pcre_match_impl(static_cast<pcre_cache_entry *>(entry), subject, count, result, global, flags_passed, flags, offset);
#endif
    } zend_catch { bailed = 1; } zend_end_try();
    return bailed;
}
void php_swoole_stdext_minit(int module_number) { swr_v1_phpstdext_minit(); }
#endif
