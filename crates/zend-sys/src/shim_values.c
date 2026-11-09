/* Value, string, array and object helpers (crates/phpprocess and later families).
 * Stays C (docs/zend.md, What stays C): allocator (OBJ_RELEASE), version
 * (zval_get_long's slow path, zend_is_true's return type) and budget (GC_ADDREF,
 * ZEND_HASH_FOREACH_VAL). */
#include "shim.h"

void swr_zend_object_release(zend_object *object) {
    OBJ_RELEASE(object);
}

void swr_zend_object_addref(zend_object *object) {
    GC_ADDREF(object);
}

void swr_zend_hash_foreach(const HashTable *table, bool (*each)(zval *value, void *context), void *context) {
    zval *value;
    ZEND_HASH_FOREACH_VAL((HashTable *) table, value) {
        if (!each(value, context)) {
            break;
        }
    }
    ZEND_HASH_FOREACH_END();
}

zend_long swr_zval_get_long(const zval *value) {
    return zval_get_long(value);
}

bool swr_zend_is_true(const zval *value) {
    return zend_is_true((zval *) value);
}
