// SPDX-License-Identifier: Apache-2.0
// What crates/process's event worker, task worker, reactor process, thread mode, manager
// and factory operations need from C++ (swoole_rs_workers.h): the calls into Server, Reactor,
// MessageBus, Pipe and Thread, the Factory and ThreadFactory methods, the task hooks, and
// the Worker and ProcessPool layout checks.
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
#include "swoole_coroutine.h"
#include "swoole_api.h"
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

// The Server's Rust-owned worker registry (pid -> Worker*).
static inline swr_v1_process_worker_registry *worker_registry_of(const Server *serv) {
    return static_cast<swr_v1_process_worker_registry *>(serv->worker_registry);
}

// Reported by the registry's terminate op when a worker's waitpid fails.
static void event_worker_waitpid_warn(int pid) {
    swoole_log_as("Factory::kill_event_workers", swoole_sys_warning("waitpid(%d) failed", pid));
}

using network::Socket;


template <int>
struct WorkersNative;

// What a crates/process/src/workers/factory.rs operation acts on.
template <>
struct WorkersNative<4> {
    Server *serv;
    Worker *worker;
    SendData *send;
    const ExitStatus *status;
    Connection *conn;
    int fd;

    static uint64_t call(void *context, uint32_t op, uint64_t arg);
};
using FactoryContext = WorkersNative<4>;
// crates/process/src/workers/factory_methods.rs defines the Factory, ProcessFactory and
// Server methods over this context, and reads Factory::server_ after the vptr.
static_assert(sizeof(FactoryContext) == 48 && offsetof(FactoryContext, fd) == 40, "FactoryContext layout");
static_assert(sizeof(Factory) == 2 * sizeof(void *), "Factory layout");
static const bool factory_call_registered = (swr_v1_workers_set_factory_call(FactoryContext::call), true);

uint64_t FactoryContext::call(void *context, uint32_t op, uint64_t arg) {
    auto *ctx = static_cast<FactoryContext *>(context);
    Server *serv = ctx->serv;
    SendData *send = ctx->send;
    switch (op) {
    case SWR_WORKERS_OP_NEW_REACTOR_THREADS:
        // init reactor thread pool
        serv->reactor_threads = new ReactorThread[serv->reactor_num]();
        return 0;
    case SWR_WORKERS_OP_INIT_BARRIERS:
        serv->reactor_thread_barrier.init(false, serv->reactor_num + 1);
        serv->gs->manager_barrier.init(true, 2);
        return 0;
    case SWR_WORKERS_OP_NEW_PROCESS_FACTORY:
        return (uintptr_t) new ProcessFactory(serv);
    case SWR_WORKERS_OP_DELETE_REACTOR_THREADS:
        delete[] serv->reactor_threads;
        return 0;
    case SWR_WORKERS_OP_DESTROY_BARRIERS:
        serv->reactor_thread_barrier.destroy();
        serv->gs->manager_barrier.destroy();
        return 0;
    case SWR_WORKERS_OP_DESTROY_MESSAGE_BOX:
        serv->get_event_worker_pool()->message_box->destroy();
        return 0;
    case SWR_WORKERS_OP_CREATE_WORKER_PIPE: {
        auto _sock = new UnixSocket(true, SOCK_DGRAM);
        if (!_sock->ready()) {
            delete _sock;
            return 0;
        }
        serv->worker_pipes.emplace_back(_sock);
        serv->workers[arg].pipe_master = _sock->get_socket(true);
        serv->workers[arg].pipe_worker = _sock->get_socket(false);
        serv->workers[arg].pipe_object = _sock;
        return 1;
    }
    case SWR_WORKERS_OP_CREATE_PIPE_BUFFERS:
        return (int64_t) serv->create_pipe_buffers();
    case SWR_WORKERS_OP_WRITE_TO_WORKER: {
        Worker *target = serv->get_worker(arg);
        SendData _task;
        memcpy(&_task, send, sizeof(SendData));
        MessageBus *mb = &serv->get_thread(swoole_get_thread_id())->message_bus;
        Socket *sock = mb->get_pipe_socket(target->pipe_master);
        return mb->write(sock, &_task);
    }
    case SWR_WORKERS_OP_WRITE_TO_REACTOR: {
        SendData task;
        memcpy(&task, send, sizeof(SendData));
        task.info.reactor_id = ctx->conn->reactor_id;
        task.info.server_fd = swoole_get_worker_id();
        return serv->message_bus.write(serv->get_reactor_pipe_socket(task.info.fd, task.info.reactor_id), &task);
    }
    case SWR_WORKERS_OP_FINISH_CLOSE:
        return serv->factory_->finish(send);
    default:
        return swr_v1_factory_call(context, op, arg);
    }
}

