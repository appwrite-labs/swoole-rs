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
    bool (*sendfile)(void *server, int64_t fd, const char *filename, size_t length, int64_t offset, int64_t size);
    bool (*has_pipe_message)(void *server);
    int64_t (*core_worker_num)(void *server);
    /* send_pipe_message of the packed message (an EventData). */
    bool (*send_message)(void *server, void *message, int64_t worker_id);
} swr_v1_phpserver_io_host;

void swr_v1_phpserver_io_init(const swr_v1_phpserver_io_host *table);
#ifdef __cplusplus
}
#endif
#endif
