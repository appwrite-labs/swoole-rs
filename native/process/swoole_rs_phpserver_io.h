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

typedef struct {
    /* !serv->is_worker_thread() && serv->is_started() */
    bool (*closed)(void *server);
    /* add_port (get_port in a worker thread) and its Port object into return_value; false if none. */
    bool (*listen)(struct _zend_object *object, const char *host, int64_t port, int64_t type,
                   struct _zval_struct *return_value);
    /* A unix datagram send to `path` when the server has one: -1 none, else the result. */
    int (*send_unix)(void *server, const char *path, const char *data, size_t length, int64_t server_socket);
    /* sendto on the UDP (1), UDP6 (2) or unix datagram (3) socket: -1 no such listener, else 0/1. */
    int (*sendto)(void *server, int kind, const char *address, int64_t port, const char *data, size_t length,
                  int64_t server_socket);
} swr_v1_phpserver_io_host;

void swr_v1_phpserver_io_init(const swr_v1_phpserver_io_host *table);
#ifdef __cplusplus
}
#endif
#endif