static_assert(SW_SERVER_EVENT_RECV_DATA == 0 && SW_SERVER_EVENT_RECV_DGRAM == 1 && SW_SERVER_EVENT_SEND_DATA == 2 &&
                  SW_SERVER_EVENT_SEND_FILE == 3 && SW_SERVER_EVENT_CLOSE == 4 && SW_SERVER_EVENT_CONNECT == 5 &&
                  SW_SERVER_EVENT_CLOSE_FORWARD == 7 && SW_SERVER_EVENT_FINISH == 9 &&
                  SW_SERVER_EVENT_PIPE_MESSAGE == 10 && SW_SERVER_EVENT_BUFFER_FULL == 13 &&
                  SW_SERVER_EVENT_BUFFER_EMPTY == 14 && SW_SERVER_EVENT_SHUTDOWN == 16 &&
                  SW_SERVER_EVENT_COMMAND_REQUEST == 17 && SW_SERVER_EVENT_COMMAND_RESPONSE == 18,
              "crates/process/src/workers/ops.rs events");
static_assert(SW_EVENT_WORKER == 2, "crates/process/src/workers/event_worker.rs EVENT_WORKER");

static int Worker_onPipeReceive(Reactor *reactor, Event *event);
static void Worker_reactor_try_to_exit(Reactor *reactor);

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

    static uint64_t call(void *context, uint32_t op, uint64_t arg);
    static uint64_t hot_call(void *context, uint32_t op, uint64_t arg);

    int64_t run(uint32_t operation, uint64_t arg = 0) {
        return swr_v1_workers_run(operation, arg, this, hot_call);
    }
};
using WorkerContext = WorkersNative<0>;

