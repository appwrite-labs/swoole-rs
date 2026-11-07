#ifndef SWOOLE_RS_PHPSERVER_TASK_H
#define SWOOLE_RS_PHPSERVER_TASK_H
/* Swoole\Server's task family: task, taskwait, taskWaitMulti, taskCo,
 * Swoole\Server\Task::pack/unpack, the onTask/onFinish dispatch and the task
 * encoding (crates/phpserver, task.rs). Rust owns the pending callbacks, the waiting
 * coroutines and the payload (a string, or serialized), packed, sent and finished through
 * crates/process; C++ keeps task_unpack and taskWaitMulti's MultiTask, reached through `host`. A `bool *bailed`
 * reports a trapped bailout (a user __serialize/__unserialize). */
#include <stdbool.h>
#include <stddef.h>
#include <stdint.h>
#ifdef __cplusplus
extern "C" {
#endif
struct _zend_object;
struct _zval_struct;

/* sizeof(EventData) and alignof(EventData) fit Rust's buffer. */
#define SWR_PHPSERVER_EVENT_DATA_SIZE 8192
#define SWR_PHPSERVER_EVENT_DATA_ALIGN 8

typedef struct {
    /* Server::task_unpack of `buf`: its payload and ext_flags. */
    bool (*unpack)(void *buf, const char **data, size_t *length, uint32_t *flags);
    /* taskWaitMulti outside a coroutine: Server::MultiTask over `tasks` into the array `out`. */
    bool (*dispatch_multi)(void *server, struct _zval_struct *tasks, double timeout, struct _zval_struct *out,
                           bool *bailed);
} swr_v1_phpserver_task_host;

void swr_v1_phpserver_task_init(const swr_v1_phpserver_task_host *table);
/* php_swoole_server_task_pack: `data` (a string, or serialized) into `buf`; the task id, or -1;
 * *bailed when it bailed out. */
int64_t swr_v1_phpserver_task_pack(struct _zval_struct *data, void *buf, bool *bailed);
/* php_swoole_server_task_unpack of `buf` into `out`; *bailed when it bailed out. */
bool swr_v1_phpserver_task_unpack(void *buf, struct _zval_struct *out, bool *bailed);

#ifdef __cplusplus
}
#endif
#endif
