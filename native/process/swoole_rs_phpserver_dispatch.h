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

typedef struct {
    /* serv->get_worker_message_bus(): the MessageBus a request's OBJ_PTR payload came from. */
    void *(*message_bus)(void *server);
    /* onWorkerStart's native preparation: hooks and the message bus allocator. */
    void (*worker_start)(void *server);
} swr_v1_phpserver_dispatch_host;

void swr_v1_phpserver_dispatch_init(const swr_v1_phpserver_dispatch_host *table);
#ifdef __cplusplus
}
#endif
#endif