uint64_t WorkerContext::call(void *context, uint32_t op, uint64_t arg) {
    auto *ctx = static_cast<WorkerContext *>(context);
    Server *serv = ctx->serv;
    Worker *worker = ctx->worker;
    DataHead *info = ctx->info;
    Reactor *reactor = ctx->reactor;
    switch (op) {
    case SWR_WORKERS_OP_DO_TASK: {
        RecvData recv_data;
        auto packet = serv->get_worker_message_bus()->get_packet();
        recv_data.info = *info;
        recv_data.info.len = packet.length;
        recv_data.data = packet.data;
        if ((arg ? serv->onPacket : serv->onReceive)(serv, &recv_data) == SW_OK) {
            worker->add_request_count();
            sw_atomic_fetch_add(&serv->gs->request_count, 1);
        }
        return 0;
    }
    case SWR_WORKERS_OP_FREE_CLIENT_CERT: {
        Connection *conn = serv->get_connection_verify_no_ssl(info->fd);
        if (conn && conn->ssl_client_cert && conn->ssl_client_cert_pid == swoole_get_worker_pid()) {
            delete conn->ssl_client_cert;
            conn->ssl_client_cert = nullptr;
        }
        return 0;
    }
    case SWR_WORKERS_OP_FACTORY_END:
        serv->factory_->end(info->fd, false);
        return 0;
    case SWR_WORKERS_OP_STORE_CLIENT_CERT: {
        Connection *conn = serv->get_connection_verify_no_ssl(info->fd);
        if (conn) {
            auto packet = serv->get_worker_message_bus()->get_packet();
            conn->ssl_client_cert = new String(packet.data, packet.length);
            conn->ssl_client_cert_pid = swoole_get_worker_pid();
        }
        return 0;
    }
    case SWR_WORKERS_OP_ON_FINISH:
        serv->onFinish(serv, reinterpret_cast<EventData *>(serv->get_worker_message_bus()->get_buffer()));
        return 0;
    case SWR_WORKERS_OP_ON_PIPE_MESSAGE:
        serv->onPipeMessage(serv, reinterpret_cast<EventData *>(serv->get_worker_message_bus()->get_buffer()));
        return 0;
    case SWR_WORKERS_OP_COMMAND_REQUEST:
        serv->call_command_handler(serv->message_bus, worker->get_id(), serv->pipe_command->get_socket(false));
        return 0;
    case SWR_WORKERS_OP_THREAD_SHUTDOWN: {
        Reactor *current = sw_reactor();
        serv->get_thread(current->id)->shutdown(current);
        return 0;
    }
    case SWR_WORKERS_OP_SET_ISOLATION:
        swoole_set_isolation(serv->group_, serv->user_, serv->chroot_);
        return 0;
    case SWR_WORKERS_OP_OTHER_PIPE_NONBLOCK: {
        Worker *other_worker = serv->get_worker(arg);
        if (serv->is_worker() && other_worker->pipe_master) {
            other_worker->pipe_master->set_nonblock();
        }
        return 0;
    }
    case SWR_WORKERS_OP_PROTECT_SESSIONS:
        sw_shm_protect(serv->session_list, PROT_READ);
        return 0;
    case SWR_WORKERS_OP_BUS_EMPTY:
        return serv->get_worker_message_bus()->empty();
    case SWR_WORKERS_OP_BUS_CLEAR:
        serv->get_worker_message_bus()->clear();
        return 0;
    case SWR_WORKERS_OP_ON_WORKER_ERROR:
        if (serv->onWorkerError != nullptr) {
            serv->onWorkerError(serv, worker, *ctx->status);
        }
        return 0;
    case SWR_WORKERS_OP_SEND_SHUTDOWN: {
        swoole_log_as("Server::kill_worker", swoole_trace_log(SW_TRACE_SERVER, "kill worker#%d", static_cast<int>(arg)));
        DataHead event = {};
        event.type = SW_SERVER_EVENT_SHUTDOWN;
        return serv->send_to_worker_from_worker(serv->get_worker(static_cast<int>(arg)), &event, sizeof(event), SW_PIPE_MASTER);
    }
    case SWR_WORKERS_OP_REACTOR_STOP:
        reactor->running = false;
        return 0;
    case SWR_WORKERS_OP_REACTOR_WAIT_EXIT:
        return reactor->wait_exit;
    case SWR_WORKERS_OP_UNREAD_PIPE_WORKER: {
        auto pipe_worker = serv->get_worker_pipe_worker_in_message_bus(worker);
        if (pipe_worker && !pipe_worker->removed) {
            reactor->remove_read_event(pipe_worker);
        }
        return 0;
    }
    case SWR_WORKERS_OP_UNREAD_PIPE_MASTER:
        if (worker->pipe_master && !worker->pipe_master->removed) {
            reactor->remove_read_event(worker->pipe_master);
        }
        return 0;
    case SWR_WORKERS_OP_PORT_COUNT:
        return serv->ports.size();
    case SWR_WORKERS_OP_PORT_DEL:
        reactor->del(serv->ports[arg]->socket);
        return 0;
    case SWR_WORKERS_OP_CONN_UNREAD:
        reactor->remove_read_event(ctx->conn->socket);
        return 0;
    case SWR_WORKERS_OP_REACTOR_ID:
        return reactor->id;
    case SWR_WORKERS_OP_UNREAD_THREAD_PIPE: {
        auto pipe_master = serv->get_worker_pipe_master_in_message_bus(static_cast<WorkerId>(arg));
        if (!pipe_master->removed) {
            reactor->remove_read_event(pipe_master);
        }
        return 0;
    }
    case SWR_WORKERS_OP_ARM_EXIT:
        reactor->set_wait_exit(true);
        reactor->set_end_callback(Reactor::PRIORITY_TRY_EXIT, Worker_reactor_try_to_exit);
        SwooleWG.exit_time = ::time(nullptr);
        return 0;
    case SWR_WORKERS_OP_REACTOR_IF_EXIT:
        return reactor->if_exit();
    case SWR_WORKERS_OP_REACTOR_TIMEOUT:
        return (int64_t) reactor->timeout_msec;
    case SWR_WORKERS_OP_SET_REACTOR_TIMEOUT:
        reactor->timeout_msec = static_cast<int32_t>(arg);
        return 0;
    case SWR_WORKERS_OP_DRAIN_WORKER_PIPES: {
        Worker *_worker = serv->get_worker(arg);
        if (_worker->pipe_worker) {
            sw_reactor()->drain_write_buffer(_worker->pipe_worker);
        }
        if (_worker->pipe_master) {
            sw_reactor()->drain_write_buffer(_worker->pipe_master);
        }
        return 0;
    }
    case SWR_WORKERS_OP_REACTOR_DESTROYED:
        swoole_log_as("Server::clean_worker_connections", swoole_trace_log(SW_TRACE_WORKER, "clean connections"));
        sw_reactor()->destroyed = true;
        return 0;
    case SWR_WORKERS_OP_UNLIMIT_PIPES: {
        // set pipe buffer size
        const Worker *_worker = serv->get_worker(arg);
        if (_worker->pipe_master) {
            _worker->pipe_master->buffer_size = UINT_MAX;
        }
        if (_worker->pipe_worker) {
            _worker->pipe_worker->buffer_size = UINT_MAX;
        }
        return 0;
    }
    case SWR_WORKERS_OP_LISTEN_PIPE:
        worker->pipe_worker->set_nonblock();
        SwooleTG.reactor->ptr = serv;
        SwooleTG.reactor->add(worker->pipe_worker, SW_EVENT_READ);
        SwooleTG.reactor->set_handler(SW_FD_PIPE, SW_EVENT_READ, Worker_onPipeReceive);
        return 0;
    case SWR_WORKERS_OP_COUNT_COROUTINES:
        SwooleTG.reactor->set_end_callback(Reactor::PRIORITY_WORKER_CALLBACK,
                                           [worker](Reactor *) { worker->coroutine_num = Coroutine::count(); });
        return 0;
    case SWR_WORKERS_OP_REACTOR_WAIT:
        return (int64_t) SwooleTG.reactor->wait();
    case SWR_WORKERS_OP_BUS_READ:
        return (int64_t) serv->get_worker_message_bus()->read(ctx->socket);
    case SWR_WORKERS_OP_BUS_POP:
        serv->get_worker_message_bus()->pop();
        return 0;
    default:
        return swr_v1_event_worker_call(context, op, arg);
    }
}

