#ifndef SWOOLE_RS_PHPSERVER_H
#define SWOOLE_RS_PHPSERVER_H
/* PHP Swoole\Server\Port (crates/phpserver). C++ keeps upstream's method table,
 * built from the stub's arginfo, and the ListenPort and Server it configures;
 * Rust registers the class, owns its objects and runs every method, reaching
 * that C++ state through `host`. The methods are Rust's
 * zim_swoole_server_port_* exports. The object is laid out as
 * `struct { ListenPort *port; ServerPortProperty property; zend_object std; }`. */
#include <stdbool.h>
#include <stdint.h>
#ifdef __cplusplus
extern "C" {
#endif
struct _zend_function_entry;
struct _zend_class_entry;
struct _zend_object;
struct _zval_struct;
struct _zend_array;

typedef struct {
    /* E_ERROR "Invalid instance of %s" (does not return). */
    void (*invalid)(struct _zend_object *object);
    /* php_swoole_server_port_deref: drop the callables and the port's link. */
    void (*deref)(struct _zend_object *object);
    /* !serv->is_worker_thread() && serv->is_started() */
    bool (*started)(void *server);
    /* 0: "can't register event callback function after server started";
     * 1: "unknown event types[%s]" with `name`. */
    void (*warn)(int kind, const char *name);
    void *(*callable_create)(struct _zval_struct *callable);
    void (*callable_free)(void *callable);
    /* Install the Server's C++ handler for a SWR_PORTS_HOOK_* if unset. */
    void (*hook)(void *server, uint32_t hook);
    /* The ListenPort settings and TLS block of set(); false: return false. */
    bool (*settings)(void *port, void *property, struct _zend_array *settings);
    /* Helper::checkOptions and the `setting` merge; true if a bailout was trapped. */
    bool (*finish)(struct _zend_object *object, struct _zval_struct *settings, void *property);
    /* getSocket() of a bound port. */
    void (*socket)(void *port, struct _zval_struct *return_value);
} swr_v1_phpserver_port_host;

struct _zend_class_entry *swr_v1_phpserver_port_minit(const struct _zend_function_entry *methods,
                                                      const swr_v1_phpserver_port_host *host,
                                                      bool not_serializable);
#ifdef __cplusplus
}
#endif
#endif
