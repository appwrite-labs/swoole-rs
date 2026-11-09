// SPDX-License-Identifier: Apache-2.0
// What crates/server needs from C++ about swoole::Server: building and destroying
// it, its ListenPorts, the std::string methods and the barriers. The rest is crates/server, from
// swr_v1_server_open to swr_v1_server_close.
#include "swoole_server.h"
#include "swoole_rs.h"

#include <type_traits>

using swoole::network::SendfileTask;

namespace swoole {

static_assert(SW_ACCEPT_MAX_COUNT == 64 && SW_MAX_CONNECTION == 100000 && SW_SESSION_LIST_SIZE == 1024 * 1024,
              "crates/server/src/connection.rs constants");
static_assert(SW_MAX_THREAD_NCPU == 4 && SW_MAX_WORKER_NCPU == 1000 && SW_TIMER_MIN_SEC == 0.001 &&
                  SW_BUFFER_MIN_SIZE == 65536,
              "crates/server/src/startup.rs constants");
static_assert(Server::DISPATCH_FDMOD == 2 && Server::DISPATCH_IPMOD == 4 && Server::DISPATCH_CO_CONN_LB == 8 &&
                  Server::TASK_IPC_MSGQUEUE == 2 && Server::TASK_IPC_PREEMPTIVE == 3 &&
                  Server::TASK_IPC_STREAM == 4 && SW_IPC_UNIXSOCK == 1 && SW_IPC_MSGQUEUE == 2 && SW_IPC_SOCKET == 3,
              "crates/server/src/startup.rs modes");
static_assert(Server::DISPATCH_ROUND == 1 && Server::DISPATCH_UIDMOD == 5 && Server::DISPATCH_CO_REQ_LB == 9 &&
                  Server::DISPATCH_CONCURRENT_LB == 10 && Server::DISPATCH_RESULT_USERFUNC_FALLBACK == -3 &&
                  SW_SOCK_UDP == 2 && SW_SOCK_UDP6 == 4 && SW_SOCK_UNIX_STREAM == 5 && SW_SOCK_UNIX_DGRAM == 6 &&
                  SW_SOCK_SSL == (1u << 9) && SW_MAX_LISTEN_PORT == 60000 && SW_SYSTEMD_FDS_START == 3,
              "crates/server/src/dispatch.rs constants");
// crates/server/src/shared.rs: a ListenPort's first word is its settings pointer.
static_assert(!std::is_polymorphic<ListenPort>::value, "crates/server/src/shared.rs");
static_assert(Server::MODE_BASE == 1 && Server::MODE_PROCESS == 2 && Server::MODE_THREAD == 3,
              "crates/server/src/lib.rs modes");
static_assert(Server::CLOSE_RESET == (1 << 1) && Server::CLOSE_ACTIVELY == (1 << 2) && SW_EVENT_DATA_PTR == (1 << 1) &&
                  SW_SERVER_EVENT_CLOSE_FORWARD == 7 && SW_SERVER_EVENT_BUFFER_EMPTY == 14,
              "crates/server/src/base_factory.rs");
// crates/server/src/server_methods.rs: Server::sendfile's buffer, and crates/server/src/factory.rs's record.
static_assert(SW_IPC_BUFFER_SIZE == SW_IPC_MAX_SIZE - 40 && sizeof(EventData) == SW_IPC_MAX_SIZE &&
                  offsetof(SendfileTask, length) == 8 && SW_SERVER_EVENT_PIPE_MESSAGE == 10 &&
                  sizeof(Factory) == 2 * sizeof(void *),
              "crates/server/src/server_methods.rs");
// crates/server/src/master_ops.rs allocates these arrays with its own record sizes, and reads a
// MessageBus's Rust bus and allocator as its first two words.
static_assert(sizeof(ServerPortGS) == 80 && sizeof(Session) == 16 && sizeof(sw_atomic_t) == 4 &&
                  SW_SERVER_EVENT_COMMAND_REQUEST == 17,
              "crates/server/src/master_ops.rs");
static_assert(SW_CONTINUE == 1 && SW_WAIT == 2 && SW_ERROR == 4 && SW_READY == 5,
              "crates/server/src/reactor_thread.rs return codes");
static_assert(SW_SERVER_EVENT_CLOSE_FORCE == SWR_SERVER_EVENT_CLOSE_FORCE &&
                  SW_SERVER_EVENT_CLOSE_FORWARD == SWR_SERVER_EVENT_CLOSE_FORWARD &&
                  SW_SERVER_EVENT_FINISH == SWR_SERVER_EVENT_FINISH &&
                  SW_SERVER_EVENT_PIPE_MESSAGE == SWR_SERVER_EVENT_PIPE_MESSAGE &&
                  SW_SERVER_EVENT_INCOMING == SWR_SERVER_EVENT_INCOMING &&
                  SW_SERVER_EVENT_SHUTDOWN == SWR_SERVER_EVENT_SHUTDOWN &&
                  SW_SERVER_EVENT_COMMAND_REQUEST == SWR_SERVER_EVENT_COMMAND_REQUEST &&
                  SW_SERVER_EVENT_COMMAND_RESPONSE == SWR_SERVER_EVENT_COMMAND_RESPONSE &&
                  SW_SERVER_EVENT_SHUTDOWN_SIGNAL == SWR_SERVER_EVENT_SHUTDOWN_SIGNAL,
              "crates/server/src/ops.rs pipe events");
static_assert(offsetof(ReactorThread, heartbeat_timer) == 32 && offsetof(ReactorThread, message_bus) == 40 &&
                  sizeof(ReactorThread) == 56 && SW_SERVER_EVENT_CLOSE == 4 && SW_SERVER_EVENT_CONNECT == 5,
              "crates/server/src/reactor_ops.rs");
static_assert(BufferChunk::TYPE_DATA == 0 && BufferChunk::TYPE_SENDFILE == 1 && BufferChunk::TYPE_CLOSE == 2,
              "crates/server/src/conn_io.rs output kinds");

// What a crates/server operation acts on besides the server (its NativeContext).
struct ServerRustContext {
    Server *serv;
    Reactor *reactor;
    Event *event;
    ListenPort *port;
    network::Socket *sock;
    Connection *conn;
    int server_fd;
    ReactorId reactor_id;
    SendData *send;
    const void *data;
    uint32_t length;
    SessionId session_id;
    char *buffer;
    off_t offset;
    SocketType type;
    const char *host;
    int number;
    void *object;
};
static_assert(sizeof(ServerRustContext) == 136 && offsetof(ServerRustContext, object) == 128,
              "crates/server/src/server_methods.rs NativeContext");

Server::Server(Mode mode)
    : detail::ServerFields(SW_CPU_NUM, SwooleG.max_sockets, get_timezone(), &SwooleG.std_allocator) {
    if (!swr_v1_server_open(this, mode, sizeof(ServerGS))) {
        throw std::bad_alloc();
    }
    message_bus.count_ids(&gs->pipe_packet_msg_id);
    tables_->message_bus = &message_bus;
}

Server::~Server() {
    swr_v1_server_close(this, [](void *port) { delete static_cast<ListenPort *>(port); });
}

bool Server::set_document_root(const std::string &path) {
    ServerRustContext ctx{this};
    ctx.data = path.c_str();
    return swr_v1_server_method(SWR_SERVER_RUN_ST_DOCUMENT_ROOT, path.length(), &ctx);
}

void Server::add_http_compression_type(const std::string &type) {
    http_compression_types.insert(type);
}
}  // namespace swoole

using swoole::ListenPort;
using swoole::Server;

extern "C" {
// The Rust ports of core-tests' server cases (tests/modules/http/server/src/http1.rs) make and delete theirs here.
Server *swr_v1_server_new(int mode) {
    return new Server(static_cast<Server::Mode>(mode));
}

void swr_v1_server_delete(Server *serv) {
    delete serv;
}

ListenPort *swr_v1_server_port_new(Server *serv) {
    return new ListenPort(serv);
}

void swr_v1_server_port_delete(ListenPort *port) {
    delete port;
}

swoole::Barrier *swr_v1_server_barrier(Server *serv, bool manager) {
    return manager ? &serv->gs->manager_barrier : &serv->reactor_thread_barrier;
}
}
