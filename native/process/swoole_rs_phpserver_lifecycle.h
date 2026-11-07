#ifndef SWOOLE_RS_PHPSERVER_LIFECYCLE_H
#define SWOOLE_RS_PHPSERVER_LIFECYCLE_H
/* Swoole\Server's object state (the Port objects, user processes and callbacks
 * upstream's ServerProperty held), its creation and release, __construct,
 * start() with its pre-start checks, addProcess, addCommand and command
 * (crates/phpserver, lifecycle.rs). C++ keeps the Server, its ports and the C++
 * callbacks it installs, reached through `host`. */
#include <stdbool.h>
#include <stddef.h>
#include <stdint.h>
#ifdef __cplusplus
extern "C" {
#endif
struct _zend_object;
struct _zval_struct;
struct _zend_class_entry;

/* Server facts `fact` answers, by id. */
enum {
    SWR_PHPSERVER_FACT_CACHE_SESSION_ADDR = 14, /* swoole_http_server_can_cache_session_addr() */
};

/* What `install` sets up on the Server in C++ (Rust fills the on* slots itself). */
enum {
    SWR_PHPSERVER_INSTALL_ALLOCATORS = 11, /* the message bus's, and the recv buffers' in base/thread mode */
    SWR_PHPSERVER_INSTALL_ADMIN_COMMANDS = 12,
};

typedef struct {
    int64_t (*fact)(void *server, uint32_t id);
    void (*install)(void *server, uint32_t which);
    void *(*create)(int64_t mode); /* new Server(mode) */
    int (*add_systemd_socket)(void *server);
    /* A new Server\Port object (sw_malloc_zval) bound to `port` and its server. */
    struct _zval_struct *(*port_object)(void *server, void *port);
    /* serv->create(): null, or sw_error after a failure. */
    const char *(*create_server)(void *server);
    /* serv->start() in zend_try: 0 started and returned, -1 failed (*error is the reason),
     * 1 bailed out. */
    int (*start)(void *server, const char **error);
    /* Delete the Server. */
    void (*release)(void *server);
    /* addProcess's worker: -3 bailed out, -2 a thread that is not a user worker, -1 add_worker failed,
     * else the id. */
    int64_t (*add_process)(void *server, struct _zval_struct *process);
    /* json_encode($data) as a new zend_string, or null; in zend_try (*bailed when it bailed out). */
    struct _zend_string *(*json_encode)(struct _zval_struct *data, bool *bailed);
    /* json_decode($text, true) into `out`, or false. */
    void (*json_decode)(const char *text, size_t length, struct _zval_struct *out);
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
