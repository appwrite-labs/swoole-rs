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

int swr_zend_try_callable_warning(const char *message) {
    SWR_TRAP(swr_zend_callable_warning(message));
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

/* Whether releasing `value` runs no PHP code (no destructor, no cycle collection), so
 * it needs no trap: an uncounted value, a string, or the last reference to a plain
 * array of those. */
static bool swr_release_runs_no_code(zval *value) {
    if (!Z_REFCOUNTED_P(value) || Z_TYPE_P(value) == IS_STRING) return true;
    if (Z_TYPE_P(value) != IS_ARRAY || Z_REFCOUNT_P(value) != 1 || Z_ARRVAL_P(value)->pDestructor != ZVAL_PTR_DTOR)
        return false;
    zval *item;
    ZEND_HASH_FOREACH_VAL(Z_ARRVAL_P(value), item) {
        if (Z_REFCOUNTED_P(item) && Z_TYPE_P(item) != IS_STRING) return false;
    } ZEND_HASH_FOREACH_END();
    return true;
}

int swr_zend_try_ptr_dtor(zval *value) {
    /* A pending thread exit makes the trap report a bailout, as before. */
    if (!SWR_ZEND_EXIT_PENDING() && swr_release_runs_no_code(value)) {
        zval_ptr_dtor(value);
        return 0;
    }
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

int swr_zend_try_hash_index_del(HashTable *table, zend_ulong index) {
    SWR_TRAP(zend_hash_index_del(table, index));
}

int swr_zend_try_hash_clean(zval *array) {
    if (Z_TYPE_P(array) != IS_ARRAY) {
        return 0;
    }
    SEPARATE_ARRAY(array);
    SWR_TRAP(zend_hash_clean(Z_ARRVAL_P(array)));
}

int swr_lent_symtable_update(zval *array, struct iovec name, zval *value) {
    /* add_assoc_zval_ex; as above, only a key already there needs the trap.
     * ZEND_HANDLE_NUMERIC_STR reads a first byte even of an empty key. */
    const char *key = name.iov_len ? name.iov_base : "";
    size_t length = name.iov_len;
    zend_ulong index;
    HashTable *table = Z_ARRVAL_P(array);
    if (ZEND_HANDLE_NUMERIC_STR(key, length, index)) {
        if (zend_hash_index_add(table, index, value)) return 0;
        SWR_TRAP(zend_hash_index_update(table, index, value));
    }
    if (zend_hash_str_add(table, key, length, value)) return 0;
    SWR_TRAP(zend_hash_str_update(table, key, length, value));
}
