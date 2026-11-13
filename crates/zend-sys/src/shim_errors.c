/* Stays C (docs/zend.md, What stays C): bailout (every error runs the user error
 * handler) and variadic (php_error_docref and zend_error). */
#include "shim.h"
#include "shim_errors.h"

/* Run `statement` inside SWR_ZEND_TRY; 1 if it bailed out. */
#define SWR_TRAP(statement)                                                                \
    volatile int bailed = 0;                                                               \
    SWR_ZEND_TRY {                                                                             \
        statement;                                                                         \
    }                                                                                      \
    SWR_ZEND_CATCH {                                                                           \
        bailed = 1;                                                                        \
    }                                                                                      \
    SWR_ZEND_END_TRY();                                                                        \
    return bailed

int swr_zend_try_error(int level, const char *message) {
    SWR_TRAP(php_error_docref(NULL, level, "%s", message));
}

int swr_zend_try_zend_error(int level, const char *message) {
    SWR_TRAP(zend_error(level, "%s", message));
}

void swr_v1_core_set_last_error(int code);

void swr_zend_callable_warning(const char *message) {
    swr_v1_core_set_last_error(503); /* SW_ERROR_PHP_FATAL_ERROR */
    php_error_docref(NULL, E_WARNING, "%s", message);
}

int swr_zend_try_report(void (*report)(const char *message), const char *message) {
    SWR_TRAP(report(message));
}

#define SWR_TRY_PARSE(name, type, slow)                                                    \
    int name(zval *arg, type *out, uint32_t number, bool *parsed) {                      \
        *parsed = false;                                                                   \
        SWR_TRAP(*parsed = slow(arg, out, number));                                        \
    }

SWR_TRY_PARSE(swr_zend_try_parse_long, zend_long, zend_parse_arg_long_slow)
SWR_TRY_PARSE(swr_zend_try_parse_bool, bool, zend_parse_arg_bool_slow)
SWR_TRY_PARSE(swr_zend_try_parse_double, double, zend_parse_arg_double_slow)
SWR_TRY_PARSE(swr_zend_try_parse_str, zend_string *, zend_parse_arg_str_slow)

int swr_zend_try_get_long(zval *value, zend_long *out) {
    *out = 0;
    SWR_TRAP(*out = zval_get_long(value));
}

int swr_zend_try_get_double(zval *value, double *out) {
    *out = 0.0;
    SWR_TRAP(*out = zval_get_double(value));
}

int swr_zend_try_get_string(zval *value, zend_string **out) {
    *out = ZSTR_EMPTY_ALLOC();
    SWR_TRAP(*out = zval_get_string(value));
}

int swr_zend_try_ptr_dtor(zval *value) {
    SWR_TRAP(zval_ptr_dtor(value));
}

int swr_zend_try_hash_update(HashTable *table, zend_string *key, zend_ulong index, zval *value) {
    /* A new key releases nothing, so no PHP code can run and needs no trap. A
     * null key is the integer `index` (zend_hash_index_update). */
    if (key ? zend_hash_add(table, key, value) != NULL
            : zend_hash_index_add(table, index, value) != NULL) {
        return 0;
    }
    if (key) {
        SWR_TRAP(zend_hash_update(table, key, value));
    }
    SWR_TRAP(zend_hash_index_update(table, index, value));
}

int swr_zend_try_hash_clean(HashTable *table) {
    SWR_TRAP(zend_hash_clean(table));
}

int swr_zend_try_symtable_update(zval *array, const char *key, size_t length, zval *value) {
    /* add_assoc_zval_ex; as above, only a key already there needs the trap. */
    zend_ulong index;
    HashTable *table = Z_ARRVAL_P(array);
    if (ZEND_HANDLE_NUMERIC_STR(key, length, index)) {
        if (zend_hash_index_add(table, index, value)) return 0;
        SWR_TRAP(zend_hash_index_update(table, index, value));
    }
    if (zend_hash_str_add(table, key, length, value)) return 0;
    SWR_TRAP(zend_hash_str_update(table, key, length, value));
}
