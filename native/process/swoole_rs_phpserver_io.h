#ifndef SWOOLE_RS_PHPSERVER_IO_H
#define SWOOLE_RS_PHPSERVER_IO_H
/* Swoole\Server's on/getCallback/listen, its send family, sendMessage and
 * finish (crates/phpserver, io.rs). C++ keeps the Server, its callbacks and
 * ports and the task encoding, reached through `host`. */
#include <stdbool.h>
#include <stddef.h>
#include <stdint.h>
#ifdef __cplusplus
extern "C" {
#endif
struct _zend_object;
struct _zval_struct;
struct _zend_string;

typedef struct {
    /* !serv->is_worker_thread() && serv->is_started() */
    bool (*closed)(void *server);
    /* The primary port's on()/getCallback() (0/1), called as a PHP method into `retval`;
     * true if a bailout was trapped. */
    bool (*port_call)(struct _zend_object *object, int which, struct _zval_struct *name, struct _zval_struct *callable,
                      struct _zval_struct *retval);
    /* add_port (get_port in a worker thread) and its Port object into return_value; false if none. */
    bool (*listen)(struct _zend_object *object, const char *host, int64_t port, int64_t type,
                   struct _zval_struct *return_value);
    /* A unix datagram send to `path` when the server has one: -1 none, else the result. */
    int (*send_unix)(void *server, const char *path, const char *data, size_t length, int64_t server_socket);
    /* serv->send(), then the send_yield wait or the kernel-nobufs pause. */
    bool (*send)(void *server, int64_t fd, struct _zend_string *data);
    /* sendto on the UDP (1), UDP6 (2) or unix datagram (3) socket: -1 no such listener, else 0/1. */
    int (*sendto)(void *server, int kind, const char *address, int64_t port, const char *data, size_t length,
                  int64_t server_socket);
    bool (*is_master)(void *server);
    bool (*sendfile)(void *server, int64_t fd, const char *filename, size_t length, int64_t offset, int64_t size);
    bool (*has_pipe_message)(void *server);
    bool (*is_worker)(void *server); /* is_worker() || is_task_worker() */
    int64_t (*worker_id)(void);
    int64_t (*core_worker_num)(void *server);
    /* php_swoole_server_task_pack and send_pipe_message: -1 pack failed, else 0/1. */
    int (*send_message)(void *server, struct _zval_struct *message, int64_t worker_id);
    bool (*task_enable_coroutine)(void *server);
    /* php_swoole_server_task_finish with the task's info, or none. */
    bool (*finish)(void *server, struct _zval_struct *data, struct _zend_object *task);
    /* A Server\Task's server; E_ERROR (does not return) when unset. */
    void *(*task_server)(struct _zend_object *task);
    bool (*started)(void *server);
} swr_v1_phpserver_io_host;

void swr_v1_phpserver_io_init(const swr_v1_phpserver_io_host *table);
#ifdef __cplusplus
}
#endif
#endif
