#ifndef SWOOLE_RS_PHPPROCESS_H
#define SWOOLE_RS_PHPPROCESS_H
/* PHP Swoole\Process (crates/phpprocess). C++ keeps the Worker, pipe and
 * message-queue objects; Rust registers the class with its method table, owns
 * its objects and runs every method, reaching
 * that C++ state through `host`. The methods are Rust's zim_swoole_process_*
 * exports. */
#include <stdbool.h>
#include <stddef.h>
#include <stdint.h>
#include <sys/types.h>
#ifdef __cplusplus
extern "C" {
#endif
struct _zend_class_entry;
struct _zend_object;
struct _zval_struct;
struct _zend_array;

/* What the methods read of a Worker. */
typedef struct {
    uint32_t id;
    int32_t pid;
    int32_t pipe; /* pipe_current's fd, or -1 without one */
    bool pipe_object;
    bool queue;
    bool balance; /* msgqueue_mode == SW_MSGQUEUE_BALANCE */
    bool redirect_stdin;
    bool redirect_stdout;
    bool redirect_stderr;
} swr_phpprocess_worker_view;

typedef struct {
    struct _zend_class_entry *exception;
    struct _zend_class_entry **server; /* swoole_server_ce, registered later */
    bool (*cli)(void);
    bool (*display_errors)(void);
    void (*set_last_error)(int error);
    int (*construct_guard)(void); /* 0, or 1 master process, 2 async-io threads */
    void (*unconstructed)(void);  /* fatal: "must call constructor first" */
    void *(*worker_new)(void);
    void (*worker_free)(void *worker);
    bool (*worker_share)(void *worker); /* false outside a server worker thread */
    uint32_t (*worker_base)(void);
    void (*worker_redirect)(void *worker);
    void (*worker_set_id)(void *worker, uint32_t id);
    int (*worker_pipe)(void *worker, int socket_type); /* master fd, or -1 */
    swr_phpprocess_worker_view (*worker_view)(const void *worker);
    int (*queue_use)(void *worker, int32_t key, int64_t mode, bool blocking, int64_t capacity);
    bool (*queue_stat)(void *worker, size_t *num, size_t *bytes);
    bool (*queue_free)(void *worker);
    bool (*queue_push)(void *worker, const void *message, size_t length);
    ssize_t (*queue_pop)(void *worker, void *message, size_t length);
    int (*fork)(void *worker);
    void (*child_enter)(void *worker);
    int (*reactor_init)(void);
    void (*event_wait)(void);
    bool (*event_available)(void);
    void (*event_free)(void);
    ssize_t (*pipe_read)(void *worker, char *buffer, size_t length, bool blocking);
    ssize_t (*pipe_write)(void *worker, const char *data, size_t length, bool blocking);
    int (*pipe_close)(void *worker, int64_t which);
    bool (*pipe_timeout)(void *worker, double seconds);
    bool (*pipe_blocking)(void *worker, bool blocking);
    struct _zend_object *(*dup_socket)(int fd, bool stream);
    int (*convert_to_fd)(struct _zval_struct *value);
    void (*daemon_dup)(int from, int to);
    int (*daemon)(int nochdir, int noclose);
    uint32_t (*cpu_num)(void);
    void *(*callable_create)(struct _zval_struct *callable);
    void (*callable_free)(void *callable);
    void (*callable_defer_free)(void *callable);
    bool (*callable_call)(void *callable, uint32_t argc, struct _zval_struct *argv, bool coroutine);
    bool (*enable_coroutine)(void);
    bool (*main_thread)(void);
    bool (*has_timer)(void);
    uintptr_t (*signal_handler)(int signo);
    void (*signal_set)(int signo, void (*handler)(int), bool safety);
    bool (*sync_server)(void);
    void (*signal_listen)(void);
    void (*signal_listeners)(int delta);
    void (*reset_worker_type)(void);
} swr_v1_phpprocess_host;

struct _zend_class_entry *swr_v1_phpprocess_minit(const swr_v1_phpprocess_host *table,
                                                  int module);
/* The php_swoole_process_* functions Server and Process\Pool call. */
void *swr_v1_phpprocess_worker(const struct _zval_struct *object, bool check);
void swr_v1_phpprocess_set_worker(const struct _zval_struct *object, void *worker, bool enable_coroutine, int pipe_type);
int swr_v1_phpprocess_start(void *worker, struct _zval_struct *object);
void swr_v1_phpprocess_clean(void);
bool swr_v1_phpprocess_signal_isset(int signo);
#ifdef __linux__ /* HAVE_CPU_AFFINITY on the platforms this build supports */
bool swr_v1_phpprocess_array_to_cpu_set(const struct _zval_struct *array, void *set);
void swr_v1_phpprocess_cpu_set_to_array(struct _zval_struct *array, const void *set);
#endif

/* PHP Swoole\Process\Pool: as Process, over a C++ ProcessPool. */
typedef struct {
    bool started;
    bool async;
    uint8_t ipc_mode;
    bool message_bus;
    bool on_message; /* pool->onMessage is set */
    uint32_t worker_num;
} swr_phpprocess_pool_view;

/* The pool's callbacks, as ProcessPool's onStart/onWorkerStart/... take them
 * (pool, worker or message pointers); null leaves one unset. */
typedef struct {
    void (*start)(void *pool);
    void (*shutdown)(void *pool);
    void (*worker_start)(void *pool, void *worker);
    void (*worker_stop)(void *pool, void *worker);
    void (*worker_exit)(void *pool, void *worker);
    void (*message)(void *pool, void *message);
} swr_v1_phpprocess_pool_events;

typedef struct {
    struct _zend_class_entry *exception;
    struct _zend_class_entry **process; /* swoole_process_ce */
    bool (*server_exists)(void);
    int64_t (*worker_id)(void);
    void *(*pool_new)(uint32_t worker_num, int32_t key, int ipc_type, struct _zval_struct *object, bool async);
    void (*pool_free)(void *pool);
    swr_phpprocess_pool_view (*pool_view)(const void *pool);
    struct _zval_struct *(*pool_object)(const void *pool);
    void (*pool_set_async)(void *pool, bool async);
    void (*pool_configure)(void *pool, struct _zend_array *options);
    int (*pool_listen)(void *pool, const char *host, int64_t port, int64_t backlog, bool unix_socket);
    int (*pool_response)(void *pool, const char *data, size_t length);
    bool (*pool_send)(void *pool, int64_t worker_id, const char *data, size_t length);
    uintptr_t (*signal_swap)(int signo, void (*handler)(int));
    int (*pool_protocol)(void *pool, bool message_bus);
    int (*pool_run)(void *pool, const swr_v1_phpprocess_pool_events *events);
    bool (*pool_detach)(void *pool);
    bool (*pool_shutdown)(void *pool);
    void (*pool_signal)(void *pool, void *worker, int signo);
    void (*worker_signals)(bool term, void (*handler)(int));
    void (*enter_worker)(bool coroutine);
    void (*enter_master)(void);
    void (*message_data)(void *pool, void *message, struct _zval_struct *target);
    void (*worker_busy)(bool busy);
    int32_t (*pool_worker_pid)(const void *pool, int64_t worker_id);
    void (*worker_set_pid)(void *worker, int32_t pid);
    void *(*worker_copy)(void *pool, int64_t worker_id, int *pipe);
} swr_v1_phpprocess_pool_host;

void swr_v1_phpprocess_pool_minit(const swr_v1_phpprocess_pool_host *table);
void *swr_v1_phpprocess_pool_current(void);
#ifdef __cplusplus
}
#endif
#endif