// The ops each received event makes, in a small frame of their own: the rest of
// call needs a large, stack-protected one.
uint64_t WorkerContext::hot_call(void *context, uint32_t op, uint64_t arg) {
    auto *ctx = static_cast<WorkerContext *>(context);
    Server *serv = ctx->serv;
    Worker *worker = ctx->worker;
    DataHead *info = ctx->info;
    switch (op) {
    case SWR_WORKERS_OP_EVENT_BEGIN:
        worker->set_status_to_busy();
        return info->type;
    case SWR_WORKERS_OP_EVENT_END:
        worker->set_status_to_idle();
        return worker->has_exceeded_max_request();
    case SWR_WORKERS_OP_PIPE_EVENT_WRITE:
        return (int64_t) swoole_event_write(ctx->socket, info, arg);
    case SWR_WORKERS_OP_PIPE_SEND_SYNC:
        return (int64_t) ctx->socket->send_sync(info, arg);
    case SWR_WORKERS_OP_RECV_CONN: {
        Connection *conn = serv->get_connection_verify(info->fd);
        if (!conn) {
            return 0;
        }
        if (info->len > 0) {
            auto packet = serv->get_worker_message_bus()->get_packet();
            sw_atomic_fetch_sub(&conn->recv_queued_bytes, packet.length);
            swoole_log_as("Server::worker_accept_event",
                          swoole_trace_log(SW_TRACE_SERVER,
                                           "[Worker] session_id=%ld, len=%lu, qb=%d",
                                           conn->session_id,
                                           packet.length,
                                           conn->recv_queued_bytes));
        }
        conn->last_dispatch_time = info->time;
        return conn->closed ? 2 : 1;
    }
    default:
        return call(context, op, arg);
    }
}

static WorkerContext Worker_context(Server *serv, Worker *worker) {
    return WorkerContext{serv, worker, nullptr, sw_reactor(), nullptr, nullptr, nullptr};
}

static void Worker_reactor_try_to_exit(Reactor *reactor) {
    Server *serv;
    if (sw_likely(swoole_get_worker_type() != SW_TASK_WORKER)) {
        serv = static_cast<Server *>(reactor->ptr);
    } else {
        auto pool = static_cast<ProcessPool *>(reactor->ptr);
        serv = static_cast<Server *>(pool->ptr);
    }
    auto ctx = Worker_context(serv, sw_worker());
    ctx.reactor = reactor;
    ctx.run(SWR_WORKERS_RUN_TRY_EXIT);
}

/**
 * receive data from reactor
 * This function is intended solely for process mode; in thread or base mode, `ReactorThread_onRead()` will be executed.
 */
static int Worker_onPipeReceive(Reactor *reactor, Event *event) {
    auto *serv = static_cast<Server *>(reactor->ptr);
    auto ctx = Worker_context(serv, sw_worker());
    ctx.reactor = reactor;
    ctx.info = &serv->get_worker_message_bus()->get_buffer()->info;
    ctx.socket = event->socket;
    ctx.run(SWR_WORKERS_RUN_PIPE_RECEIVE);
    return SW_OK;
}
static int ReactorProcess_onPipeRead(Reactor *reactor, Event *event);
static int ReactorProcess_onClose(Reactor *reactor, Event *event);
static void ReactorProcess_onTimeout(Timer *timer, TimerNode *tnode);

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

    static uint64_t call(void *context, uint32_t op, uint64_t arg);

    int64_t run(uint32_t operation) {
        return swr_v1_workers_run(operation, 0, this, call);
    }
};
using ReactorProcessContext = WorkersNative<1>;

