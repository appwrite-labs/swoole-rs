// SPDX-License-Identifier: Apache-2.0
// What crates/process's event worker, task worker, reactor process, thread mode and manager
// operations need from C++ (swoole_rs_workers.h): the calls into Server, Reactor,
// MessageBus, Pipe and Thread, the thread factory's threads, and the
// Worker and ProcessPool layout checks.
#include <sys/uio.h>
#include <sys/mman.h>
#include <system_error>

#include "swoole_server.h"
#include "swoole_thread.h"
#include "swoole_rs_process.h"
#include "swoole_rs_workers.h"
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

using network::Socket;


template <int>
struct WorkersNative;

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

// What a crates/process/src/workers/event_worker.rs operation acts on.
template <>
struct WorkersNative<0> {
    Server *serv;
    Worker *worker;
    DataHead *info;
    Reactor *reactor;
    Connection *conn;
    const ExitStatus *status;
    Socket *socket;

    int64_t run(uint32_t operation, uint64_t arg = 0) {
        return swr_v1_workers_run(operation, arg, this, swr_v1_event_worker_call);
    }
};
using WorkerContext = WorkersNative<0>;

static WorkerContext Worker_context(Server *serv, Worker *worker) {
    return WorkerContext{serv, worker, nullptr, sw_reactor(), nullptr, nullptr, nullptr};
}

// What a crates/process/src/workers/reactor_process.rs operation acts on.
template <>
struct WorkersNative<1> {
    Server *serv;
    ProcessPool *pool;
    Worker *worker;
    Reactor *reactor;
    Event *event;
    Connection *conn;
    double now;

    int64_t run(uint32_t operation) {
        return swr_v1_workers_run(operation, 0, this, swr_v1_reactor_process_call);
    }
};
using ReactorProcessContext = WorkersNative<1>;

// What a crates/process/src/workers/task_worker.rs operation acts on: crates/server's
// task_worker_ops.rs.
template <>
struct WorkersNative<2> {
    Server *serv;
    ProcessPool *pool;
    Worker *worker;
    const EventData *task;
    Socket *socket;
    const char *data;
    size_t data_len;
    EventData *packed;
    EventData *buf;
    int file;
    swr_v1_server_command_fn handler;
    PacketPtr packet;

    int64_t run(uint32_t operation, uint64_t arg = 0) {
        return swr_v1_workers_run(operation, arg, this, swr_v1_task_worker_call);
    }
};
using TaskWorkerContext = WorkersNative<2>;

static_assert(sizeof(TaskWorkerContext) == 112 && SW_SERVER_EVENT_FINISH == 9 && SW_WORKER_IDLE == 2,
              "crates/server/src/task_worker_ops.rs layout and constants");

// Server::task_pack, task_unpack and task_dump are Rust's (crates/process/src/task_file.rs).
static_assert(sizeof(DataHead) == 40 && offsetof(EventData, data) == sizeof(DataHead) && sizeof(String) == 40,
              "crates/process/src/task_file.rs layouts");

static const swr_v1_thread_ops thread_ops = {
    []() -> void * {
        try { return new std::shared_ptr<Thread>(std::make_shared<Thread>()); }
        catch (...) { return nullptr; }
    },
    [](void *slot) { delete static_cast<std::shared_ptr<Thread> *>(slot); },
    [](void *slot) -> int32_t {
        try {
            auto &thread = *static_cast<std::shared_ptr<Thread> *>(slot);
            if (thread->joinable()) thread->join();
            return 0;
        } catch (const std::system_error &error) { return error.code().value(); }
          catch (...) { return EFAULT; }
    },
};

std::shared_ptr<Thread> &ThreadFactory::thread_at(size_t index) const {
    return *static_cast<std::shared_ptr<Thread> *>(swr_v1_thread_manager_thread(manager_, index));
}

// `new ThreadFactory(serv)`: a factory whose manager holds a thread for each worker and the manager.
static Factory *thread_factory_new(Server *serv) {
    void *manager = swr_v1_thread_manager_new();
    if (!swr_v1_thread_manager_threads_init(manager, serv->get_all_worker_num() + 1, &thread_ops)) {
        swr_v1_thread_manager_free(manager);
        throw std::bad_alloc();
    }
    return static_cast<Factory *>(swr_v1_server_factory_new(serv, manager));
}

