#ifndef SWOOLE_RS_PROCESS_H
#define SWOOLE_RS_PROCESS_H
#include <stdint.h>
#include <stdbool.h>
#include <stddef.h>
#ifdef __cplusplus
extern "C" {
#endif
typedef struct { uint32_t status; int32_t error; } swr_v1_process_affinity_outcome;
swr_v1_process_affinity_outcome swr_v1_process_worker_affinity(uint32_t worker_id, const int32_t *available, int32_t available_len, bool thread);
typedef struct { uint32_t status; int32_t error; int32_t fd; size_t read; } swr_v1_process_task_file_read_result;
swr_v1_process_task_file_read_result swr_v1_process_task_file_read(const char *path, uint8_t *buffer, size_t length, bool peek);
/* Pack a task payload into EventData::data: inline when it fits in dest_cap, else a
 * temp file (from `template`, an mkstemp pattern) whose path+length land in dest as
 * a PacketTask. Native builds the header and sets info.len/flag from this. */
typedef struct { bool ok, tmpfile; size_t info_len; } swr_v1_process_task_pack_result;
swr_v1_process_task_pack_result swr_v1_process_task_pack(uint8_t *dest, size_t dest_cap, const uint8_t *data, size_t len, const char *template_);
/* Laid out as swoole::Signal. */
typedef struct { uintptr_t handler; uint16_t signo; bool activated; } swr_v1_signal_slot;
extern swr_v1_signal_slot swr_v1_signals[128];
typedef struct { uint32_t status; uintptr_t handler; } swr_v1_signal_lookup_result;
uintptr_t swr_v1_signal_set_action(int signo, uintptr_t func, bool restart, bool mask);
uintptr_t swr_v1_signal_set(int signo, uintptr_t handler, bool safety, void (*callback)(int), bool (*has_reactor)(void));
void swr_v1_signal_dispatch(void (*callback)(int));
swr_v1_signal_lookup_result swr_v1_signal_lookup(int signo);
bool swr_v1_signal_isset(int signo);
void swr_v1_signal_reset(void);
void swr_v1_signal_clear(void);
void swr_v1_signal_mask_all(bool block, bool *blocking_all);
#if defined(__APPLE__) || defined(__FreeBSD__) || defined(__NetBSD__) || defined(__OpenBSD__) || defined(__DragonFly__)
uintptr_t swr_v1_signal_kqueue_set(int kq, int signo, uintptr_t handler, bool *failed);
void swr_v1_signal_kqueue_clear(int kq, bool available);
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
void swr_v1_signalfd_init(void);
int swr_v1_signalfd_fd(void);
bool swr_v1_signalfd_owned(void);
uintptr_t swr_v1_signalfd_set(int signo, uintptr_t handler);
swr_v1_signalfd_created swr_v1_signalfd_create(void);
void swr_v1_signalfd_close(void);
bool swr_v1_signalfd_clear(void);
swr_v1_signalfd_received swr_v1_signalfd_receive(int fd);
#endif
/* Rust-owned worker lookup: pid -> native Worker* and category. Replaces
 * Server::get_worker_by_pid's event-worker scan and user-map branch and the
 * kill_event_workers / kill_user_workers loops. The handle lives on Server; native
 * registers a worker at spawn, unregisters at reap/detach. Task workers live in the
 * task pool, not here. */
#define SWR_WORKER_EVENT 1
#define SWR_WORKER_USER 2
typedef struct swr_v1_process_worker_registry swr_v1_process_worker_registry;
swr_v1_process_worker_registry *swr_v1_process_worker_registry_new(void);
void swr_v1_process_worker_registry_free(swr_v1_process_worker_registry *);
void swr_v1_process_worker_registry_register(swr_v1_process_worker_registry *, int pid, void *worker, int category);
bool swr_v1_process_worker_registry_unregister(swr_v1_process_worker_registry *, int pid);
void *swr_v1_process_worker_registry_lookup(const swr_v1_process_worker_registry *, int pid);
void swr_v1_process_worker_registry_detach(swr_v1_process_worker_registry *, int pid);
bool swr_v1_process_worker_registry_reap_detached(swr_v1_process_worker_registry *, int pid);
/* Send signo to every worker of category, then wait for each. kill_fn/wait_fn are
 * swoole_kill / swoole_waitpid; warn_fn reports a wait failure. */
void swr_v1_process_worker_registry_terminate(const swr_v1_process_worker_registry *, int category, int signo,
                                              int (*kill_fn)(int pid, int signo),
                                              int (*wait_fn)(int pid, int *status, int options),
                                              void (*warn_fn)(int pid));
/* Byte-exact mirror of swoole::Worker (include/swoole_process_pool.h), whose layout
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
uint8_t swr_v1_worker_status(const swr_v1_worker_record *);
void swr_v1_worker_set_status(swr_v1_worker_record *, uint8_t status);
uint8_t swr_v1_worker_type(const swr_v1_worker_record *);
void swr_v1_worker_set_type(swr_v1_worker_record *, uint8_t kind);
int swr_v1_worker_pid(const swr_v1_worker_record *);
void swr_v1_worker_set_pid(swr_v1_worker_record *, int pid);
uint32_t swr_v1_worker_id(const swr_v1_worker_record *);
void swr_v1_worker_set_id(swr_v1_worker_record *, uint32_t id);
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
/* ProcessPool's IPC (crates/process/src/pool.rs): the stream listener, its
 * connection, the reply and packet buffers, the worker loops and dispatch. */
typedef struct swr_v1_process_pool_ipc swr_v1_process_pool_ipc;
typedef struct {
    void *pool;
    void *worker;
    void *socket;
    void *data;
    size_t length;
    int flags;
    ptrdiff_t result;
    int error;
} swr_v1_pool_call;
typedef void (*swr_v1_pool_step)(void *call);
typedef struct {
    bool (*running)(void *pool, void *worker);
    swr_v1_pool_step end;
    swr_v1_pool_step queue_pop;
    swr_v1_pool_step queue_push;
    swr_v1_pool_step accept;
    swr_v1_pool_step recv;
    swr_v1_pool_step send;
    void (*close)(void *socket);
    swr_v1_pool_step pipe_read;
    swr_v1_pool_step pipe_wait;
    swr_v1_pool_step bus_read;
    swr_v1_pool_step bus_deliver;
    swr_v1_pool_step task;
    swr_v1_pool_step message;
    void (*shutdown)(void *worker);
    swr_v1_pool_step pipe_write;
    void (*warn)(void *pool, void *worker, int kind, int64_t a, int64_t b);
} swr_v1_pool_hooks;
typedef struct {
    const swr_v1_pool_hooks *hooks;
    swr_v1_process_pool_ipc *ipc;
    void *pool;
    void *worker;
    uint32_t worker_id;
    uint8_t ipc_mode;
    bool schedule_by_sysvmsg;
    uint32_t max_packet_size;
    size_t event_size;
} swr_v1_pool_loop;
enum {
    SWR_POOL_WARN_MSGRCV = 1,
    SWR_POOL_WARN_ACCEPT = 2,
    SWR_POOL_WARN_READ = 3,
    SWR_POOL_WARN_BAD_PACKET = 4,
    SWR_POOL_WARN_TASK_FAILED = 5,
    SWR_POOL_WARN_PIPE = 6,
    SWR_POOL_TRACE_POP = 7,
    SWR_POOL_WARN_SEND = 8,
    SWR_POOL_TRACE_PUSH = 9,
};
#define SWR_POOL_BAILED 1
void swr_v1_process_set_bailout(int (*trap)(void (*body)(void *), void *arg), void (*raise)(void));
void swr_v1_process_bailout(void);
swr_v1_process_pool_ipc *swr_v1_process_pool_ipc_new(bool listener);
void swr_v1_process_pool_ipc_free(swr_v1_process_pool_ipc *ipc, void (*close)(void *socket));
bool swr_v1_process_pool_ipc_address(swr_v1_process_pool_ipc *ipc, const char *host, int port);
void swr_v1_process_pool_ipc_set_listener(swr_v1_process_pool_ipc *ipc, void *socket);
void *swr_v1_process_pool_ipc_listener(const swr_v1_process_pool_ipc *ipc);
const char *swr_v1_process_pool_ipc_address_of(const swr_v1_process_pool_ipc *ipc, int *port);
void *swr_v1_process_pool_ipc_connection(const swr_v1_process_pool_ipc *ipc);
bool swr_v1_process_pool_ipc_respond(swr_v1_process_pool_ipc *ipc, const uint8_t *data, uint32_t length);
char *swr_v1_process_pool_ipc_prepare(swr_v1_process_pool_ipc *ipc, uint32_t max_packet_size);
char *swr_v1_process_pool_ipc_packet(swr_v1_process_pool_ipc *ipc);
int swr_v1_process_pool_run(const swr_v1_pool_loop *pool_loop, int protocol);
uint32_t swr_v1_process_pool_schedule(const swr_v1_worker_record *workers, uint32_t count, uint32_t *round_id, uint8_t *warning);
ptrdiff_t swr_v1_process_worker_send(const swr_v1_pool_hooks *hooks, void *pool, void *pipe, bool queue, uint32_t worker_id, const uint8_t *data, size_t length, int flags);
bool swr_v1_process_pool_dispatch(const swr_v1_pool_hooks *hooks, void *pool, swr_v1_worker_record *workers, uint32_t count, uint16_t start_id, uint32_t *round_id, uint8_t *warning, bool schedule_by_sysvmsg, bool queue, int *worker_id, const uint8_t *event, uint32_t size, bool blocking);
/* Server::task and Server::task_sync (crates/process/src/task_sync.rs). Each
 * hook takes a swr_v1_pool_call whose pool is the Server and worker the MultiTask. */
typedef struct {
    swr_v1_pool_step dispatch;
    swr_v1_pool_step pipe_clean;
    swr_v1_pool_step pipe_timeout;
    swr_v1_pool_step pipe_read;
    swr_v1_pool_step lock;
    swr_v1_pool_step pack;
    swr_v1_pool_step unpack;
    swr_v1_pool_step fail;
    swr_v1_pool_step contents;
    swr_v1_pool_step release;
    void (*warn)(int kind);
} swr_v1_task_hooks;
typedef struct {
    const swr_v1_task_hooks *hooks;
    void *server;
    void *mtask;
    uint32_t *tasking_num;
    int64_t *task_count;
    uint8_t *result;
    size_t result_size;
} swr_v1_task_server;
enum { SWR_TASK_WARN_PACK = 1, SWR_TASK_WARN_DISPATCH = 2 };
#define SWR_TASK_NONE_DISPATCHED 2
int swr_v1_process_task(const swr_v1_task_server *task_server, uint8_t *task, int *worker_id, bool blocking, bool *ok);
int swr_v1_process_task_sync(const swr_v1_task_server *task_server, uint8_t *task, int *worker_id, double timeout, bool *ok);
int swr_v1_process_task_sync_many(const swr_v1_task_server *task_server, uint16_t count, double timeout, const char *path, size_t event_size, int *ok);
#ifdef __cplusplus
}
#endif
#endif
