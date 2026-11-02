#ifndef SWOOLE_RS_PHPSERVER_DISPATCH_H
#define SWOOLE_RS_PHPSERVER_DISPATCH_H
/* Server events into PHP callbacks (crates/phpserver, dispatch.rs). The
 * Server's handlers stay C++ trampolines: each asks Rust to build the
 * callback's arguments and call it, and calls zend_bailout() when Rust reports
 * a bailout it trapped (docs/zend.md). Rust calls PHP only through `host`,
 * whose calls run inside zend_try. */
#include <stdbool.h>
#include <stddef.h>
#include <stdint.h>
#ifdef __cplusplus
extern "C" {
#endif
struct _zval_struct;

/* What the dispatch reads of the Server, per event. */
typedef struct {
    int64_t master_pid;
    int64_t manager_pid;
    bool event_object;     /* serv->event_object */
    bool enable_coroutine; /* serv->enable_coroutine */
    bool coroutine;        /* serv->is_enable_coroutine() */
    bool enable_library;   /* SWOOLE_G(enable_library) */
    bool task_worker;      /* serv->is_task_worker() */
} swr_phpserver_facts;

/* A datagram's sender, for onPacket. */
typedef struct {
    const char *data;
    size_t length;
    const char *address; /* null: not UDP, UDP6 or a unix datagram */
    int64_t port;        /* -1: none (unix datagram) */
    int64_t server_port; /* -1: no server socket connection */
} swr_phpserver_packet;

typedef struct {
    void (*facts)(void *server, swr_phpserver_facts *facts);
    struct _zval_struct *(*zserv)(void *server);
    /* A server-level callback (php_swoole_server_callback_type), or null. */
    void *(*callback)(void *server, int type);
    /* php_swoole_server_get_callback(serv, server_fd, type), or null. */
    void *(*port_callback)(void *server, int server_fd, int type);
    /* zend::function::call(cb, argc, args, nullptr, coroutine) in zend_try:
     * 0 failed, 1 succeeded, 2 bailed out. */
    int (*call)(void *callback, uint32_t argc, struct _zval_struct *args, bool coroutine);
    /* zend::function::call("\\Swoole\\Server\\Helper::<name>", argc, args) in
     * zend_try; true if it bailed out. */
    bool (*helper)(const char *name, uint32_t argc, struct _zval_struct *args);
    void (*recv_data)(void *server, struct _zval_struct *data, void *request);
    void (*close)(void *server, int64_t session_id);
    /* onWorkerStart's native preparation: hooks and the message bus allocator. */
    void (*worker_start)(void *server);
    int64_t (*pid)(void);
    bool (*display_errors)(void);
    void (*set_last_error)(int code);
} swr_v1_phpserver_dispatch_host;

/* Events: SWR_PHPSERVER_ON_* (the callback's server-level type for lifecycle
 * events). Every swr_v1_phpserver_on* returns true when the trampoline must
 * re-raise a bailout. */
void swr_v1_phpserver_dispatch_init(const swr_v1_phpserver_dispatch_host *table);
/* onReceive: args[0] is the server; Rust fills the rest and returns argc.
 * The trampoline calls the callback itself (no zend_try on the hot path), then
 * hands the result back. */
uint32_t swr_v1_phpserver_receive_args(void *server, void *request, int64_t fd, int64_t reactor_id, double time,
                                       bool event_object, struct _zval_struct *args);
bool swr_v1_phpserver_receive_done(void *server, bool succeeded, int64_t fd, struct _zval_struct *args,
                                   uint32_t argc);
/* onConnect/onClose: `callback` was chosen by C++. */
bool swr_v1_phpserver_on_connection(void *server, void *cb, bool close, int64_t fd, int64_t reactor_id,
                                    double time);
/* onBufferFull (full) / onBufferEmpty. */
bool swr_v1_phpserver_on_buffer(void *server, bool full, int64_t fd, int server_fd);
bool swr_v1_phpserver_on_packet(void *server, const swr_phpserver_packet *packet, int server_fd, double time);
bool swr_v1_phpserver_on_pipe_message(void *server, struct _zval_struct *data, int64_t source_worker_id, double time);
/* onStart, onManagerStart/Stop, onBeforeShutdown, onShutdown, onBeforeReload,
 * onAfterReload (type; worker_id unused) and onWorkerStart/Stop/Exit. */
bool swr_v1_phpserver_on_lifecycle(void *server, int kind, int64_t worker_id);
bool swr_v1_phpserver_on_worker_error(void *server, int64_t worker_id, int64_t pid, int64_t status, int64_t code,
                                      int64_t signal);
#ifdef __cplusplus
}
#endif
#endif
