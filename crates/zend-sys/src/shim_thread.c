#include "shim_thread.h"

bool swr_zend_array_is_list(const zend_array *array) {
    return zend_array_is_list((zend_array *) array);
}

void swr_zend_hash_foreach_kv(const HashTable *table,
                              void (*each)(zend_string *key, zend_ulong index, zval *value, void *context),
                              void *context) {
    zend_string *key;
    zend_ulong index;
    zval *value;
    ZEND_HASH_FOREACH_KEY_VAL((HashTable *) table, index, key, value) {
        ZVAL_DEREF(value);
        each(key, index, value, context);
    }
    ZEND_HASH_FOREACH_END();
}

bool swr_zend_handle_numeric_str(const char *key, size_t length, zend_ulong *index) {
    return _zend_handle_numeric_str(key, length, index);
}

zend_long swr_zend_dval_to_lval_safe(double value) {
    return zend_dval_to_lval_safe(value);
}

zend_long swr_zend_resource_handle(const zval *value) {
    return Z_RES_HANDLE_P(value);
}

uint8_t swr_zend_is_numeric_string(const char *text, size_t length, zend_long *lval, double *dval) {
    return is_numeric_string(text, length, lval, dval, false);
}

int swr_zend_smart_strcmp(zend_string *a, zend_string *b) {
    return zendi_smart_strcmp(a, b);
}
