/* Stays C (docs/zend.md, What stays C): globals (EG(objects_store),
 * EG(regular_list)). */
#include "shim_vm.h"

uint32_t swr_zend_objects_top(void) {
    return EG(objects_store).top;
}

zend_object *swr_zend_objects_get(uint32_t handle) {
    zend_object *obj = EG(objects_store).object_buckets[handle];
    if (obj && IS_OBJ_VALID(obj) && obj->handlers && obj->handlers->get_class_name) {
        return obj;
    }
    return NULL;
}

zend_object *swr_zend_objects_find(uint32_t handle, const zend_object_handlers *handlers) {
    if (handle >= EG(objects_store).top) {
        return NULL;
    }
    zend_object *obj = EG(objects_store).object_buckets[handle];
    if (!obj || !IS_OBJ_VALID(obj) || obj->handlers != handlers || (OBJ_FLAGS(obj) & IS_OBJ_FREE_CALLED)) {
        return NULL;
    }
    return obj;
}

bool swr_zend_objects_hold(uint32_t handle, const zend_object_handlers *handlers, zval *out) {
    zend_object *obj = swr_zend_objects_find(handle, handlers);
    if (!obj) {
        return false;
    }
    ZVAL_OBJ_COPY(out, obj);
    return true;
}

uint32_t swr_zend_resource_count(void) {
    return zend_array_count(&EG(regular_list));
}