uint64_t ReactorProcessContext::call(void *context, uint32_t op, uint64_t arg) {
    auto *ctx = static_cast<ReactorProcessContext *>(context);
    Server *serv = ctx->serv;
    Worker *worker = ctx->worker;
    Reactor *reactor = ctx->reactor ? ctx->reactor : SwooleTG.reactor;
    Connection *conn = ctx->conn;
    auto *pipe_buffer = serv->message_bus.get_buffer();
    switch (op) {
    case SWR_WORKERS_OP_POOL_CREATE: {
        ProcessPool *pool = serv->get_event_worker_pool();
        *pool = {};
        if (pool->create(serv->worker_num, 0, SW_IPC_UNIXSOCK) < 0) {
            return (int64_t) SW_ERR;
        }
        pool->set_max_request(serv->max_request, serv->max_request_grace);
        // store to ProcessPool object
        pool->ptr = serv;
        pool->max_wait_time = serv->max_wait_time;
        pool->use_msgqueue = 0;
        pool->main_loop = Server::reactor_process_main_loop;
        pool->onWorkerNotFound = [](ProcessPool *pool, const ExitStatus &exit_status) -> int {
            auto serv = static_cast<Server *>(pool->ptr);
            return serv->restart_worker_process(exit_status);
        };
        memcpy(serv->workers, pool->workers, sizeof(*serv->workers) * serv->worker_num);
        pool->workers = serv->workers;
        SW_LOOP_N(serv->worker_num) {
            pool->workers[i].pool = pool;
            pool->workers[i].set_id(i);
            pool->workers[i].set_type(SW_WORKER);
        }
        return 0;
    }
    case SWR_WORKERS_OP_CREATE_PIPE_BUFFERS:
        return (int64_t) serv->create_pipe_buffers();
    case SWR_WORKERS_OP_RUN_SINGLE_WORKER: {
        ProcessPool *pool = serv->get_event_worker_pool();
        SwooleWG.worker = &pool->workers[0];
        return (int64_t) Server::reactor_process_main_loop(pool, &pool->workers[0]);
    }
    case SWR_WORKERS_OP_BUS_READ:
        return (int64_t) serv->message_bus.read(ctx->event->socket);
    case SWR_WORKERS_OP_INFO_TYPE:
        return pipe_buffer->info.type;
    case SWR_WORKERS_OP_ON_PIPE_MESSAGE:
        serv->onPipeMessage(serv, reinterpret_cast<EventData *>(pipe_buffer));
        return 0;
    case SWR_WORKERS_OP_ON_FINISH:
        serv->onFinish(serv, reinterpret_cast<EventData *>(pipe_buffer));
        return 0;
    case SWR_WORKERS_OP_STOP_ASYNC:
        serv->stop_async_worker(serv->get_worker(reactor->id));
        return 0;
    case SWR_WORKERS_OP_FINISH_SEND_FILE: {
        SendData _send;
        _send.info = pipe_buffer->info;
        _send.data = pipe_buffer->data;
        serv->factory_->finish(&_send);
        return 0;
    }
    case SWR_WORKERS_OP_INFO_REACTOR_ID:
        return (int64_t) pipe_buffer->info.reactor_id;
    case SWR_WORKERS_OP_FINISH_SEND_DATA: {
        SendData _send;
        auto packet = serv->message_bus.get_packet();
        memcpy(&_send.info, &pipe_buffer->info, sizeof(_send.info));
        _send.info.type = SW_SERVER_EVENT_RECV_DATA;
        _send.data = packet.data;
        _send.info.len = packet.length;
        serv->factory_->finish(&_send);
        return 0;
    }
    case SWR_WORKERS_OP_END_ACTIVELY:
        serv->factory_->end(pipe_buffer->info.fd, Server::CLOSE_ACTIVELY);
        return 0;
    case SWR_WORKERS_OP_COMMAND_REQUEST:
        serv->call_command_handler(serv->message_bus, sw_worker()->get_id(), serv->get_worker(0)->pipe_master);
        return 0;
    case SWR_WORKERS_OP_COMMAND_RESPONSE: {
        auto packet = serv->message_bus.get_packet();
        swr_v1_server_command_result(serv->state_handle(), serv, pipe_buffer->info.fd, packet.data, packet.length);
        return 0;
    }
    case SWR_WORKERS_OP_BUS_POP:
        serv->message_bus.pop();
        return 0;

    case SWR_WORKERS_OP_TIMER_DETACHED:
        return SwooleTG.timer && SwooleTG.timer->get_reactor() == nullptr;
    case SWR_WORKERS_OP_TIMER_REINIT:
        SwooleTG.timer->reinit();
        return 0;
    case SWR_WORKERS_OP_PORT_ADD: {
        auto ls = serv->ports[arg];
        ls->gs->connection_nums[worker->get_id()] = 0;
        return (int64_t) reactor->add(ls->socket, SW_EVENT_READ);
    }
    case SWR_WORKERS_OP_REACTOR_SETUP:
        reactor->id = worker->get_id();
        reactor->ptr = serv;
        reactor->max_socket = serv->get_max_connection();
        reactor->close = Server::close_connection;
        reactor->set_handler(SW_FD_STREAM_SERVER, SW_EVENT_READ, Server::accept_connection);
    // close
    reactor->set_default_error_handler(ReactorProcess_onClose);
    // pipe
        reactor->set_handler(SW_FD_PIPE, SW_EVENT_READ, ReactorProcess_onPipeRead);
        return 0;
    case SWR_WORKERS_OP_ADD_PIPES:
        worker->pipe_worker->set_nonblock();
        worker->pipe_master->set_nonblock();
        if (reactor->add(worker->pipe_worker, SW_EVENT_READ) < 0) {
            return (int64_t) SW_ERR;
        }
        return (int64_t) reactor->add(worker->pipe_master, SW_EVENT_READ);
    case SWR_WORKERS_OP_TASK_PIPE_NONBLOCK:
        serv->get_task_worker_pool()->workers[arg].pipe_master->set_nonblock();
        return 0;
    case SWR_WORKERS_OP_ADD_HEARTBEAT_TIMER:
        serv->heartbeat_timer =
            swoole_timer_add(sec2msec(serv->heartbeat_check_interval), true, ReactorProcess_onTimeout, reactor);
        return serv->heartbeat_timer != nullptr;
    case SWR_WORKERS_OP_REACTOR_DEL_EVENT:
        return (int64_t) reactor->del(ctx->event->socket);
    case SWR_WORKERS_OP_CLOSE_NOTIFY: {
        Event notify_ev{};
        notify_ev.type = SW_FD_SESSION;
        notify_ev.fd = conn->fd;
        notify_ev.socket = conn->socket;
        notify_ev.reactor_id = conn->reactor_id;
        ReactorProcess_onClose(reactor, &notify_ev);
        return 0;
    }
    default:
        return swr_v1_reactor_process_call(context, op, arg);
    }
}

