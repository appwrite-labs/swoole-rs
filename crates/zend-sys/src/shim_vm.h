/* The executor's object store and resource list, for swoole_get_vm_status()
 * and swoole_get_objects() (crates/phpswoole). */
#ifndef SWOOLE_RS_ZEND_SHIM_VM_H
#define SWOOLE_RS_ZEND_SHIM_VM_H
#include "php.h"

/* EG(objects_store).top. */
uint32_t swr_zend_objects_top(void);
/* EG(objects_store).object_buckets[handle] when it holds a live object with
 * class-name handlers (Swoole's object_valid), else NULL; handle < top. */
zend_object *swr_zend_objects_get(uint32_t handle);
/* ZVAL_OBJ_COPY(out, EG(objects_store).object_buckets[handle]) when the store
 * holds an object there with these handlers whose free_obj has not begun;
 * false, with `out` untouched, otherwise. */
/* The live object of `handlers` at `handle`, or NULL; no reference is taken. */
zend_object *swr_zend_objects_find(uint32_t handle, const zend_object_handlers *handlers);
bool swr_zend_objects_hold(uint32_t handle, const zend_object_handlers *handlers, zval *out);
/* zend_array_count(&EG(regular_list)). */
uint32_t swr_zend_resource_count(void);
#endif
