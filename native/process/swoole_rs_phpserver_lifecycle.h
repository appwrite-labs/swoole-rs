#ifndef SWOOLE_RS_PHPSERVER_LIFECYCLE_H
#define SWOOLE_RS_PHPSERVER_LIFECYCLE_H
/* Swoole\Server's object state (the Port objects, user processes and callbacks
 * upstream's ServerProperty held), its creation and release, __construct,
 * start() with its pre-start checks, addProcess, addCommand and command
 * (crates/phpserver, lifecycle.rs). C++ keeps the zend_try frames, reached through
 * `host`. */
#include <stdbool.h>
#include <stddef.h>
#include <stdint.h>
#ifdef __cplusplus
extern "C" {
#endif
struct _zend_object;
struct _zval_struct;
struct _zend_class_entry;

typedef struct {
    /* serv->start() in zend_try: 0 started and returned, -1 failed (*error is the reason),
     * 1 bailed out. */
    int (*start)(void *server, const char **error);
    /* addProcess's worker: -3 bailed out, -2 a thread that is not a user worker, -1 add_worker failed,
     * else the id. */
    int64_t (*add_process)(void *server, struct _zval_struct *process);
} swr_v1_phpserver_lifecycle_host;

void swr_v1_phpserver_lifecycle_init(const swr_v1_phpserver_lifecycle_host *table);
/* The Server object's create_object and free_obj handlers. */
struct _zend_object *swr_v1_phpserver_create(struct _zend_class_entry *ce);
void swr_v1_phpserver_free(struct _zend_object *object);
/* add_port's Port object for `port`, kept by the Server object `server`. */
struct _zval_struct *swr_v1_phpserver_add_port(struct _zend_object *object, void *port);
/* php_swoole_server_send_yield: 0 false, 1 true, 2 bailed out (re-raise it). */
int swr_v1_phpserver_send_yield(void *server, int64_t session_id, const char *data, size_t length);
/* The Port object at `index` of the Server object, or null. */
struct _zval_struct *swr_v1_phpserver_port(struct _zend_object *object, size_t index);
#ifdef __cplusplus
}
#endif
#endif
