#ifndef SWOOLE_RS_PROCESS_H
#define SWOOLE_RS_PROCESS_H
#include <stdint.h>
#include <stdbool.h>
#include <stddef.h>
#include "swoole_rs_corebase_worker_registry.h"
#ifdef __cplusplus
extern "C" {
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
typedef struct swr_v1_process_reload_task swr_v1_process_reload_task;
/* ProcessPool's IPC (crates/process/src/pool.rs). */
typedef struct swr_v1_process_pool_ipc swr_v1_process_pool_ipc;
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
#ifdef __cplusplus
}
#endif
#endif