static int ReactorProcess_onPipeRead(Reactor *reactor, Event *event) {
    auto *serv = static_cast<Server *>(reactor->ptr);
    return ReactorProcessContext{serv, nullptr, nullptr, reactor, event, nullptr, 0}.run(
        SWR_WORKERS_RUN_REACTOR_PROCESS_PIPE_READ);
}

static int ReactorProcess_onClose(Reactor *reactor, Event *event) {
    auto *serv = static_cast<Server *>(reactor->ptr);
    return ReactorProcessContext{serv, nullptr, nullptr, reactor, event, nullptr, 0}.run(
        SWR_WORKERS_RUN_REACTOR_PROCESS_CLOSE);
}

static void ReactorProcess_onTimeout(Timer *timer, TimerNode *tnode) {
    auto *reactor = static_cast<Reactor *>(tnode->data);
    auto *serv = static_cast<Server *>(reactor->ptr);
    ReactorProcessContext{serv, nullptr, nullptr, reactor, nullptr, nullptr, 0}.run(
        SWR_WORKERS_RUN_REACTOR_PROCESS_TIMEOUT);
}
// What a crates/process/src/workers/task_worker.rs operation acts on. crates/server's
// task_worker_ops.rs reads the fields up to `file`.
template <>
struct WorkersNative<2> {
    Server *serv;
    ProcessPool *pool;
    Worker *worker;
    // The task being run, or the one being finished.
    const EventData *task;
    Socket *socket;
    // Server::finish's result, its packed form and its file.
    const char *data;
    size_t data_len;
    EventData *packed;
    EventData *buf;
    int file;
    swr_v1_server_command_fn handler;
    PacketPtr packet;

    static uint64_t call(void *context, uint32_t op, uint64_t arg);

    int64_t run(uint32_t operation, uint64_t arg = 0) {
        return swr_v1_workers_run(operation, arg, this, call);
    }
};
using TaskWorkerContext = WorkersNative<2>;

static int TaskWorker_onPipeReceive(Reactor *reactor, Event *event);

static int TaskWorker_onTask(ProcessPool *pool, Worker *worker, EventData *task) {
    TaskWorkerContext ctx{static_cast<Server *>(pool->ptr), pool, worker, task};
    return ctx.run(SWR_WORKERS_RUN_TASK_ON_TASK);
}

static void TaskWorker_onStart(ProcessPool *pool, Worker *worker) {
    TaskWorkerContext ctx{static_cast<Server *>(pool->ptr), pool, worker};
    ctx.run(SWR_WORKERS_RUN_TASK_ON_START);
}

static void TaskWorker_onStop(ProcessPool *pool, Worker *worker) {
    TaskWorkerContext ctx{static_cast<Server *>(pool->ptr), pool, worker};
    ctx.run(SWR_WORKERS_RUN_TASK_ON_STOP);
}

static int TaskWorker_loop_async(ProcessPool *pool, Worker *worker) {
    TaskWorkerContext ctx{static_cast<Server *>(pool->ptr), pool, worker};
    return ctx.run(SWR_WORKERS_RUN_TASK_LOOP_ASYNC);
}

