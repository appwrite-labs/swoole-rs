#ifndef SWOOLE_RS_PHPSERVER_TASK_H
#define SWOOLE_RS_PHPSERVER_TASK_H
/* Swoole\Server's task family: task, taskwait, taskWaitMulti, taskCo,
 * Swoole\Server\Task::pack/unpack, the onTask/onFinish dispatch and the task
 * encoding (crates/phpserver, task.rs). Rust owns the pending callbacks, the waiting
 * coroutines and the payload (a string, or serialized); C++ keeps the Server, its
 * task_pack/task_unpack/finish and the coroutine, reached through `host`. A `bool *bailed`
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
    bool started;
    bool worker;      /* is_worker() */
    bool task_worker; /* is_task_worker() */
    bool task_object; /* task_enable_coroutine || task_object */
    bool task_coroutine;
    bool event_object;
    bool enable_coroutine;
    int64_t task_worker_num;
} swr_phpserver_task_facts;

typedef struct {
    int64_t id;
    int64_t worker_id; /* get_task_src_worker_id */
    double time;
    int64_t flags; /* info.ext_flags */
} swr_phpserver_task_info;

typedef struct {
    void (*facts)(void *server, swr_phpserver_task_facts *facts);
    /* Server::task_pack of the payload into `buf` (an EventData): the task id, or -1 (fd and len reset). */
    int64_t (*pack)(void *buf, const char *data, size_t length);
    /* Server::task_unpack of `buf`: its payload and ext_flags. */
    bool (*unpack)(void *buf, const char **data, size_t *length, uint32_t *flags);
    /* The "unserialize() failed" warning. */
    void (*log_unserialize)(int64_t offset, size_t length);
    /* buf->info.ext_flags |= flags */
    void (*flag)(void *buf, uint32_t flags);
    /* buf->size() */
    size_t (*size)(void *buf);
    void (*info)(void *server, void *buf, swr_phpserver_task_info *info);
    /* serv->task(buf, dst_worker_id) */
    bool (*dispatch)(void *server, void *buf, int *dst_worker_id);
    /* serv->task_sync(buf, dst_worker_id, timeout), then the result unpacked into `out`. */
    bool (*dispatch_wait)(void *server, void *buf, int *dst_worker_id, double timeout, struct _zval_struct *out,
                          bool *bailed);
    /* taskWaitMulti outside a coroutine: Server::MultiTask over `tasks` into the array `out`. */
    bool (*dispatch_multi)(void *server, struct _zval_struct *tasks, double timeout, struct _zval_struct *out,
                           bool *bailed);
    /* gs->tasking_num -= 1 */
    void (*tasking_done)(void *server);
    /* The Swoole\Server\Task object `out` (object_init_ex'd by Rust) gets the server and req's DataHead. */
    void (*bind_task)(struct _zend_object *task, void *server, void *req);
    /* serv->finish(data, length, flags, req) */
    bool (*finish)(void *server, const char *data, size_t length, int flags, void *req);
    /* sw_callable_create(zfn); null if not callable. */
    void *(*callable_create)(struct _zval_struct *callable);
    void (*callable_free)(void *callable);
    /* zend::function::call(cb, argc, args, retval, coroutine) in a zend_try: 0 failed, 1 called, 2 bailout. */
    int (*call)(void *callable, uint32_t argc, struct _zval_struct *args, struct _zval_struct *retval,
                bool coroutine);
    /* Coroutine::get_current_safe(), its fatal error trapped. */
    void *(*co_current)(bool *bailed);
    /* swoole_coroutine_is_in() */
    bool (*co_in)(void);
    /* co->yield_ex(timeout), co->resume() */
    bool (*co_yield)(void *co, double timeout);
    void (*co_resume)(void *co);
    /* The "task[%ld] has expired" warning, as php_swoole_server_onFinish logs it. */
    void (*log_expired)(int64_t task_id);
} swr_v1_phpserver_task_host;

void swr_v1_phpserver_task_init(const swr_v1_phpserver_task_host *table);
/* onTask/onFinish: write the callback's SW_OK/SW_ERR to *result; true if the C++ caller must
 * re-raise a bailout. */
bool swr_v1_phpserver_on_task(void *server, void *req, int *result);
bool swr_v1_phpserver_on_finish(void *server, void *req, int *result);
/* php_swoole_server_task_pack: `data` (a string, or serialized) into `buf`; the task id, or -1;
 * *bailed when it bailed out. */
int64_t swr_v1_phpserver_task_pack(struct _zval_struct *data, void *buf, bool *bailed);
/* php_swoole_server_task_unpack of `buf` into `out`; *bailed when it bailed out. */
bool swr_v1_phpserver_task_unpack(void *buf, struct _zval_struct *out, bool *bailed);

#ifdef __cplusplus
}
#endif
#endif
