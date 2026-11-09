#ifndef SWOOLE_RS_PROCESS_H
#define SWOOLE_RS_PROCESS_H
#include <stdint.h>
#include <stdbool.h>
#include <stddef.h>
#include "swoole_rs_corebase_worker_registry.h"
#ifdef __cplusplus
extern "C" {
#endif
typedef struct { uint32_t status; int32_t error; } swr_v1_process_affinity_outcome;
#if defined(__linux__) || defined(__FreeBSD__)
swr_v1_process_affinity_outcome swr_v1_process_worker_affinity(uint32_t worker_id, const int32_t *available, int32_t available_len, bool thread);
#endif
typedef struct {
    void *(*current)(void);
    void (*signal_init)(void);
    void (*yield_for)(void *co, double timeout);
    void (*resume)(void *co);
    bool (*timed_out)(void *co);
} swr_v1_wait_coroutines;
typedef struct { int pid; bool timed_out; } swr_v1_waited;
swr_v1_waited swr_v1_wait_pid(int pid, int *status, int options, double timeout, const swr_v1_wait_coroutines *ops);
void swr_v1_wait_reap(int signo, void (*resume)(void *co));
size_t swr_v1_wait_count(void);
#ifdef __linux__
typedef struct { uint32_t status; int fd; int32_t error; } swr_v1_signalfd_created;
typedef struct { uint32_t status; uint32_t signo; uintptr_t handler; int32_t error; } swr_v1_signalfd_received;
int swr_v1_signalfd_fd(void);
bool swr_v1_signalfd_owned(void);
uintptr_t swr_v1_signalfd_set(int signo, uintptr_t handler);
swr_v1_signalfd_created swr_v1_signalfd_create(void);
void swr_v1_signalfd_close(void);
bool swr_v1_signalfd_clear(void);
swr_v1_signalfd_received swr_v1_signalfd_receive(int fd);
#endif
/* Byte-exact mirror of swoole::Worker (swoole_rs_process_pool.h), whose layout
 * Rust owns (crates/process/src/worker_record.rs). A static_assert in the process
 * patch locks sizeof/offsetof of swoole::Worker to this mirror, so a layout drift
 * fails the build rather than corrupting shared memory. The scalar process fields
 * (pid/status/type/id) migrate behind the accessors; pointer/atomic fields stay
 * C++-owned storage. See #629. */
typedef struct swr_v1_worker_record {
    int pid;
    uint32_t id;
    void *pool;
    void *queue;
    bool shared;
    bool redirect_stdout;
    bool redirect_stdin;
    bool redirect_stderr;
    uint8_t status;
    uint8_t kind;
    uint8_t msgqueue_mode;
    uint8_t child_process;
    uint32_t concurrency;
    int64_t start_time;
    int64_t dispatch_count;
    int64_t request_count;
    int64_t response_count;
    size_t coroutine_num;
    void *lock;
    void *pipe_object;
    void *pipe_master;
    void *pipe_worker;
    void *pipe_current;
    void *ptr;
} swr_v1_worker_record;
void *swr_v1_worker_globals(void);
void swr_v1_worker_global_max_requests(uint32_t max_request, uint32_t grace, int (*random)(int,int));
void swr_v1_worker_global_detach(swr_v1_worker_record *worker);
void swr_v1_worker_global_stopped(void);
typedef struct swr_v1_process_reload_task swr_v1_process_reload_task;
#define SWR_RELOAD_WARN_KILL_ALL_FAILED 1
#define SWR_RELOAD_WARN_FORCE_KILL 2
#define SWR_RELOAD_WARN_KILL_ONE_FAILED 3
typedef void (*swr_v1_process_reload_warn)(int kind, int pid, uint32_t worker_id);
swr_v1_process_reload_task *swr_v1_process_reload_task_new(void);
void *swr_v1_process_reload_task_free(swr_v1_process_reload_task *);
void swr_v1_process_reload_task_set_timer(swr_v1_process_reload_task *, void *timer);
void swr_v1_process_reload_task_add_workers(swr_v1_process_reload_task *, const swr_v1_worker_record *workers, size_t count);
bool swr_v1_process_reload_task_remove(swr_v1_process_reload_task *, int pid);
bool swr_v1_process_reload_task_completed(const swr_v1_process_reload_task *);
void swr_v1_process_reload_task_kill_all(swr_v1_process_reload_task *, int signo, swr_v1_process_reload_warn warn);
void swr_v1_process_reload_task_kill_one(swr_v1_process_reload_task *, int signo, swr_v1_process_reload_warn warn);
/* The manager reap loop (crates/process/src/reaper.rs); start_reload returns one of: */
#define SWR_RELOAD_NONE 0
#define SWR_RELOAD_KILL_ONE 1
#define SWR_RELOAD_SKIP 2
#define SWR_RELOAD_SIGNALLED 3
typedef struct {
    void *ctx;
    bool leave_when_stopped;
    bool skip_no_child;
    bool (*running)(void *ctx);
    bool (*reload_requested)(void *ctx);
    swr_v1_process_reload_task *(*reload_task)(void *ctx);
    int (*wait)(void *ctx, bool nonblock, int *status, int *error);
    void (*service)(void *ctx);
    void (*wait_failed)(void *ctx);
    uint32_t (*start_reload)(void *ctx);
    bool (*reap)(void *ctx, int pid, int status);
    void (*finish_reload)(void *ctx);
    swr_v1_process_reload_warn warn;
} swr_v1_process_reap_host;
void swr_v1_process_reap_loop(const swr_v1_process_reap_host *host);
/* ProcessPool's IPC (crates/process/src/pool.rs): the stream connection a task
 * worker replies on. */
typedef struct swr_v1_process_pool_ipc swr_v1_process_pool_ipc;
void *swr_v1_process_pool_ipc_connection(const swr_v1_process_pool_ipc *ipc);
/* ProcessPool's layout (crates/process/src/process_pool.rs), which the process
 * patch static_asserts; crates/server/src/pool_native.rs holds its objects. */
typedef struct {
    bool running, reload_init, read_message, started, schedule_by_sysvmsg, async;
    uint8_t ipc_mode;
    int protocol_type;
    int master_pid;
    uint32_t max_wait_time;
    uint64_t reload_count;
    int64_t reload_last_time;
    uint8_t type;
    uint16_t start_id;
    uint8_t use_msgqueue, use_socket;
    uint32_t max_packet_size;
    int msgqueue_key;
    uint32_t worker_num, max_request, max_request_grace;
    uint8_t scheduler_warning;
    int64_t warning_time;
    void *on_start, *on_shutdown, *on_before_reload, *on_after_reload, *on_task, *on_worker_start, *on_message,
        *on_worker_exit, *on_worker_stop, *on_worker_error, *on_worker_message, *on_worker_not_found, *main_loop;
    uint32_t round_id;
    void *workers, *pipes, *registry, *queue, *ipc, *message_box, *message_bus, *reload_task, *ptr;
} swr_v1_pool_record;
/* The reap loop's wait: wait(), or waitpid(-1, WNOHANG) when `nonblock`. */
int swr_v1_process_reap_wait(void *_ctx, bool nonblock, int *status, int *error);
#ifdef __cplusplus
}
#endif
#endif
