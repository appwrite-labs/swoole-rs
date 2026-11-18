/* Stays C: version-specific class layout and callback signature conversions (budget). */
#include "shim_slots.h"

/* version: create_object occupies an anonymous union in the PHP class layout. */
void swr_slot_create_object(zend_class_entry *ce, zend_object *(*create)(zend_class_entry *)) {
    ce->create_object = create;
}

/* budget: the typed Rust free and destroy handlers need pointer-signature casts for the Zend slots. */
void swr_slot_object_free(zend_object_handlers *handlers, zend_object_free_obj_t free_obj,
                          zend_object_dtor_obj_t dtor_obj) {
    handlers->free_obj = free_obj;
    if (dtor_obj) {
        handlers->dtor_obj = dtor_obj;
    }
}

/* budget: the Rust handler takes borrowed typed arguments where the Zend slot takes raw pointers. */
void swr_slot_unset_property(zend_object_handlers *handlers, zend_object_unset_property_t unset) {
    handlers->unset_property = unset;
}

/* budget: install typed Rust lifecycle callbacks without transmuting the module function pointers. */
void swr_slot_module(zend_module_entry *entry, zend_result (*startup)(int, int),
                     zend_result (*shutdown)(int, int), zend_result (*request_startup)(int, int),
                     zend_result (*request_shutdown)(int, int), void (*info)(zend_module_entry *)) {
    entry->module_startup_func = startup;
    entry->module_shutdown_func = shutdown;
    entry->request_startup_func = request_startup;
    entry->request_shutdown_func = request_shutdown;
    entry->info_func = info;
}
