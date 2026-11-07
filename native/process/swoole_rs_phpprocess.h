#ifndef SWOOLE_RS_PHPPROCESS_H
#define SWOOLE_RS_PHPPROCESS_H
/* PHP Swoole\Process (crates/phpprocess). Rust registers the class with its
 * method table, owns its objects, their workers, pipes and message queues, and
 * runs every method, reaching the server, callables, the reactor and signals
 * through `host`. The methods are Rust's zim_swoole_process_* exports. */
#include <stdbool.h>
#include <stdint.h>
#ifdef __cplusplus
extern "C" {
#endif
struct _zend_class_entry;
struct _zend_object;
struct _zval_struct;
struct _zend_array;

typedef struct {
    struct _zend_class_entry *exception;
    struct _zend_class_entry **server; /* swoole_server_ce, registered later */
    int (*construct_guard)(void); /* 0, or 1 master process, 2 async-io threads */
    void (*unconstructed)(void);  /* fatal: "must call constructor first" */
    bool (*worker_share)(void *worker); /* false outside a server worker thread */
    uint32_t (*worker_base)(void);
    int (*reactor_init)(void);
    void (*event_wait)(void);
    struct _zend_object *(*dup_socket)(int fd, bool stream);
    int (*convert_to_fd)(struct _zval_struct *value);
    void *(*callable_create)(struct _zval_struct *callable);
    void (*callable_free)(void *callable);
    void (*callable_defer_free)(void *callable);
    bool (*callable_call)(void *callable, uint32_t argc, struct _zval_struct *argv, bool coroutine);
    bool (*enable_coroutine)(void);
    bool (*main_thread)(void);
    void (*signal_set)(int signo, void (*handler)(int), bool safety);
    bool (*sync_server)(void);
    void (*signal_listen)(void);
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
