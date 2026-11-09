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

uint32_t swr_zend_resource_count(void) {
    return zend_array_count(&EG(regular_list));
}