// What a crates/process/src/workers/thread.rs operation acts on.
template <>
struct WorkersNative<5> {
    Server *serv;
    ThreadFactory *factory;
    Worker *worker;

    static uint64_t call(void *context, uint32_t op, uint64_t arg);

    int64_t run(uint32_t operation, uint64_t arg = 0) {
        return swr_v1_workers_run(operation, arg, this, call);
    }
};
using ThreadContext = WorkersNative<5>;

uint64_t ThreadContext::call(void *context, uint32_t op, uint64_t arg) {
    auto *ctx = static_cast<ThreadContext *>(context);
    Server *serv = ctx->serv;
    ThreadFactory *factory = ctx->factory;
    Worker *worker = ctx->worker;
    switch (op) {
    case SWR_WORKERS_OP_THREAD_MANAGER:
        return (uintptr_t) factory->manager_;
    case SWR_WORKERS_OP_THREAD_SUPPORTED:
#ifdef SW_THREAD
        return 1;
#else
        return 0;
#endif
    case SWR_WORKERS_LOG_CALLOC_CONNECTIONS:
        swoole_log_as("Server::create_thread_factory",
                      swoole_sys_warning("calloc[2](%d) failed",
                                         static_cast<int>(serv->max_connection * sizeof(Connection))));
        return 0;
    case SWR_WORKERS_OP_NEW_THREAD_FACTORY:
        return (uintptr_t) thread_factory_new(serv);
    case SWR_WORKERS_TRACE_NOTIFY_MANAGER_EXIT:
        swoole_log_as("ThreadFactory::terminate_manager_thread",
                      swoole_trace_log(SW_TRACE_THREAD, "notify manager thread to exit"));
        return 0;
    case SWR_WORKERS_OP_JOIN_THREAD:
        factory->thread_at(arg)->join();
        swoole_log_as("ThreadFactory::wait", swoole_trace_log(SW_TRACE_THREAD, "thread=%d join", (int) arg));
        return 0;
    case SWR_WORKERS_TRACE_MANAGER_EXITED:
        swoole_log_as("ThreadFactory::terminate_manager_thread",
                      swoole_trace_log(SW_TRACE_THREAD, "manager thread is exited"));
        return 0;
    case SWR_WORKERS_TRACE_MANAGER_WAKE:
        swoole_log_as("ThreadFactory::wait",
                      swoole_trace_log(SW_TRACE_THREAD,
                                       "manager thread is waiting for worker exit, queue size: %zu",
                                       (size_t) arg));
        return 0;
    case SWR_WORKERS_OP_SELECT_WORKER:
        ctx->worker = reinterpret_cast<Worker *>(arg);
        swoole_log_as("ThreadFactory::wait",
                      swoole_trace_log(SW_TRACE_THREAD,
                                       "worker(type=%d, tid=%d, id=%d) exit, status=%d",
                                       ctx->worker->get_type(),
                                       ctx->worker->get_pid(),
                                       ctx->worker->get_id(),
                                       swr_v1_worker_status(reinterpret_cast<const swr_v1_worker_record *>(ctx->worker))));
        return 0;
    case SWR_WORKERS_OP_THREAD_EXIT_STATUS:
        return (int64_t) factory->thread_at(worker->get_id())->get_exit_status();
    case SWR_WORKERS_OP_CALL_WORKER_ERROR: {
        ExitStatus exit_status(worker->get_pid(), factory->thread_at(worker->get_id())->get_exit_status() << 8);
        serv->call_worker_error_callback(worker, exit_status);
        swoole_log_as("ThreadFactory::wait",
                      swoole_trace_log(SW_TRACE_THREAD,
                                       "worker(tid=%d, id=%d) abnormal exit, status=%d",
                                       exit_status.get_pid(),
                                       worker->get_id(),
                                       exit_status.get_code()));
        return 0;
    }
    case SWR_WORKERS_OP_SPAWN_THREAD:
        factory->thread_at(arg & UINT32_MAX)->start([serv, factory, arg]() {
            ThreadContext{serv, factory}.run(SWR_WORKERS_RUN_THREAD_BODY, arg);
        });
        return 0;
    case SWR_WORKERS_OP_THREAD_ENTER:
        swoole_thread_init(false);
        swoole_set_thread_id(arg);
        swoole_set_thread_type(Server::THREAD_WORKER);
        swoole_trace_log(SW_TRACE_THREAD, "at_thread_enter=%d", (int) arg);
        return swoole_thread_get_native_id();
    case SWR_WORKERS_OP_THREAD_EXIT:
        swoole_trace_log(SW_TRACE_THREAD, "at_thread_exit=%d", (int) arg);
        swoole_thread_clean(false);
        return 0;
    case SWR_WORKERS_OP_CREATE_MESSAGE_BUS: {
        auto mb = new MessageBus();
        mb->count_ids(&serv->gs->pipe_packet_msg_id);
        mb->set_buffer_size(serv->ipc_max_size);
        mb->set_always_chunked_transfer();
        if (!mb->alloc_buffer()) {
            throw std::bad_alloc();
        }
        serv->init_pipe_sockets(mb);
        SwooleTG.message_bus = mb;
        return 0;
    }
    case SWR_WORKERS_OP_DESTROY_MESSAGE_BUS:
        SwooleTG.message_bus->clear();
        delete SwooleTG.message_bus;
        SwooleTG.message_bus = nullptr;
        return 0;
    case SWR_WORKERS_OP_WORKER_THREAD_START: {
        struct Main {
            ThreadContext context;
            uint64_t arg;
        } main{{serv, factory, worker}, arg};
        swr_v1_workers_thread_start(factory->thread_at(arg & UINT32_MAX).get(), [](void *context) {
            auto *main = static_cast<Main *>(context);
            main->context.run(SWR_WORKERS_RUN_THREAD_MAIN, main->arg);
        }, &main);
        return 0;
    }
    case SWR_WORKERS_OP_REACTOR_THREAD_MAIN:
        Server::reactor_thread_main_loop(serv, static_cast<int>(arg));
        return 0;
    case SWR_WORKERS_OP_SET_MASTER_THREAD_ID:
        SwooleTG.id = sw_reactor()->id = factory->get_master_thread_id();
        return 0;
    case SWR_WORKERS_OP_START_MASTER_THREAD:
        return (int64_t) serv->start_master_thread(sw_reactor());
    default:
        return swr_v1_thread_mode_call(context, op, arg);
    }
}

