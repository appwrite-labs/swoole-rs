#ifndef SWOOLE_RS_PHPSERVER_SERVER_H
#define SWOOLE_RS_PHPSERVER_SERVER_H
/* PHP Swoole\Server and its data classes (crates/phpserver, server.rs). Rust
 * registers the classes with their method tables, creates their objects and runs the
 * methods it owns over the Server's records (crates/server's view); C++ answers what
 * only it can through `host`, and reads Server\Task objects as ServerTaskObject. */
#include <stdbool.h>
#include <stddef.h>
#include <stdint.h>
#ifdef __cplusplus
extern "C" {
#endif
struct _zend_class_entry;
struct _zend_object;
struct _zval_struct;
struct _zend_object_handlers;

typedef struct {
    /* E_ERROR "Invalid instance of %s" (does not return). */
    void (*invalid)(struct _zend_object *object);

    /* Coroutine::count() and Coroutine::get_peak_num(). */
    void (*coroutines)(int64_t *counts);
    void (*socket)(void *server, int64_t port, struct _zval_struct *return_value);
} swr_v1_phpserver_host;

/* php_swoole_server_{get,set}_port_property: per thread in thread mode, else in
 * the port's `ptr` (`slot`). With `set`, stores `property` first. */
void *swr_v1_phpserver_port_property(uint16_t object_id, void **slot, void *property, bool set);

/* MINIT: register the classes, properties and constants; returns Swoole\Server's
 * class and its object handlers (for SW_INIT_CLASS_ENTRY_EX's subclasses). */
struct _zend_class_entry *swr_v1_phpserver_minit(const swr_v1_phpserver_host *table, bool thread, bool sockets,
                                                 int module, const struct _zend_object_handlers **handlers);
#ifdef __cplusplus
}
#endif
#endif
