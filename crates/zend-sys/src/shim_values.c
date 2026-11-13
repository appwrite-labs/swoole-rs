/* Object and type helpers. Stays C (docs/zend.md, What stays C): allocator
 * (OBJ_RELEASE); swr_zend_type_name bounds zend_get_type_by_const, which is
 * undefined past the types it names. */
#include "shim.h"

void swr_zend_object_release(zend_object *object) {
    OBJ_RELEASE(object);
}

const char *swr_zend_type_name(uint32_t kind) {
    return kind >= IS_NULL && kind <= IS_RESOURCE ? zend_get_type_by_const((int) kind) : NULL;
}