// What a crates/process/src/workers/manager.rs operation acts on: crates/server's
// manager_ops.rs.
template <>
struct WorkersNative<3> {
    Server *serv;
    Worker *worker;
    const EventData *task;
    swr_v1_server_command_fn handler;
    int wait_status;
    int wait_error;
    EventData msg;
    WorkerStopMessage stop;

    int64_t run(uint32_t operation, uint64_t arg = 0) {
        return swr_v1_workers_run(operation, arg, this, swr_v1_manager_call);
    }
};
using ManagerContext = WorkersNative<3>;
static_assert(offsetof(ManagerContext, msg) == 48 && sizeof(ManagerContext) == 56 + sizeof(EventData),
              "crates/server/src/manager_ops.rs layout");

// The Server methods of the deleted worker-side files are Rust
// (crates/process/src/workers/entry.rs); each enters here with its operation, and
// this builds the context its C++ forward built.
static int64_t workers_enter(uint32_t operation, void *object, void *other, void *extra, uint64_t arg) {
    auto *serv = static_cast<Server *>(object);
    auto *worker = static_cast<Worker *>(other);
    switch (operation) {
    case SWR_WORKERS_RUN_SIGNAL_INIT:
    case SWR_WORKERS_RUN_KILL_WORKER:
    case SWR_WORKERS_RUN_DRAIN_PIPES:
        return Worker_context(serv, sw_worker()).run(operation, arg);
    case SWR_WORKERS_RUN_WORKER_SIGNAL:
        return Worker_context(sw_server(), sw_worker()).run(operation, arg);
    case SWR_WORKERS_RUN_ACCEPT_EVENT: {
        auto ctx = Worker_context(serv, sw_worker());
        ctx.info = static_cast<DataHead *>(extra);
        return ctx.run(operation, arg);
    }
    case SWR_WORKERS_RUN_SEND_TO_REACTOR_THREAD: {
        auto ctx = Worker_context(serv, sw_worker());
        ctx.info = static_cast<DataHead *>(other);
        ctx.socket = serv->get_reactor_pipe_socket(*static_cast<const SessionId *>(extra), ctx.info->reactor_id);
        return ctx.run(operation, arg);
    }
    case SWR_WORKERS_RUN_CALL_WORKER_ERROR: {
        auto ctx = Worker_context(serv, worker);
        ctx.status = static_cast<const ExitStatus *>(extra);
        return ctx.run(operation, arg);
    }
    case SWR_WORKERS_RUN_WORKER_START:
    case SWR_WORKERS_RUN_CALL_WORKER_START:
    case SWR_WORKERS_RUN_CALL_WORKER_STOP:
    case SWR_WORKERS_RUN_STOP_ASYNC:
    case SWR_WORKERS_RUN_CLEAN_CONNECTIONS:
    case SWR_WORKERS_RUN_START_EVENT_WORKER:
        return Worker_context(serv, worker).run(operation, arg);
    case SWR_WORKERS_RUN_CREATE_THREAD_FACTORY:
    case SWR_WORKERS_RUN_DESTROY_THREAD_FACTORY:
        return ThreadContext{serv}.run(operation, arg);
    // A ThreadFactory method's factory, or the server's for a Server method.
    case SWR_WORKERS_RUN_THREAD_FACTORY_START:
    case SWR_WORKERS_RUN_THREAD_MANAGER_WAIT:
    case SWR_WORKERS_RUN_THREAD_RELOAD:
    case SWR_WORKERS_RUN_THREAD_TERMINATE_MANAGER:
    case SWR_WORKERS_RUN_THREAD_ID:
    case SWR_WORKERS_RUN_START_WORKER_THREADS:
    case SWR_WORKERS_RUN_STOP_WORKER_THREADS:
        return ThreadContext{serv,
                             static_cast<ThreadFactory *>(other ? other
                                                        : serv->is_thread_mode() ? serv->factory_ : nullptr)}
            .run(operation, arg);
    case SWR_WORKERS_RUN_INIT_TASK_WORKERS:
        return TaskWorkerContext{serv, serv->get_task_worker_pool()}.run(operation, arg);
    case SWR_WORKERS_RUN_FINISH: {
        const auto *bytes = static_cast<const swr_v1_workers_bytes *>(extra);
        EventData buf;
        TaskWorkerContext ctx{serv, nullptr, nullptr, static_cast<const EventData *>(other)};
        ctx.buf = &buf;
        ctx.data = bytes->data;
        ctx.data_len = bytes->length;
        return ctx.run(operation, arg);
    }
    case SWR_WORKERS_RUN_REACTOR_PROCESS_START:
        return ReactorProcessContext{serv, nullptr, nullptr, nullptr, nullptr, nullptr, 0}.run(operation);
    case SWR_WORKERS_RUN_REACTOR_PROCESS_MAIN: {
        auto *pool = static_cast<ProcessPool *>(object);
        return ReactorProcessContext{static_cast<Server *>(pool->ptr), pool, worker, nullptr, nullptr, nullptr, 0}.run(
            operation);
    }
    case SWR_WORKERS_RUN_START_MANAGER:
    case SWR_WORKERS_RUN_RELOAD:
        return ManagerContext{serv}.run(operation, arg);
    case SWR_WORKERS_RUN_RESTART_WORKER: {
        const auto *status = static_cast<const ExitStatus *>(other);
        return ManagerContext{serv}.run(
            operation, (uint64_t) (uint32_t) status->get_pid() | ((uint64_t) (uint32_t) status->get_status() << 32));
    }
    case SWR_WORKERS_RUN_READ_WORKER_MESSAGE:
        return ManagerContext{static_cast<Server *>(static_cast<ProcessPool *>(object)->ptr),
                              nullptr,
                              static_cast<EventData *>(other)}
            .run(operation, arg);
    default:
        abort();
    }
}
static const bool workers_enter_registered = (swr_v1_workers_set_enter(workers_enter), true);

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
