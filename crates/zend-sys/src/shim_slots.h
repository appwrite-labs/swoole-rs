/* Zend's callback slots, filled with Rust handlers that take crates/zend's handle
 * types (a transparent `int` or non-null pointer each, so one ABI with what Zend
 * passes): Rust declares each setter over its handler's own type, so the
 * `swr_slot_` prefix keeps them out of bindgen's allowlist. */
#ifndef SWOOLE_RS_ZEND_SHIM_SLOTS_H
#define SWOOLE_RS_ZEND_SHIM_SLOTS_H
#include "shim.h"

/* `ce->create_object = create`. */
void swr_slot_create_object(zend_class_entry *ce, zend_object *(*create)(zend_class_entry *));
/* `handlers->free_obj`, and `handlers->dtor_obj` unless null. */
void swr_slot_object_free(zend_object_handlers *handlers, zend_object_free_obj_t free_obj,
                          zend_object_dtor_obj_t dtor_obj, zend_object_get_gc_t get_gc);
/* `handlers->unset_property = unset`. */
void swr_slot_unset_property(zend_object_handlers *handlers, zend_object_unset_property_t unset);
/* The module's phase functions. */
void swr_slot_module(zend_module_entry *entry, zend_result (*startup)(int, int),
                     zend_result (*shutdown)(int, int), zend_result (*request_startup)(int, int),
                     zend_result (*request_shutdown)(int, int), void (*info)(zend_module_entry *));
void swr_slot_gc_add(zend_get_gc_buffer *roots, zval *value);
HashTable *swr_slot_gc_properties(zend_object *object, zval **table, int *count,
                                  void (*visit)(zend_object *, zend_get_gc_buffer *));
#endif
