#ifndef SWOOLE_RS_PHPSERVER_SERVER_H
#define SWOOLE_RS_PHPSERVER_SERVER_H
/* PHP Swoole\Server and its data classes (crates/phpserver, server.rs). Rust
 * registers the classes with their method tables, creates their objects and runs the
 * methods it owns; C++ keeps the Server the objects wrap, reached through `host`, and
 * reads Server\Task objects as ServerTaskObject. */
#include <stdbool.h>
#include <stddef.h>
#include <stdint.h>
#ifdef __cplusplus
extern "C" {
#endif
struct _zend_class_entry;
struct _zend_object;
struct _zval_struct;
struct _zend_object_handlers;

/* Server::stats(): SWR_PHPSERVER_STATS_* index the values. */
enum {
    SWR_PHPSERVER_STATS_WORKER = 19,     /* sw_worker() set: the next four follow */
    SWR_PHPSERVER_STATS_QUEUE = 24,      /* task queue stat succeeded: the next two follow */
    SWR_PHPSERVER_STATS_TASKS = 27,      /* task_worker_num > 0: the next three follow */
    SWR_PHPSERVER_STATS_COUNT = 33,
};

/* getClientInfo(): what Rust reads of a verified connection. */
typedef struct {
    int64_t uid;
    int64_t worker_id;
    int64_t websocket_status;
    int64_t server_port; /* -1: no server socket connection */
    int64_t server_fd;
    int64_t socket_fd;
    int64_t socket_type;
    int64_t remote_port;
    int64_t reactor_id;
    int64_t connect_time;
    int64_t last_time; /* (int) last_recv_time */
    int64_t close_errno;
    int64_t recv_queued_bytes;
    int64_t send_queued_bytes;
    double last_recv_time;
    double last_send_time;
    double last_dispatch_time;
    const char *remote_ip;
    const char *ssl_client_cert; /* null unless set by this worker */
    size_t ssl_client_cert_length;
    bool uid_mode;      /* dispatch_mode == DISPATCH_UIDMOD */
    bool co_conn_lb;    /* dispatch_mode == DISPATCH_CO_CONN_LB */
    bool websocket;     /* the connection's port speaks WebSocket */
} swr_phpserver_client;

typedef struct {
    /* E_ERROR "Invalid instance of %s" (does not return). */
    void (*invalid)(struct _zend_object *object);
    void (*set_last_error)(int code);
    bool (*display_errors)(void);

    /* Facts and operations of the Server `server`. */
    bool (*started)(void *server);
    /* 0 is_worker() || is_task_worker(); 1 is_process_mode() || is_task_worker() */
    bool (*role)(void *server, int which);
    int64_t (*worker_id)(void);
    int64_t (*pid)(void *server, int which); /* 0 master, 1 manager */
    bool (*worker)(void *server, int64_t worker_id, int64_t *pid, int64_t *status);
    bool (*close)(void *server, int64_t session_id, bool reset);
    /* -1: no such session; else the feedback's result */
    int (*feedback)(void *server, int64_t session_id, bool pause);
    /* -1: no such session; 0 closed or closing; 1 open */
    int (*exists)(void *server, int64_t session_id);
    bool (*protect)(void *server, int64_t session_id, bool value);
    /* -1: no such session; 0 already bound; 1 bound now */
    int (*bind)(void *server, int64_t session_id, uint32_t uid);
    bool (*client)(void *server, int64_t session_id, swr_phpserver_client *client);
    bool (*sendwait)(void *server, int64_t session_id, const char *data, size_t length);
    bool (*reload)(void *server, bool all);
    bool (*shutdown)(void *server);
    bool (*kill_worker)(void *server, int64_t worker_id);
    void (*stats)(void *server, int64_t *values);
    int64_t (*heartbeat_interval)(void *server);
    /* Each unhealthy session, closed first when `close`. */
    void (*heartbeat)(void *server, bool close, void (*each)(void *context, int64_t session_id), void *context);
    void (*socket)(void *server, int64_t port, struct _zval_struct *return_value);

    /* Connections by fd, for getClientList and the iterator. */
    int (*min_fd)(void *server);
    int (*max_fd)(void *server);
    int (*session_fd)(void *server, int64_t session_id); /* -1: no such session */
    /* A connection at `fd`: its session and server fd. */
    bool (*connection_at)(void *server, int fd, int64_t *session_id, int *server_fd);
    int (*port_fd)(void *port);
    int64_t (*connection_num)(void *server, void *port); /* port may be null */
    /* exists()/getClientInfo() called on the server object, as a method
     * (0 exists, 1 getClientInfo); true if a bailout was trapped. */
    bool (*call)(void *server, int which, struct _zval_struct *argument, struct _zval_struct *retval);
} swr_v1_phpserver_host;

typedef struct {
    struct _zend_class_entry *server;
    struct _zend_class_entry *task;
    struct _zend_class_entry *event;
    struct _zend_class_entry *packet;
    struct _zend_class_entry *pipe_message;
    struct _zend_class_entry *status_info;
    struct _zend_class_entry *task_result;
    struct _zend_class_entry *connection_iterator;
    const struct _zend_object_handlers *server_handlers;
    const struct _zend_object_handlers *task_handlers;
} swr_phpserver_classes;

/* MINIT: register the classes, properties and constants. */
void swr_v1_phpserver_minit(const swr_v1_phpserver_host *table,
                            bool thread,
                            bool sockets,
                            int module,
                            swr_phpserver_classes *classes);
#ifdef __cplusplus
}
#endif
#endif
