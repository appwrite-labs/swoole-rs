#include "shim_slots.h"

void swr_slot_create_object(zend_class_entry *ce, zend_object *(*create)(zend_class_entry *)) {
    ce->create_object = create;
}

void swr_slot_object_free(zend_object_handlers *handlers, zend_object_free_obj_t free_obj,
                          zend_object_dtor_obj_t dtor_obj) {
    handlers->free_obj = free_obj;
    if (dtor_obj) {
        handlers->dtor_obj = dtor_obj;
    }
}

void swr_slot_unset_property(zend_object_handlers *handlers, zend_object_unset_property_t unset) {
    handlers->unset_property = unset;
}

void swr_slot_module(zend_module_entry *entry, zend_result (*startup)(int, int),
                     zend_result (*shutdown)(int, int), zend_result (*request_startup)(int, int),
                     zend_result (*request_shutdown)(int, int), void (*info)(zend_module_entry *)) {
    entry->module_startup_func = startup;
    entry->module_shutdown_func = shutdown;
    entry->request_startup_func = request_startup;
    entry->request_shutdown_func = request_shutdown;
    entry->info_func = info;
}
