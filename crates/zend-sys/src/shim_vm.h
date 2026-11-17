/* The executor's object store and resource list, for swoole_get_vm_status()
 * and swoole_get_objects() (crates/phpswoole). */
#ifndef SWOOLE_RS_ZEND_SHIM_VM_H
#define SWOOLE_RS_ZEND_SHIM_VM_H
#include "php.h"

/* EG(objects_store).top. */
uint32_t swr_zend_objects_top(void);
/* Whether EG(objects_store).object_buckets[handle] holds a live object with
 * class-name handlers (Swoole's object_valid); false for a handle at or past the
 * store's top. A non-null `out` then holds it, with a reference of its own
 * (ZVAL_OBJ_COPY), in place of what it held, which is not released; false,
 * with `out` untouched, for an object whose free_obj has begun. */
bool swr_zend_objects_copy(uint32_t handle, zval *out);
/* The object EG(objects_store) holds at `handle`, or NULL for a handle at or
 * past its top, slot 0 (NULL) or a free slot (!IS_OBJ_VALID). No reference is
 * taken. */
zend_object *swr_zend_objects_live(uint32_t handle);
/* zend_array_count(&EG(regular_list)). */
uint32_t swr_zend_resource_count(void);
#endif
