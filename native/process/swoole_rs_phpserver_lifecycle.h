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
    SWR_PHPSERVER_FACT_MODE = 1,
    SWR_PHPSERVER_FACT_STARTED = 2,
    SWR_PHPSERVER_FACT_SHUTDOWN = 3,
    SWR_PHPSERVER_FACT_WORKER_THREAD = 4,
    SWR_PHPSERVER_FACT_MASTER = 6,
    SWR_PHPSERVER_FACT_THREAD_MODE = 7,
    SWR_PHPSERVER_FACT_REACTOR = 8,   /* sw_reactor() != nullptr */
    SWR_PHPSERVER_FACT_ENABLE_COROUTINE = 9,
    SWR_PHPSERVER_FACT_SEND_YIELD = 10,
    SWR_PHPSERVER_FACT_HAS_ON_CLOSE = 11,
    SWR_PHPSERVER_FACT_UNSAFE_EVENTS = 12, /* is_support_unsafe_events() */
    SWR_PHPSERVER_FACT_HASH_DISPATCH = 13,
    SWR_PHPSERVER_FACT_CACHE_SESSION_ADDR = 14, /* swoole_http_server_can_cache_session_addr() */
    SWR_PHPSERVER_FACT_WORKER_NUM = 15,
    SWR_PHPSERVER_FACT_TASK_WORKER_NUM = 16,
    SWR_PHPSERVER_FACT_OUTPUT_BUFFER_SIZE = 17,
    SWR_PHPSERVER_FACT_MAX_CONNECTION = 18,
};

/* C++ callbacks `install` sets on the Server. */
enum {
    SWR_PHPSERVER_INSTALL_CONTROL = 1, /* onStart, onShutdown, onWorkerStart ... onWorkerError */
    SWR_PHPSERVER_INSTALL_TASK = 2,    /* onTask and onFinish */
    SWR_PHPSERVER_INSTALL_PIPE_MESSAGE = 3,
    SWR_PHPSERVER_INSTALL_BUFFER_EMPTY = 4,
    SWR_PHPSERVER_INSTALL_CLOSE = 5,
    SWR_PHPSERVER_INSTALL_HTTP_CLOSE = 6,
    SWR_PHPSERVER_INSTALL_RECEIVE = 7,
    SWR_PHPSERVER_INSTALL_HTTP_RECEIVE = 8,
    SWR_PHPSERVER_INSTALL_REDIS_RECEIVE = 9,
    SWR_PHPSERVER_INSTALL_USER_WORKER_START = 10, /* unless one is set */
    SWR_PHPSERVER_INSTALL_ALLOCATORS = 11,        /* the message bus's, and the recv buffers' in base/thread mode */
    SWR_PHPSERVER_INSTALL_ADMIN_COMMANDS = 12,
};

/* A ListenPort's protocol flags (`set_protocols`, `port_facts`). */
enum {
    SWR_PHPSERVER_PROTOCOL_HTTP = 1,
    SWR_PHPSERVER_PROTOCOL_WEBSOCKET = 2,
    SWR_PHPSERVER_PROTOCOL_HTTP2 = 4,
    SWR_PHPSERVER_PROTOCOL_REDIS = 8,
};

typedef struct {
    void *port; /* ListenPort * */
    uint32_t protocols;
    bool dgram;
    bool stream;
    bool ssl;
    bool ssl_verify_without_ca; /* verify_peer with no client cert file, cafile or capath */
} swr_phpserver_port_facts;

typedef struct {
    const char *host;
    int64_t port;
    int64_t type;
    int64_t fd;
    bool ssl;
} swr_phpserver_port_info;

/* addCommand's handlers: the C++ std::function calls swr_v1_phpserver_command_run, which
 * writes the reply through `reply`. */
typedef void (*swr_phpserver_reply)(void *out, const char *data, size_t length);

