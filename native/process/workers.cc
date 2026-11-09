// SPDX-License-Identifier: Apache-2.0
// What crates/process's task calls need from C++ (the task hooks), and the Worker,
// Factory and ProcessPool layout checks against the records Rust lays out.
#include <sys/uio.h>
#include <sys/mman.h>

#include "swoole_server.h"
#include "swoole_rs_process.h"
#include "swoole_rs_locks.h"
#include "swoole_util.h"
#include "swoole_memory.h"
#include "swoole_rs_memory.h"
#include "swoole_coroutine.h"
#include "swoole_pipe.h"

namespace swoole {

// Rust owns the Worker record layout (crates/process/src/worker_record.rs, mirrored by
// swr_v1_worker_record). Lock swoole::Worker to it so a drift fails this build, not the
// sw_shm-backed worker array at runtime. The scalar process fields migrate behind
// accessors; see #629.
static_assert(sizeof(Worker) == sizeof(swr_v1_worker_record), "Worker layout drift (size)");
// pid and id have no C++ fields — they are the two reserved words before pool; pool's
// offset anchors them. status and type (kind) have no C++ fields either — they are the
// two reserved bytes between id and msgqueue_mode; the size plus the pool and
// msgqueue_mode offsets bracket every reserved field, so layout drift still fails the build.
static_assert(offsetof(Worker, pool) == offsetof(swr_v1_worker_record, pool), "Worker.pool drift");
static_assert(offsetof(Worker, msgqueue_mode) == offsetof(swr_v1_worker_record, msgqueue_mode),
              "Worker.msgqueue_mode drift");

// crates/server/src/factory.rs's record, and crates/process reads its server first.
static_assert(sizeof(Factory) == 2 * sizeof(void *) && offsetof(Factory, manager_) == sizeof(void *),
              "Factory layout");

static_assert(SW_SERVER_EVENT_RECV_DATA == 0 && SW_SERVER_EVENT_RECV_DGRAM == 1 && SW_SERVER_EVENT_SEND_DATA == 2 &&
                  SW_SERVER_EVENT_SEND_FILE == 3 && SW_SERVER_EVENT_CLOSE == 4 && SW_SERVER_EVENT_CONNECT == 5 &&
                  SW_SERVER_EVENT_CLOSE_FORWARD == 7 && SW_SERVER_EVENT_FINISH == 9 &&
                  SW_SERVER_EVENT_PIPE_MESSAGE == 10 && SW_SERVER_EVENT_BUFFER_FULL == 13 &&
                  SW_SERVER_EVENT_BUFFER_EMPTY == 14 && SW_SERVER_EVENT_SHUTDOWN == 16 &&
                  SW_SERVER_EVENT_COMMAND_REQUEST == 17 && SW_SERVER_EVENT_COMMAND_RESPONSE == 18,
              "crates/process/src/workers/ops.rs events");
static_assert(SW_EVENT_WORKER == 2, "crates/process/src/workers/event_worker.rs EVENT_WORKER");

static_assert(SW_WORKER_IDLE == 2, "crates/server/src/task_worker_ops.rs SW_WORKER_IDLE");

// Server::task_pack, task_unpack and task_dump are Rust's (crates/process/src/task_file.rs).
static_assert(sizeof(DataHead) == 40 && offsetof(EventData, data) == sizeof(DataHead) && sizeof(String) == 40,
              "crates/process/src/task_file.rs layouts");

// The task calls run in Rust (crates/process/src/task_sync.rs) over these hooks;
// a PHP bailout inside one (MultiTask's pack/unpack, a signal handler while the
// pipe waits) returns here first and is raised again.
static swr_v1_pool_call *task_call(void *arg) {
    return static_cast<swr_v1_pool_call *>(arg);
}

static Server *task_server_of(void *arg) {
    return static_cast<Server *>(task_call(arg)->pool);
}

static Server::MultiTask *task_mtask(void *arg) {
    return static_cast<Server::MultiTask *>(task_call(arg)->worker);
}

static Pipe *task_pipe(void *arg) {
    return task_server_of(arg)->task_notify_pipe(swoole_get_worker_id());
}

static const swr_v1_task_hooks task_hooks = {
    [](void *arg) {
        auto c = task_call(arg);
        auto pool = task_server_of(arg)->get_task_worker_pool();
        auto task = static_cast<EventData *>(c->data);
        auto dst_worker_id = static_cast<int *>(c->socket);
        c->result = (c->flags ? pool->dispatch_sync(task, dst_worker_id) : pool->dispatch(task, dst_worker_id)) == SW_OK;
    },
    [](void *arg) { task_pipe(arg)->clean(); },
    [](void *arg) { task_pipe(arg)->set_timeout(*static_cast<double *>(task_call(arg)->data)); },
    [](void *arg) { task_call(arg)->result = task_pipe(arg)->read(task_call(arg)->data, task_call(arg)->length); },
    [](void *arg) {
        void *lock = task_server_of(arg)->get_worker(swoole_get_worker_id())->lock;
        task_call(arg)->flags ? swr_v1_locks_lock(lock, Lock::MUTEX, LOCK_EX, -1, 0) : swr_v1_locks_unlock(lock, Lock::MUTEX);
    },
    [](void *arg) {
        auto c = task_call(arg);
        c->result = task_mtask(arg)->pack(c->flags, static_cast<EventData *>(c->data));
    },
    [](void *arg) { task_mtask(arg)->unpack(task_call(arg)->flags, static_cast<EventData *>(task_call(arg)->data)); },
    [](void *arg) { task_mtask(arg)->fail(task_call(arg)->flags); },
    [](void *arg) {
        auto c = task_call(arg);
        auto content = file_get_contents(static_cast<const char *>(c->data));
        c->data = content ? content->str : nullptr;
        c->length = content ? content->length : 0;
        c->socket = content ? new std::shared_ptr<String>(content) : nullptr;
    },
    [](void *arg) { delete static_cast<std::shared_ptr<String> *>(task_call(arg)->socket); },
    [](int kind) {
        if (kind == SWR_TASK_WARN_PACK) {
            swoole_warning("task pack failed");
        } else {
            swoole_warning("failed to dispatch task");
        }
    },
};

static swr_v1_task_server task_server(Server *serv, EventData *result, Server::MultiTask *mtask = nullptr) {
    return {&task_hooks,
            serv,
            mtask,
            const_cast<uint32_t *>(&serv->gs->tasking_num),
            const_cast<int64_t *>(reinterpret_cast<volatile int64_t *>(&serv->gs->task_count)),
            reinterpret_cast<uint8_t *>(result),
            sizeof(EventData)};
}

static_assert(SW_ERROR_TASK_DISPATCH_FAIL == 2002, "crates/process/src/task_sync.rs ERROR_TASK_DISPATCH_FAIL");
static const swr_v1_task_native task_native = {
    []() { return SwooleG.task_tmpfile.c_str(); },
    [](void *string, size_t size) { static_cast<String *>(string)->extend(size); },
    [](void *serv, void *mtask, bool with_result, swr_v1_task_server *out) -> uint16_t {
        auto *server = static_cast<Server *>(serv);
        auto *multi = static_cast<Server::MultiTask *>(mtask);
        *out = task_server(server, with_result ? server->get_task_result() : nullptr, multi);
        return multi ? multi->count : 0;
    },
};
[[maybe_unused]] static const bool task_native_set = (swr_v1_process_task_set_native(&task_native), true);

// ProcessPool's layout is Rust's swr_v1_pool_record (crates/process/src/process_pool.rs).
#define SWR_POOL_FIELD(field, mirror)                                                                                  \
    static_assert(offsetof(ProcessPool, field) == offsetof(swr_v1_pool_record, mirror), #field " drift")
static_assert(sizeof(ProcessPool) == sizeof(swr_v1_pool_record), "ProcessPool layout drift (size)");
SWR_POOL_FIELD(running, running);
SWR_POOL_FIELD(reload_init, reload_init);
SWR_POOL_FIELD(read_message, read_message);
SWR_POOL_FIELD(started, started);
SWR_POOL_FIELD(schedule_by_sysvmsg, schedule_by_sysvmsg);
SWR_POOL_FIELD(async, async);
SWR_POOL_FIELD(ipc_mode, ipc_mode);
SWR_POOL_FIELD(protocol_type_, protocol_type);
SWR_POOL_FIELD(master_pid, master_pid);
SWR_POOL_FIELD(max_wait_time, max_wait_time);
SWR_POOL_FIELD(reload_count, reload_count);
SWR_POOL_FIELD(reload_last_time, reload_last_time);
SWR_POOL_FIELD(type, type);
SWR_POOL_FIELD(start_id, start_id);
SWR_POOL_FIELD(use_msgqueue, use_msgqueue);
SWR_POOL_FIELD(use_socket, use_socket);
SWR_POOL_FIELD(max_packet_size_, max_packet_size);
SWR_POOL_FIELD(msgqueue_key, msgqueue_key);
SWR_POOL_FIELD(worker_num, worker_num);
SWR_POOL_FIELD(max_request, max_request);
SWR_POOL_FIELD(max_request_grace, max_request_grace);
SWR_POOL_FIELD(scheduler_warning, scheduler_warning);
SWR_POOL_FIELD(warning_time, warning_time);
SWR_POOL_FIELD(onStart, on_start);
SWR_POOL_FIELD(onShutdown, on_shutdown);
SWR_POOL_FIELD(onBeforeReload, on_before_reload);
SWR_POOL_FIELD(onAfterReload, on_after_reload);
SWR_POOL_FIELD(onTask, on_task);
SWR_POOL_FIELD(onWorkerStart, on_worker_start);
SWR_POOL_FIELD(onMessage, on_message);
SWR_POOL_FIELD(onWorkerExit, on_worker_exit);
SWR_POOL_FIELD(onWorkerStop, on_worker_stop);
SWR_POOL_FIELD(onWorkerError, on_worker_error);
SWR_POOL_FIELD(onWorkerMessage, on_worker_message);
SWR_POOL_FIELD(onWorkerNotFound, on_worker_not_found);
SWR_POOL_FIELD(main_loop, main_loop);
SWR_POOL_FIELD(round_id, round_id);
SWR_POOL_FIELD(workers, workers);
SWR_POOL_FIELD(pipes, pipes);
SWR_POOL_FIELD(registry_, registry);
SWR_POOL_FIELD(queue, queue);
SWR_POOL_FIELD(ipc_, ipc);
SWR_POOL_FIELD(message_box, message_box);
SWR_POOL_FIELD(message_bus, message_bus);
SWR_POOL_FIELD(reload_task, reload_task);
SWR_POOL_FIELD(ptr, ptr);
#undef SWR_POOL_FIELD
// The constants and layouts crates/process/src/process_pool.rs and crates/server/src/pool_native.rs use.
static_assert(SW_MESSAGE_BOX_SIZE == 65536 && SW_INPUT_BUFFER_SIZE == 2 * 1024 * 1024 && SW_WORKER_MAX_WAIT_TIME == 3 &&
                  SW_IPC_BUFFER_MAX_SIZE == 64 * 1024,
              "pool constants drift");
static_assert(SW_WORKER_MESSAGE_STOP == 1 && offsetof(DataHead, len) == 16 && offsetof(DataHead, type) == 22 &&
                  offsetof(EventData, data) == 40 && sizeof(EventData) == SW_IPC_MAX_SIZE &&
                  offsetof(WorkerStopMessage, pid) == 0 && offsetof(WorkerStopMessage, worker_id) == 4 &&
                  sizeof(RecvData) == 48 && sizeof(ExitStatus) == 8,
              "pool message layout drift");
static_assert(SW_PROTOCOL_TASK == 1 && SW_PROTOCOL_STREAM == 2 && SW_PROTOCOL_MESSAGE == 3 && SW_MASTER == 1 &&
                  SW_MANAGER == 3 && SW_WORKER == 2 && SW_TASK_WORKER == 4 && SW_USER_WORKER == 5,
              "pool enums drift");
}  // namespace swoole
