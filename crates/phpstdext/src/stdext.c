/* SPDX-License-Identifier: Apache-2.0
 * What crates/phpstdext needs from C (see README.md): Zend VM primitives whose
 * bodies are static in zend_execute.c (Swoole's copy, which upstream's
 * swoole_stdext.cc includes the same way) and PCRE's
 * cache, each inside zend_try. build.rs compiles it with the `php` and `active`
 * features. */
#include "php.h"
#include "zend_exceptions.h"
#include "ext/pcre/php_pcre.h"
#include "thirdparty/php/zend/zend_execute.c"

uint8_t swr_phpstdext_undefined_cv(zend_execute_data *execute_data, uint32_t var) {
    volatile uint8_t bailed = 0;
    zend_try { zval_undefined_cv(var EXECUTE_DATA_CC); }
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