typedef struct {
    int64_t (*fact)(void *server, uint32_t id);
    void (*install)(void *server, uint32_t which);
    bool (*cli)(void);
    void (*set_last_error)(int code);
    int (*last_error)(void);
    const char *(*strerror)(int code);
    /* 0: no server yet; 1: this worker thread's server, in *server; 2: not the main thread
     * (SW_THREAD); 3: a server already exists. */
    int (*admit)(void **server);
    void *(*create)(int64_t mode); /* new Server(mode) */
    int (*add_systemd_socket)(void *server);
    void *(*add_port)(void *server, int64_t type, const char *host, int64_t port);
    /* The listening port at `index` of serv->ports, or null. */
    void *(*port_at)(void *server, size_t index);
    void (*port_info)(void *port, swr_phpserver_port_info *info);
    /* A new Server\Port object (sw_malloc_zval) bound to `port` and its server. */
    struct _zval_struct *(*port_object)(void *server, void *port);
    void (*port_facts)(struct _zval_struct *port_object, swr_phpserver_port_facts *facts);
    /* Optionally clear_protocol(), then turn on each SWR_PHPSERVER_PROTOCOL_* flag. */
    void (*set_protocols)(void *port, uint32_t protocols, bool clear);
    /* php_swoole_server_isset_callback(serv, port, type). */
    bool (*isset_callback)(void *server, void *port, int type);
    /* Server\Port::set($setting) in zend_try: true if it bailed out. */
    bool (*port_set)(struct _zval_struct *port_object, struct _zval_struct *setting);
    /* 0: Server, 1: Http\Server, 2: WebSocket\Server, 3: Redis\Server. */
    int (*kind)(struct _zend_object *object);
    /* serv->create(): null, or sw_error after a failure. */
    const char *(*create_server)(void *server);
    /* serv->start() in zend_try: 0 started and returned, -1 failed (*error is the reason),
     * 1 bailed out. */
    int (*start)(void *server, const char **error);
    /* SW_THREAD: a worker thread's start() (settings, coroutine options, the worker loop):
     * 0 not a worker thread, 1 ran, 2 bailed out. */
    int (*thread_worker)(struct _zend_object *object);
    /* SW_THREAD in thread mode: bootstrap, init_arguments and the thread starter, in zend_try;
     * *state is what `thread_release` frees. True if it bailed out. */
    bool (*thread_prepare)(struct _zend_object *object, void **state);
    void (*thread_release)(void *state);
    /* Serialize `setting` for worker threads (SW_THREAD's copy_setting). */
    void (*copy_setting)(void *server, struct _zval_struct *setting);
    /* Release the dispatch_func callable, or (`destroy`) delete the Server. */
    void (*release)(void *server, bool destroy);
    void (*port_release)(struct _zval_struct *port_object);
    void *(*callable_create)(struct _zval_struct *callable);
    void (*callable_free)(void *callable);
    /* Whether `process` is a Swoole\Process. */
    bool (*is_process)(struct _zval_struct *process);
    /* addProcess's worker: -3 bailed out, -2 a thread that is not a user worker, -1 add_worker failed,
     * else the id. */
    int64_t (*add_process)(void *server, struct _zval_struct *process);
    /* serv->add_command with a handler that calls swr_v1_phpserver_command_run. */
    bool (*add_command)(void *server, const char *name, size_t length, int64_t types, void *callable);
    /* json_encode($data) as a new zend_string, or null; in zend_try (*bailed when it bailed out). */
    struct _zend_string *(*json_encode)(struct _zval_struct *data, bool *bailed);
    /* json_decode($text, true) into `out`, or false. */
    void (*json_decode)(const char *text, size_t length, struct _zval_struct *out);
    /* serv->command(...) whose callback calls `done(context, message, length)` and resumes the
     * coroutine it returns, if any. */
    bool (*command)(void *server, int64_t process_id, int64_t process_type, const char *name, size_t name_length,
                    const char *data, size_t length, void *(*done)(void *context, const char *message, size_t length),
                    void *context);
    /* zend::function::call(callable, argc, args, retval, false) in zend_try: 0 failed, 1 ok, 2 bailed out. */
    int (*call)(void *callable, uint32_t argc, struct _zval_struct *args, struct _zval_struct *retval);
    /* \Swoole\Server\Helper::onBeforeStart($server) when the library is enabled, in zend_try:
     * true if it bailed out. */
    bool (*before_start_helper)(struct _zval_struct *server);
    /* Coroutine::get_current_safe() in zend_try (*bailed when it bailed out), is_suspending() and
     * yield(). */
    void *(*co_current)(bool *bailed);
    bool (*co_suspending)(void *co);
    void (*co_yield)(void *co);
    struct _zend_class_entry *(*process_class)(void);
    struct _zend_class_entry *(*exception_class)(void);
} swr_v1_phpserver_lifecycle_host;

void swr_v1_phpserver_lifecycle_init(const swr_v1_phpserver_lifecycle_host *table);
/* The Server object's create_object and free_obj handlers. */
struct _zend_object *swr_v1_phpserver_create(struct _zend_class_entry *ce);
void swr_v1_phpserver_free(struct _zend_object *object);
/* add_port's Port object for `port`, kept by the Server object `server`. */
struct _zval_struct *swr_v1_phpserver_add_port(struct _zend_object *object, void *port);
/* The Server object's user process `index`, or null. */
struct _zval_struct *swr_v1_phpserver_user_process(struct _zend_object *object, size_t index);
/* A command handler's call: the callable's reply, written through `reply`; true if it bailed out. */
bool swr_v1_phpserver_command_run(void *callable, void *server, const char *message, size_t length,
                                  swr_phpserver_reply reply, void *out);
/* The Port object at `index` of the Server object, or null. */
struct _zval_struct *swr_v1_phpserver_port(struct _zend_object *object, size_t index);
#ifdef __cplusplus
}
#endif
#endif
