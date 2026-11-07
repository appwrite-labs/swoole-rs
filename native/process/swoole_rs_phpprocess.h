#ifndef SWOOLE_RS_PHPPROCESS_H
#define SWOOLE_RS_PHPPROCESS_H
/* PHP Swoole\Process (crates/phpprocess). C++ keeps the Worker and pipe
 * objects; Rust registers the class with its method table, owns its objects,
 * their message queues, and runs every method, reaching that C++ state through
 * `host`. The methods are Rust's zim_swoole_process_* exports. */
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
    bool redirect_stdin;
    bool redirect_stdout;
    bool redirect_stderr;
} swr_phpprocess_worker_view;

typedef struct {
    struct _zend_class_entry *exception;
    struct _zend_class_entry **server; /* swoole_server_ce, registered later */
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

/* PHP Swoole\Process\Pool (crates/phpprocess), over crates/process's ProcessPool. */
void swr_v1_phpprocess_pool_minit(void);
void *swr_v1_phpprocess_pool_current(void);
#ifdef __cplusplus
}
#endif
#endif
