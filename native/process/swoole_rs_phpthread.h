#ifndef SWOOLE_RS_PHPTHREAD_H
#define SWOOLE_RS_PHPTHREAD_H
/* PHP Swoole\Thread, Thread\Map, ArrayList, Queue and Barrier (crates/phpthread),
 * ZTS builds only. Rust registers the classes, starts the threads and runs each
 * thread's request (TSRM, request startup, the script, shutdown), owns the shared
 * tables, queues and barriers and runs every method: the zim_swoole_thread_*
 * exports. The Lock and Atomic resources are crates/phplocks'. A thread-mode
 * Server runs its workers through swr_v1_phpthread_run, passing their arguments
 * as an array handle. */
#include <stdbool.h>
#include <stddef.h>
#include <stdint.h>
#ifdef SW_THREAD /* config.h: ZTS builds with thread support */
#ifdef __cplusplus
extern "C" {
#endif
struct _zend_class_entry;
struct _zend_array;
struct _zval_struct;

/* MINIT: Swoole\Thread with its constants, then Swoole\Thread\Error. */
void swr_v1_phpthread_minit(void);
struct _zend_class_entry *swr_v1_phpthread_map_minit(void);
struct _zend_class_entry *swr_v1_phpthread_arraylist_minit(void);
struct _zend_class_entry *swr_v1_phpthread_queue_minit(void);
struct _zend_class_entry *swr_v1_phpthread_barrier_minit(void);
/* The main thread's RINIT keeps its script path and $argv for new threads; every
 * RSHUTDOWN drops the thread's arguments, and the main thread's releases what
 * RINIT kept (exit(200) while other threads still run). */
void swr_v1_phpthread_rinit(void);
void swr_v1_phpthread_rshutdown(void);
/* exit() in a thread (upstream php_swoole_thread_bailout): unwinds to the nearest
 * native scope, else out of the coroutine, else out of the thread's script. */
void swr_v1_phpthread_bailout(void);
/* A PHP thread's body: `file` as the primary script with `argv` (an array
 * handle the thread releases, or null) as its arguments; entered(context) first,
 * left(context, exit status) once the request is over. */
void swr_v1_phpthread_run(const char *file, size_t length, void *argv, void *context,
                          void (*entered)(void *context), void (*left)(void *context, int status));
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
