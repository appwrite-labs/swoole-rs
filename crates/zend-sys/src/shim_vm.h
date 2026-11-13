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
/* ZVAL_OBJ_COPY(out, EG(objects_store).object_buckets[handle]) when the store
 * holds an object there with these handlers whose free_obj has not begun;
 * false, with `out` untouched, otherwise. */
bool swr_zend_objects_hold(uint32_t handle, const zend_object_handlers *handlers, zval *out);
/* zend_array_count(&EG(regular_list)). */
uint32_t swr_zend_resource_count(void);
#endif
