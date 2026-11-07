#ifndef SWOOLE_RS_PHPPROCESS_H
#define SWOOLE_RS_PHPPROCESS_H
/* PHP Swoole\Process (crates/phpprocess). Rust registers the class with its
 * method table (php_swoole_process_minit), owns its objects, their workers,
 * pipes and message queues, and runs every method. */
#include <stdbool.h>
#include <stdint.h>
#ifdef __cplusplus
extern "C" {
#endif
struct _zend_class_entry;
struct _zend_object;
struct _zval_struct;
struct _zend_array;

/* The php_swoole_process_* functions Server and Process\Pool call. */
void *swr_v1_phpprocess_worker(const struct _zval_struct *object, bool check);
void swr_v1_phpprocess_set_worker(const struct _zval_struct *object, void *worker, bool enable_coroutine, int pipe_type);
int swr_v1_phpprocess_start(void *worker, struct _zval_struct *object);
#ifdef __linux__ /* HAVE_CPU_AFFINITY on the platforms this build supports */
bool swr_v1_phpprocess_array_to_cpu_set(const struct _zval_struct *array, void *set);
void swr_v1_phpprocess_cpu_set_to_array(struct _zval_struct *array, const void *set);
#endif

/* PHP Swoole\Process\Pool (crates/phpprocess), over crates/process's ProcessPool. */
void *swr_v1_phpprocess_pool_current(void);
#ifdef __cplusplus
}
#endif
#endif
