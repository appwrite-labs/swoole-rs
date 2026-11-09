#ifndef SWOOLE_RS_PHPSERVER_H
#define SWOOLE_RS_PHPSERVER_H
/* PHP Swoole\Server\Port (crates/phpserver). C++ keeps the ListenPort and Server
 * it configures; Rust registers the class with its method table, owns its objects and runs every method, reaching
 * that C++ state through `host`. The methods are Rust's
 * zim_swoole_server_port_* exports. The object is laid out as
 * `struct { ListenPort *port; ServerPortProperty property; zend_object std; }`. */
#include <stdbool.h>
#include <stddef.h>
#include <stdint.h>
#ifdef __cplusplus
extern "C" {
#endif
struct _zend_class_entry;
struct _zend_object;
struct _zval_struct;

typedef struct {
    /* E_ERROR "Invalid instance of %s" (does not return). */
    void (*invalid)(struct _zend_object *object);
    /* getSocket() of a bound port. */
    void (*socket)(void *port, struct _zval_struct *return_value);
} swr_v1_phpserver_port_host;

/* php_swoole_server_port_set_ptr and _get_property: the object's ListenPort and ServerPortProperty. */
void swr_v1_phpserver_port_set(struct _zend_object *object, void *port);
void *swr_v1_phpserver_port_object_property(struct _zend_object *object);
/* The object's ListenPort; E_ERROR "Invalid instance of %s" (no return) when unbound. */
void *swr_v1_phpserver_port_port(struct _zend_object *object);
/* Drop the callables and the port's link, as the Server does at shutdown. */
void swr_v1_phpserver_port_deref(struct _zend_object *object);

struct _zend_class_entry *swr_v1_phpserver_port_minit(const swr_v1_phpserver_port_host *table,
                                                      bool not_serializable, bool sockets);
#ifdef __cplusplus
}
#endif
#endif