uint64_t TaskWorkerContext::call(void *context, uint32_t op, uint64_t arg) {
    auto *ctx = static_cast<TaskWorkerContext *>(context);
    Server *serv = ctx->serv;
    ProcessPool *pool = ctx->pool;
    Worker *worker = ctx->worker;
    switch (op) {
    case SWR_WORKERS_OP_POOL_INIT:
        pool->ptr = serv;
        pool->onTask = TaskWorker_onTask;
        pool->onWorkerStart = TaskWorker_onStart;
        pool->onWorkerStop = TaskWorker_onStop;
        return 0;
    case SWR_WORKERS_OP_SET_ASYNC_LOOP:
        pool->main_loop = TaskWorker_loop_async;
        return 0;

    case SWR_WORKERS_OP_HAS_COMMAND: {
        return swr_v1_server_command_handler(serv->state_handle(), serv->get_command_id(ctx->task), &ctx->handler);
    }
    case SWR_WORKERS_OP_UNPACK_COMMAND:
        return Server::task_unpack(const_cast<EventData *>(ctx->task), sw_tg_buffer(), &ctx->packet);
    case SWR_WORKERS_OP_COMMAND_REPLY: {
        auto result = detail::command_answer(ctx->handler, serv, ctx->packet.data, ctx->packet.length);
        SendData task{};
        task.info.fd = serv->get_task_id(ctx->task);
        task.info.reactor_id = worker->get_id();
        task.info.server_fd = -1;
        task.info.type = SW_SERVER_EVENT_COMMAND_RESPONSE;
        task.info.len = result.length();
        task.data = result.c_str();
        return serv->message_bus.write(serv->get_command_reply_socket(), &task);
    }

    case SWR_WORKERS_OP_LISTEN_TASK_PIPE: {
        Socket *socket = serv->get_worker_pipe_worker_in_message_bus(worker);
        socket->set_nonblock();
        sw_reactor()->ptr = pool;
        swoole_event_add(socket, SW_EVENT_READ);
        swoole_event_set_handler(SW_FD_PIPE, SW_EVENT_READ, TaskWorker_onPipeReceive);
        return 0;
    }
    case SWR_WORKERS_OP_UNLIMIT_BUS_PIPES: {
        Worker *_worker = serv->get_worker(arg);
        serv->get_worker_pipe_worker_in_message_bus(_worker)->buffer_size = UINT_MAX;
        serv->get_worker_pipe_master_in_message_bus(_worker)->buffer_size = UINT_MAX;
        return 0;
    }
    default:
        return swr_v1_task_worker_call(context, op, arg);
    }
}
static_assert(offsetof(TaskWorkerContext, file) == 72 && SW_SERVER_EVENT_FINISH == 9 && SW_WORKER_IDLE == 2,
              "crates/server/src/task_worker_ops.rs layout and constants");

