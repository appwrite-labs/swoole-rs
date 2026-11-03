#ifndef SWOOLE_RS_PHPTHREAD_H
#define SWOOLE_RS_PHPTHREAD_H
/* PHP Swoole\Thread, Thread\Map, ArrayList, Queue and Barrier (crates/phpthread),
 * ZTS builds only. C++ keeps the thread body (TSRM, request startup, the script) and
 * the coroutine and ext/sockets sockets a container may hold, reached through `host`; the Lock and Atomic
 * resources are crates/phplocks'. Rust registers the classes, starts the threads, owns the shared tables,
 * queues and barriers and runs every method: the zim_swoole_thread_* exports. A
 * thread-mode Server passes its workers' arguments as an array handle. */
#include <stdbool.h>
#include <stddef.h>
#include <stdint.h>
#ifdef SW_THREAD /* config.h: ZTS builds with thread support */
#ifdef __cplusplus
extern "C" {
#endif
struct _zend_class_entry;
struct _zend_string;
struct _zend_array;
struct _zval_struct;

typedef struct {
    struct _zend_class_entry *exception; /* swoole_exception_ce */
    /* php_swoole_serialize (a persistent string, or null); *bailed when it bailed out */
    struct _zend_string *(*serialize)(struct _zval_struct *value, bool *bailed);
    /* php_swoole_unserialize into `out`; true when it bailed out */
    bool (*unserialize)(const char *data, size_t length, struct _zval_struct *out);
    /* IS_CO_SOCKET, IS_PHP_SOCKET, or 0 */
    uint32_t (*object_kind)(struct _zval_struct *object);
    int (*co_socket_cast)(struct _zval_struct *object, int *type); /* a dup'd fd, or -1 */
    /* a dup'd fd; -1 after throwing "failed to dup socket fd", -2 after "invalid socket fd" */
    int (*php_socket_cast)(struct _zval_struct *object);
    /* A coroutine socket or ext/sockets Socket over `fd`, which it takes; false on failure */
    bool (*co_socket_create)(struct _zval_struct *return_value, int fd, int type);
    bool (*php_socket_create)(struct _zval_struct *return_value, int fd);
    void (*set_last_error)(int error);
    /* fatal "must call constructor first": Swoole's (ArrayList) or PHP's (Map, Queue) */
    void (*unconstructed)(bool list);
    /* Swoole\Thread's body on the new thread, reporting entry and exit status */
    void (*thread_run)(const char *file, size_t length, void *argv, void *context,
                       void (*entered)(void *context), void (*left)(void *context, int status));
    void (*thread_arguments)(struct _zval_struct *return_value); /* Thread::getArguments() */
    void (*thread_info)(struct _zval_struct *return_value);      /* Thread::getInfo() */
    size_t (*thread_count)(void);                                /* running PHP threads */
    bool (*thread_set_name)(const char *name);
    int64_t (*thread_native_id)(void);
    void (*error)(int level, const char *message);     /* php_swoole_error(level, "%s", message) */
    void (*sys_error)(int level, const char *message); /* php_swoole_sys_error(level, "%s", message) */
    void (*warning)(const char *message); /* Swoole's warning log; the message names its function */
} swr_v1_phpthread_host;

void swr_v1_phpthread_minit(const swr_v1_phpthread_host *host);
struct _zend_class_entry *swr_v1_phpthread_map_minit(void);
struct _zend_class_entry *swr_v1_phpthread_arraylist_minit(void);
struct _zend_class_entry *swr_v1_phpthread_queue_minit(void);
struct _zend_class_entry *swr_v1_phpthread_barrier_minit(void);
struct _zend_class_entry *swr_v1_phpthread_thread_minit(void);
/* The main thread's RINIT keeps its script path and $argv (true if serializing
 * bailed out); a new thread's request starts with them (valid until the main
 * thread's RSHUTDOWN releases them). */
bool swr_v1_phpthread_rinit(const char *path, int argc, struct _zval_struct *argv);
struct _zend_string *swr_v1_phpthread_request(const char **path, int *argc);
void swr_v1_phpthread_rshutdown(void);
/* A thread's STDIN, STDOUT and STDERR constants, opened inside its request's
 * zend_try; their descriptors outlive the request and are closed after it. */
void swr_v1_phpthread_stdio_open(void);
void swr_v1_phpthread_stdio_close(void);
/* A shared table: new, from a PHP array, appended to, copied out as a PHP array,
 * referenced again, released. */
void *swr_v1_phpthread_array_new(void);
void *swr_v1_phpthread_array_from(struct _zend_array *array);
void swr_v1_phpthread_array_append(void *array, struct _zval_struct *value);
void swr_v1_phpthread_array_to_array(void *array, struct _zval_struct *return_value);
void swr_v1_phpthread_array_add_ref(void *array);
void swr_v1_phpthread_array_release(void *array);
#ifdef __cplusplus
}
#endif
#endif
#endif
