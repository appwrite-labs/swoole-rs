#ifndef SWOOLE_RS_PHPTHREAD_H
#define SWOOLE_RS_PHPTHREAD_H
/* PHP Swoole\Thread, Thread\Map, ArrayList, Queue and Barrier (crates/phpthread),
 * ZTS builds only. C++ keeps upstream's method tables, built from the stubs'
 * arginfo, the thread body (TSRM, request startup, the script), the thread
 * resources (Lock, Atomic) and sockets a container may hold, reached through
 * `host`. Rust registers the classes, starts the threads, owns the shared tables,
 * queues and barriers and runs every method: the zim_swoole_thread_* exports. A
 * thread-mode Server passes its workers' arguments as an array handle. */
#include <stdbool.h>
#include <stddef.h>
#include <stdint.h>
#ifdef SW_THREAD /* config.h: ZTS builds with thread support */
#ifdef __cplusplus
extern "C" {
#endif
struct _zend_function_entry;
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
    /* IS_CO_SOCKET, IS_PHP_SOCKET, IS_LOCK, IS_ATOMIC, IS_ATOMIC_LONG, or 0 */
    uint32_t (*object_kind)(struct _zval_struct *object);
    /* The ThreadResource of a Lock, Atomic or Atomic\Long object */
    void *(*resource_cast)(struct _zval_struct *object, uint32_t kind);
    /* A new object of `kind` taking over a reference to `resource` */
    void (*resource_create)(struct _zval_struct *return_value, uint32_t kind, void *resource);
    void (*resource_add_ref)(void *resource);
    void (*resource_del_ref)(void *resource);
    int (*stream_cast)(struct _zval_struct *stream);            /* a dup'd fd, or -1 */
    int (*co_socket_cast)(struct _zval_struct *object, int *type); /* a dup'd fd, or -1 */
    /* a dup'd fd; -1 after throwing "failed to dup socket fd", -2 after "invalid socket fd" */
    int (*php_socket_cast)(struct _zval_struct *object);
    void (*stream_create)(struct _zval_struct *return_value, int64_t fd);
    void (*co_socket_create)(struct _zval_struct *return_value, int64_t fd, int type);
    void (*php_socket_create)(struct _zval_struct *return_value, int64_t fd);
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
} swr_v1_phpthread_host;

void swr_v1_phpthread_minit(const swr_v1_phpthread_host *host);
struct _zend_class_entry *swr_v1_phpthread_map_minit(const struct _zend_function_entry *methods);
struct _zend_class_entry *swr_v1_phpthread_arraylist_minit(const struct _zend_function_entry *methods);
struct _zend_class_entry *swr_v1_phpthread_queue_minit(const struct _zend_function_entry *methods);
struct _zend_class_entry *swr_v1_phpthread_barrier_minit(const struct _zend_function_entry *methods);
struct _zend_class_entry *swr_v1_phpthread_thread_minit(const struct _zend_function_entry *methods);
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