// Server::task_pack, task_unpack and task_dump are Rust's (crates/process/src/task_file.rs).
static_assert(sizeof(DataHead) == 40 && offsetof(EventData, data) == sizeof(DataHead) && sizeof(String) == 40 &&
                  sizeof(PacketTask) == sizeof(size_t) + SW_TASK_TMP_PATH_SIZE,
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

/**
 * receive data from worker process
 */
static int TaskWorker_onPipeReceive(Reactor *reactor, Event *event) {
    auto *pool = static_cast<ProcessPool *>(reactor->ptr);
    EventData task;
    TaskWorkerContext ctx{static_cast<Server *>(pool->ptr), pool, sw_worker()};
    ctx.socket = event->socket;
    ctx.buf = &task;
    return ctx.run(SWR_WORKERS_RUN_TASK_ON_PIPE);
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
    case SWR_WORKERS_OP_NEW_REACTOR_THREADS:
        serv->reactor_threads = new ReactorThread[serv->reactor_num]();
        return 0;
    case SWR_WORKERS_OP_NEW_THREAD_FACTORY:
        return (uintptr_t) new ThreadFactory(serv);
    case SWR_WORKERS_OP_DELETE_REACTOR_THREADS:
        delete[] serv->reactor_threads;
        return 0;
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

ThreadFactory::ThreadFactory(Server *server) : BaseFactory(server) {
    manager_ = swr_v1_thread_manager_new();
    if (!swr_v1_thread_manager_threads_init(manager_, server_->get_all_worker_num() + 1, &thread_ops)) {
        swr_v1_thread_manager_free(manager_);
        throw std::bad_alloc();
    }
}

std::shared_ptr<Thread> &ThreadFactory::thread_at(size_t index) const {
    return *static_cast<std::shared_ptr<Thread> *>(swr_v1_thread_manager_thread(manager_, index));
}

ThreadFactory::~ThreadFactory() {
    ThreadFactory::shutdown();
    swr_v1_thread_manager_free(manager_);
}

bool ThreadFactory::shutdown() {
    int error = swr_v1_thread_manager_join_all(manager_);
    if (error) throw std::system_error(error, std::generic_category());
    return true;
}

// What a crates/process/src/workers/manager.rs operation acts on.
template <>
struct WorkersNative<3> {
    Server *serv;
    // The worker being restarted, or the message being read.
    Worker *worker;
    const EventData *task;
    swr_v1_server_command_fn handler;
    // Rust (crates/server's manager_ops.rs) reads the fields above and these two.
    int wait_status;
    int wait_error;
    EventData msg;
    WorkerStopMessage stop;

    static uint64_t call(void *context, uint32_t op, uint64_t arg);

    int64_t run(uint32_t operation, uint64_t arg = 0) {
        return swr_v1_workers_run(operation, arg, this, call);
    }
};
using ManagerContext = WorkersNative<3>;

static void Manager_signal_handler(int signo) {
    ManagerContext ctx{sw_server()};
    ctx.run(SWR_WORKERS_RUN_MANAGER_SIGNAL, signo);
}

uint64_t ManagerContext::call(void *context, uint32_t op, uint64_t arg) {
    auto *ctx = static_cast<ManagerContext *>(context);
    Server *serv = ctx->serv;
    switch (op) {
    case SWR_WORKERS_OP_FORK_MANAGER:
        return (int64_t) swoole_fork_exec([serv]() {
            ManagerContext child{serv};
            child.run(SWR_WORKERS_RUN_MANAGER_MAIN);
        });
    case SWR_WORKERS_OP_SIGNAL_SET:
        swr_workers_signal_set(arg, Manager_signal_handler);
        return 0;
    case SWR_WORKERS_OP_MANAGER_BARRIER:
        serv->gs->manager_barrier.wait();
        return 0;
    case SWR_WORKERS_OP_HAS_USER:
        return !serv->user_.empty();
    case SWR_WORKERS_OP_SET_ISOLATION:
        swoole_set_isolation(serv->group_, serv->user_, serv->chroot_);
        return 0;
    case SWR_WORKERS_OP_KILL_EVENT_WORKERS:
        serv->factory_->kill_event_workers();
        return 0;
    case SWR_WORKERS_OP_KILL_USER_WORKERS:
        serv->factory_->kill_user_workers();
        return 0;
    case SWR_WORKERS_OP_POP_MESSAGE:
        return (int64_t) serv->get_event_worker_pool()->pop_message(&ctx->msg, sizeof(ctx->msg));
    case SWR_WORKERS_OP_MESSAGE_TYPE:
        return ctx->msg.info.type;
    case SWR_WORKERS_OP_ON_WORKER_MESSAGE:
        serv->get_event_worker_pool()->onWorkerMessage(serv->get_event_worker_pool(), &ctx->msg);
        return 0;
    case SWR_WORKERS_OP_STOP_MESSAGE_WORKER:
        memcpy(&ctx->stop, ctx->msg.data, sizeof(ctx->stop));
        return ctx->stop.worker_id;
    case SWR_WORKERS_OP_SPAWN_TASK_WORKER:
        serv->factory_->spawn_task_worker(serv->get_worker(arg));
        return 0;
    case SWR_WORKERS_OP_DETACH_STOPPED:
        swr_v1_process_worker_registry_detach(static_cast<swr_v1_process_worker_registry *>(serv->worker_registry),
                                              ctx->stop.pid);
        return 0;
    case SWR_WORKERS_OP_RESPAWN_TASK_WORKER:
        return (int64_t) serv->factory_->spawn_task_worker(ctx->worker);
    case SWR_WORKERS_OP_COMMAND_REPLY: {
        const auto result = detail::command_answer(ctx->handler, serv, ctx->task->data, ctx->task->info.len);
        SendData task{};
        task.info.fd = ctx->task->info.fd;
        task.info.reactor_id = 0;
        task.info.server_fd = -1;
        task.info.type = SW_SERVER_EVENT_COMMAND_RESPONSE;
        task.info.len = result.length();
        task.data = result.c_str();
        serv->message_bus.write(serv->get_command_reply_socket(), &task);
        return 0;
    }
    default:
        return swr_v1_manager_call(context, op, arg);
    }
}

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
                             other ? static_cast<ThreadFactory *>(other)
                                   : dynamic_cast<ThreadFactory *>(serv->factory_)}
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

ProcessFactory::ProcessFactory(Server *server) : Factory(server) {}

ProcessFactory::~ProcessFactory() = default;

/**
 * kill and wait all user process
 */
void Factory::kill_user_workers() const {
    swr_v1_process_worker_registry_terminate(
        worker_registry_of(server_), SWR_WORKER_USER, SIGTERM, swoole_kill, swoole_waitpid, [](int pid) {
            swoole_log_as("Factory::kill_user_workers", swoole_sys_warning("waitpid(%d) failed", pid));
        });
}

/**
 * [Manager] kill and wait all event worker process
 */
void Factory::kill_event_workers() const {
    swr_v1_process_worker_registry_terminate(
        worker_registry_of(server_), SWR_WORKER_EVENT, SIGTERM, swoole_kill, swoole_waitpid, event_worker_waitpid_warn);
}

pid_t Factory::spawn_task_worker(Worker *worker) const {
    return server_->get_task_worker_pool()->spawn(worker);
}
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
