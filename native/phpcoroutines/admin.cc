// SPDX-License-Identifier: Apache-2.0
// The admin server's commands are crates/server/src/admin.rs; this lends it the
// C++ server's state: Server, Connection, ListenPort, ReactorThread and the
// reactor's sockets, which Rust reads through swr_v1_server_admin_* records.
#include "swoole_rs_phphost.h"
#include "swoole_server.h"
#include "swoole_rs.h"

#include <algorithm>
#include <cstring>
#include <iterator>

namespace swoole {

struct AdminContext {
    Server *serv;
    std::vector<void *> items;
    std::string text;
    std::string answer;
};

static swr_v1_server_text admin_text(const char *text) {
    return {text, strlen(text)};
}

static void admin_socket(network::Socket *socket, void *out) {
    *static_cast<swr_v1_server_admin_socket *>(out) = {
        socket->fd, socket->events, socket->socket_type, socket->fd_type, socket->get_port(),
        socket->total_recv_bytes, socket->total_send_bytes, socket->get_out_buffer_length(),
        admin_text(socket->get_addr())};
}

static size_t get_socket_out_buffer_total_size() {
    if (!sw_reactor()) {
        return 0;
    }
    size_t size = 0;
    sw_reactor()->foreach_socket([&size](int, network::Socket *socket) { size += socket->get_out_buffer_length(); });
    return size;
}

static uint64_t admin_memory(Server *serv, void *out) {
    bool is_thread = serv->is_reactor_thread();
    const swr_v1_server_fact facts[] = {
        {"server", sizeof(Server)},
        {"workers", serv->get_all_worker_num() * sizeof(Worker)},
        {"connection_list", serv->get_max_connection() * sizeof(Connection)},
        {"session_list", SW_SESSION_LIST_SIZE * sizeof(Session)},
        {"global_memory", dynamic_cast<GlobalMemory *>(sw_mem_pool())->get_memory_size()},
        {"thread_global_memory", sw_tg_buffer()->size},
        {"message_bus",
         is_thread ? serv->get_thread(SwooleTG.id)->message_bus.get_memory_size()
                   : serv->message_bus.get_memory_size()},
        {"socket_list", sw_reactor() ? sw_reactor()->get_event_num() * sizeof(network::Socket) : 0},
        {"socket_out_buffer", get_socket_out_buffer_total_size()},
        {"php_memory", is_thread ? 0 : zend_memory_usage(true)},
    };
    std::copy(std::begin(facts), std::end(facts), static_cast<swr_v1_server_fact *>(out));
    return sizeof(facts) / sizeof(facts[0]);
}

static void admin_connections(Server *serv, std::vector<void *> &list) {
    // Server::foreach_connection: each valid connection, max_fd read afresh.
    for (int fd = serv->gs->min_fd; fd <= serv->gs->max_fd; fd++) {
        Connection *conn = serv->get_connection(fd);
        if (!conn || !conn->socket || !conn->active || conn->socket->fd_type != SW_FD_SESSION) {
            continue;
        }
        if (serv->is_process_mode() && conn->reactor_id != SwooleTG.id) {
            continue;
        }
        if (serv->is_base_mode() && sw_worker() && conn->reactor_id != sw_worker()->get_id()) {
            continue;
        }
        list.push_back(conn);
    }
}

static uint64_t admin_call(void *context, uint32_t op, uint64_t arg, void *out) {
    auto *ctx = static_cast<AdminContext *>(context);
    Server *serv = ctx->serv;
    switch (op) {
    case SWR_SERVER_ADMIN_ANSWER:
        ctx->answer.assign(static_cast<const char *>(out), arg);
        return 0;
    case SWR_SERVER_ADMIN_PIPE_WORKERS:
        return arg ? serv->task_worker_num : serv->worker_num;
    case SWR_SERVER_ADMIN_PIPE_SOCKET: {
        ProcessPool *pool = (arg & 2) ? serv->get_task_worker_pool() : serv->get_event_worker_pool();
        admin_socket(pool->workers[arg >> 2].pipe_object->get_socket(arg & 1), out);
        return 0;
    }
    case SWR_SERVER_ADMIN_HAS_REACTOR:
        return sw_reactor() != nullptr;
    case SWR_SERVER_ADMIN_SOCKETS:
        ctx->items.clear();
        sw_reactor()->foreach_socket([ctx](int, network::Socket *socket) { ctx->items.push_back(socket); });
        return ctx->items.size();
    case SWR_SERVER_ADMIN_SOCKET:
        admin_socket(static_cast<network::Socket *>(ctx->items[arg]), out);
        return 0;
    case SWR_SERVER_ADMIN_SOCKET_NAME:
        return static_cast<network::Socket *>(ctx->items[arg])->get_name();
    case SWR_SERVER_ADMIN_COMMANDS:
        ctx->items.clear();
        { swr_v1_server_command_info info;
          while (swr_v1_server_command_at(serv->state_handle(), ctx->items.size(), &info)) ctx->items.push_back(nullptr); }
        return ctx->items.size();
    case SWR_SERVER_ADMIN_COMMAND: {
        swr_v1_server_command_info command;
        swr_v1_server_command_at(serv->state_handle(), arg, &command);
        *static_cast<swr_v1_server_admin_command_info *>(out) = {
            {reinterpret_cast<const char *>(command.name), command.length}, command.id, command.accepted_types};
        return 0;
    }
    case SWR_SERVER_ADMIN_CONNECTIONS:
        ctx->items.clear();
        admin_connections(serv, ctx->items);
        return ctx->items.size();
    case SWR_SERVER_ADMIN_SESSION: {
        Connection *conn = serv->get_connection_verify((SessionId) (int64_t) arg);
        ctx->items.assign(conn ? 1 : 0, conn);
        return conn != nullptr;
    }
    case SWR_SERVER_ADMIN_CONNECTION: {
        auto *conn = static_cast<Connection *>(ctx->items[arg]);
        auto server_socket = serv->get_port_by_server_fd(conn->server_fd)->socket;
        // Address::get_addr() formats into one buffer: keep the port's text first.
        ctx->text = server_socket->info.get_addr();
        *static_cast<swr_v1_server_admin_connection *>(out) = {
            conn->session_id, conn->reactor_id, conn->fd, server_socket->info.get_port(), conn->info.get_port(),
            conn->overflow, conn->recv_queued_bytes, conn->send_queued_bytes, conn->socket->total_recv_bytes,
            conn->socket->total_send_bytes, conn->uid, conn->connect_time, conn->last_recv_time,
            conn->last_send_time, conn->last_dispatch_time, {ctx->text.data(), ctx->text.length()},
            admin_text(conn->info.get_addr())};
        return 0;
    }
    case SWR_SERVER_ADMIN_THREAD: {
        ReactorThread *thread = serv->get_thread(SwooleTG.id);
        *static_cast<swr_v1_server_admin_thread *>(out) = {
            thread->id, thread->dispatch_count, SwooleTG.reactor->get_event_num(),
            SwooleTG.timer ? SwooleTG.timer->count() : 0};
        return 0;
    }
    case SWR_SERVER_ADMIN_MANAGER: {
        ProcessPool *pool = serv->get_event_worker_pool();
        *static_cast<swr_v1_server_admin_manager *>(out) = {pool->reload_count, pool->reload_last_time};
        return 0;
    }
    case SWR_SERVER_ADMIN_MEMORY:
        return admin_memory(serv, out);
    case SWR_SERVER_ADMIN_PORTS:
        ctx->items.assign(serv->ports.begin(), serv->ports.end());
        return ctx->items.size();
    case SWR_SERVER_ADMIN_PORT: {
        auto *port = static_cast<ListenPort *>(ctx->items[arg]);
        *static_cast<swr_v1_server_admin_port *>(out) = {{port->host.data(), port->host.length()},
                                                         admin_text(port->get_protocols()),
                                                         port->port,
                                                         port->backlog,
                                                         port->type,
                                                         port->ssl,
                                                         (long) port->get_connection_num()};
        return 0;
    }
    default:
        abort();
    }
}

static void admin_command(void *i, void *serv, const char *msg, size_t length, swr_v1_server_command_reply reply, void *out) {
    AdminContext ctx{static_cast<Server *>(serv)};
    swr_v1_server_admin_run((uint32_t) (uintptr_t) i, msg, length, &ctx, admin_call);
    reply(out, ctx.answer.data(), ctx.answer.length());
}

void register_admin_server_commands(Server *serv) {
    int types;
    const char *name;
    bool event_workers = serv->is_base_mode() || serv->single_thread;
    for (uint32_t i = 0; (name = swr_v1_server_admin_command(i, event_workers, &types)); i++) {
        swr_v1_server_add_command(serv, name, strlen(name), types, {reinterpret_cast<void (*)()>(admin_command), (void *) (uintptr_t) i});
    }
}
}  // namespace swoole
