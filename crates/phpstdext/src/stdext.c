/* SPDX-License-Identifier: Apache-2.0
 * What crates/phpstdext needs from C (see README.md): Zend VM primitives whose
 * bodies are static in zend_execute.c (Swoole's copy, which upstream's
 * swoole_stdext.cc includes the same way) and PCRE's
 * cache, each inside zend_try. build.rs compiles it with the `php` and `active`
 * features. */
#include "php.h"
#include "zend_exceptions.h"
#include "ext/pcre/php_pcre.h"
#include <sys/uio.h>
#include "thirdparty/php/zend/zend_execute.c"

/* Each shim below reads what it needs of the frame's current instruction, and
 * does nothing (0) unless that instruction is an assignment to a dimension
 * (whose OP_DATA follows it): the Rust side passes only the live frame Zend
 * gave its handler, and live zvals. */
static bool swr_phpstdext_assigning(const zend_op *opline) {
    return opline->opcode == ZEND_ASSIGN_DIM || opline->opcode == ZEND_ASSIGN_DIM_OP;
}

uint8_t swr_phpstdext_undefined_cv(zend_execute_data *execute_data) {
    const zend_op *data = EX(opline) + 1;
    if (!swr_phpstdext_assigning(EX(opline)) || data->op1_type != IS_CV) {
        return 0;
    }
    volatile uint8_t bailed = 0;
    zend_try { zval_undefined_cv(data->op1.var EXECUTE_DATA_CC); }
    zend_catch { bailed = 1; } zend_end_try();
    return bailed;
}

uint8_t swr_phpstdext_dimension(zend_execute_data *execute_data, zval *container, zval **result) {
    const zend_op *opline = EX(opline);
    if (!swr_phpstdext_assigning(opline) || Z_TYPE_P(container) != IS_ARRAY) {
        return 0;
    }
    /* Upstream selects W_CONST from the value's OP_DATA type for ASSIGN_DIM_OP. */
    bool constant = opline->opcode == ZEND_ASSIGN_DIM ? opline->op2_type == IS_CONST
                                                      : (opline + 1)->op1_type == IS_CONST;
    /* An append's key is null. */
    const zval *key = opline->op2_type == IS_UNUSED ? &EG(uninitialized_zval)
                      : opline->op2_type == IS_CONST ? RT_CONSTANT(opline, opline->op2)
                                                     : EX_VAR(opline->op2.var);
    volatile uint8_t bailed = 0;
    zend_try {
        SEPARATE_ARRAY(container);
        HashTable *table = Z_ARRVAL_P(container);
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

uint8_t swr_phpstdext_binary(zend_execute_data *execute_data, zval *slot, zval *value) {
    const zend_op *opline = EX(opline);
    if (opline->opcode != ZEND_ASSIGN_DIM_OP) {
        return 0;
    }
    volatile uint8_t bailed = 0;
    zend_try { zend_binary_op(slot, slot, value OPLINE_CC); }
    zend_catch { bailed = 1; } zend_end_try();
    return bailed;
}

uint8_t swr_phpstdext_typed_ref(zend_execute_data *execute_data, zval *element, zval *value, bool *typed) {
    const zend_op *opline = EX(opline);
    *typed = opline->opcode == ZEND_ASSIGN_DIM_OP && Z_ISREF_P(element) &&
             ZEND_REF_HAS_TYPE_SOURCES(Z_REF_P(element));
    if (!*typed) {
        return 0;
    }
    volatile uint8_t bailed = 0;
    zend_try { zend_binary_assign_op_typed_ref(Z_REF_P(element), value OPLINE_CC EXECUTE_DATA_CC); }
    zend_catch { bailed = 1; } zend_end_try();
    return bailed;
}

uint8_t swr_phpstdext_pcre_cache(zend_string *regex, void **result) {
    volatile uint8_t bailed = 0;
    zend_try {
        pcre_cache_entry *entry = pcre_get_compiled_regex_cache(regex);
        if (entry) {
            php_pcre_pce_incref(entry);
        }
        *result = entry;
    }
    zend_catch { bailed = 1; } zend_end_try();
    return bailed;
}

/* SEPARATE_ARRAY of an array zval (anything else is left alone); a table it
 * copies is moved into an allocation with the original definition behind it, a typed
 * array's definition, which its plain copy lacks. */
void swr_phpstdext_separate(zval *array, size_t info_size) {
    if (Z_TYPE_P(array) != IS_ARRAY) {
        return;
    }
    HashTable *shared = Z_ARR_P(array);
    SEPARATE_ARRAY(array);
    HashTable *plain = Z_ARR_P(array);
    if (plain == shared) {
        return;
    }
    HashTable *copy = emalloc(sizeof(HashTable) + info_size);
    memcpy(copy, plain, sizeof(HashTable));
    memcpy(copy + 1, shared + 1, info_size);
    Z_ARR_P(array) = copy;
    efree(plain);
}

/* Upstream's ArrayTypeValue and ArrayTypeInfo, as typed.rs lays them out. */
typedef struct {
    uint8_t value;
    uint8_t key;
    uint16_t offset;
    uint16_t len;
} swr_type_value;

typedef struct {
    swr_type_value this;
    swr_type_value element;
    zend_class_entry *ce;
    uint16_t len;
    char text[];
} swr_type_info;
_Static_assert(sizeof(swr_type_info) == 32 && offsetof(swr_type_info, text) == 26, "typed.rs's TypeInfo");

/* make_typed_array: ZVAL_ARR(made, a new table of `size` elements), with a copy
 * of `header` behind it, then `header->len` bytes of `text` (zeros past its end)
 * and a NUL; the table returned. `made`'s old value is not released. */
HashTable *swr_phpstdext_typed_new(zval *made, uint32_t size, const swr_type_info *header, struct iovec text) {
    size_t len = header->len;
    HashTable *ht = emalloc(sizeof(HashTable) + sizeof(swr_type_info) + len + 1);
    zend_hash_init(ht, size, NULL, ZVAL_PTR_DTOR, 0);
    swr_type_info *info = (swr_type_info *) (ht + 1);
    memcpy(info, header, sizeof(swr_type_info));
    size_t copied = MIN(len, text.iov_len);
    memcpy(info->text, text.iov_base, copied);
    memset(info->text + copied, 0, len + 1 - copied);
    ZVAL_ARR(made, ht);
    return ht;
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
