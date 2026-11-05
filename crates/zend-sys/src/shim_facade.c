#include "shim_facade.h"
zval *swr_zend_try_key_update(HashTable *table, zend_string *key, zend_ulong index, zval *value) {
    zval *result = NULL;
    zend_try {
        result = key ? zend_hash_update(table, key, value) : zend_hash_index_update(table, index, value);
    } zend_catch {
        result = NULL;
    } zend_end_try();
    return result;
}
int swr_zend_try_array_insert(zval *array, zend_ulong index, zval *value, bool append, int *result) {
    int bailed = 0;
    zend_try {
        *result = append ? add_next_index_zval(array, value) : add_index_zval(array, index, value);
    } zend_catch {
        bailed = 1;
    } zend_end_try();
    return bailed;
}
int swr_zend_try_facade_truth(zval *value, bool *result) {
    int bailed = 0;
    zend_try { *result = zend_is_true(value); }
    zend_catch { bailed = 1; } zend_end_try();
    return bailed;
}
int swr_zend_try_facade_callable(zval *value, zend_fcall_info_cache *cache, bool *result) {
    int bailed = 0;
    zend_try { *result = zend_is_callable_at_frame(value, NULL, NULL, 0, cache, NULL); }
    zend_catch { bailed = 1; } zend_end_try();
    return bailed;
}
int swr_zend_try_facade_name(zval *value, zend_string **result) {
    int bailed = 0;
    zend_try { *result = zend_get_callable_name_ex(value, NULL); }
    zend_catch { bailed = 1; } zend_end_try();
    return bailed;
}
uint32_t swr_zval_facade_refcount(const zval *value) { return zval_refcount_p(value); }
