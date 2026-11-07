#ifndef SWOOLE_RS_PHPSERVER_DISPATCH_H
#define SWOOLE_RS_PHPSERVER_DISPATCH_H
/* Server events into PHP callbacks (crates/phpserver, dispatch.rs). The
 * Server's on* slots hold Rust fns, which re-raise a bailout they trapped last
 * (docs/zend.md). What they still ask of C++ is `host`. */
#include <stdbool.h>
#include <stddef.h>
#include <stdint.h>
#include <sys/types.h>
#ifdef __cplusplus
extern "C" {
#endif
struct _zval_struct;

typedef struct {
    struct _zval_struct *(*zserv)(void *server);
    /* php_swoole_server_get_callback(serv, server_fd, type), or null. */
    void *(*port_callback)(void *server, int server_fd, int type);
    void (*recv_data)(void *server, struct _zval_struct *data, void *request);
    void (*close)(void *server, int64_t session_id);
    /* onWorkerStart's native preparation: hooks and the message bus allocator. */
    void (*worker_start)(void *server);
    int64_t (*pid)(void);
} swr_v1_phpserver_dispatch_host;

void swr_v1_phpserver_dispatch_init(const swr_v1_phpserver_dispatch_host *table);
/* A port's package_length_func: Protocol::LengthFunc over the zend::Callable in
 * protocol->private_data_1 (php_swoole_server_length_func). */
ssize_t swr_v1_phpserver_length_func(const void *protocol, void *_socket, const void *packet);
#ifdef __cplusplus
}
#endif
#endif
