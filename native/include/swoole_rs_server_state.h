#ifndef SWOOLE_RS_SERVER_STATE_H
#define SWOOLE_RS_SERVER_STATE_H
#include <stdbool.h>
#include <stddef.h>
#include <stdint.h>
#include <time.h>

#ifdef __cplusplus
#include <stdexcept>
namespace swoole {
struct Connection; struct Session; struct ServerPortGS; struct ReactorThread; struct ListenPort;
struct EventData; struct Worker; struct ServerGS; struct TimerNode; struct Allocator; struct Manager;
class Factory; class UnixSocket; class Server;
namespace network { struct Socket; }
}
#define SWR_SERVER_NATIVE(type) swoole::type
extern "C" {
#else
#define SWR_SERVER_NATIVE(type) void
#endif

typedef struct swr_v1_server_state swr_v1_server_state;
const void *swr_v1_server_static_owner(const swr_v1_server_state *, uint32_t);
const void *swr_v1_server_static_owner_clone(const void *);
void swr_v1_server_static_owner_free(const void *);
bool swr_v1_server_static_mime(const void *, const uint8_t *, size_t);
size_t swr_v1_server_static_config(const swr_v1_server_state *, uint32_t, uint32_t, const uint8_t *, size_t);
const uint8_t *swr_v1_server_static_text(const swr_v1_server_state *, uint32_t, size_t, size_t *);
/* url_rewrite_rules (crates/server/src/static_config.rs): add one, and rewrite a static context's URL. */
void swr_v1_server_static_rewrite_add(const swr_v1_server_state *, const uint8_t *, size_t, const uint8_t *, size_t);
struct swr_v2_http_context;
bool swr_v1_server_static_rewrite(const swr_v1_server_state *, struct swr_v2_http_context *);
/* Server's text members, which Rust owns (crates/server/src/state.rs). */
enum {
    SWR_SERVER_TEXT_CHROOT, SWR_SERVER_TEXT_USER, SWR_SERVER_TEXT_GROUP, SWR_SERVER_TEXT_PID_FILE,
    SWR_SERVER_TEXT_DOCUMENT_ROOT,
};
void swr_v1_server_port_shared_init(void *storage);
bool swr_v1_server_record_layout(uint32_t kind, size_t size, size_t alignment, const size_t *offsets, size_t count);
/* One of the server's callbacks (crates/server/src/callbacks.rs). Rust owns the slot and
   calls `drop(ctx)` when the slot is cleared or the state freed. */
typedef struct swr_v1_server_callback {
    void (*invoke)(void);
    void *ctx;
    void (*drop)(void *);
} swr_v1_server_callback;
/* The slots, in callbacks.rs's order. */
enum {
    SWR_SERVER_CB_CONNECT, SWR_SERVER_CB_CLOSE, SWR_SERVER_CB_BUFFER_FULL, SWR_SERVER_CB_BUFFER_EMPTY,
    SWR_SERVER_CB_TASK, SWR_SERVER_CB_RECEIVE, SWR_SERVER_CB_PACKET, SWR_SERVER_CB_START,
    SWR_SERVER_CB_BEFORE_SHUTDOWN, SWR_SERVER_CB_SHUTDOWN, SWR_SERVER_CB_MANAGER_START, SWR_SERVER_CB_MANAGER_STOP,
    SWR_SERVER_CB_WORKER_ERROR, SWR_SERVER_CB_BEFORE_RELOAD, SWR_SERVER_CB_AFTER_RELOAD, SWR_SERVER_CB_PIPE_MESSAGE,
    SWR_SERVER_CB_WORKER_START, SWR_SERVER_CB_WORKER_STOP, SWR_SERVER_CB_WORKER_EXIT, SWR_SERVER_CB_USER_WORKER_START,
    SWR_SERVER_CB_FINISH, SWR_SERVER_CB_DISPATCH_FUNC, SWR_SERVER_CB_COUNT,
};
#define SWR_SERVER_CONFIG_FIELDS(F) \
    F(uint16_t, reactor_num) \
    F(uint32_t, worker_num) \
    F(uint8_t, dgram_port_num) \
    F(uint8_t, dispatch_mode) \
    F(bool, scheduler_warning) \
    F(uint32_t, max_request) \
    F(uint32_t, max_request_grace) \
    F(SWR_SERVER_NATIVE(network::Socket) *, udp_socket_ipv4) \
    F(SWR_SERVER_NATIVE(network::Socket) *, udp_socket_ipv6) \
    F(SWR_SERVER_NATIVE(network::Socket) *, dgram_socket) \
    F(uint32_t, max_wait_time) \
    F(uint32_t, worker_max_concurrency) \
    F(bool, daemonize) \
    F(bool, have_dgram_sock) \
    F(bool, have_stream_sock) \
    F(bool, open_cpu_affinity) \
    F(bool, disable_notify) \
    F(bool, discard_timeout_request) \
    F(bool, http_parse_cookie) \
    F(bool, http_parse_post) \
    F(bool, http_parse_files) \
    F(bool, http_compression) \
    F(bool, websocket_compression) \
    F(bool, enable_static_handler) \
    F(bool, http_autoindex) \
    F(bool, enable_unsafe_event) \
    F(bool, enable_delay_receive) \
    F(bool, enable_reuse_port) \
    F(bool, reload_async) \
    F(bool, event_object) \
    F(bool, task_object) \
    F(bool, task_enable_coroutine) \
    F(bool, send_yield) \
    F(bool, enable_coroutine) \
    F(bool, single_thread) \
    F(bool, running) \
    F(int *, cpu_affinity_available) \
    F(int, cpu_affinity_available_num) \
    F(SWR_SERVER_NATIVE(UnixSocket) *, pipe_command) \
    F(double, send_timeout) \
    F(uint16_t, heartbeat_check_interval) \
    F(time_t, warning_time) \
    F(long, timezone_) \
    F(SWR_SERVER_NATIVE(TimerNode) *, master_timer) \
    F(SWR_SERVER_NATIVE(TimerNode) *, heartbeat_timer) \
    F(uint32_t, output_buffer_size) \
    F(uint32_t, input_buffer_size) \
    F(uint32_t, max_queued_bytes) \
    F(uint32_t, ipc_max_size) \
    F(void *, private_data_1) \
    F(void *, private_data_2) \
    F(void *, private_data_3) \
    F(void *, private_data_4) \
    F(SWR_SERVER_NATIVE(Factory) *, factory_) \
    F(SWR_SERVER_NATIVE(Manager) *, manager_) \
    F(uint32_t, task_worker_num) \
    F(uint8_t, task_ipc_mode) \
    F(uint32_t, task_max_request) \
    F(uint32_t, task_max_request_grace) \
    F(SWR_SERVER_NATIVE(EventData) *, task_results) \
    F(void *, worker_registry) \
    F(SWR_SERVER_NATIVE(Worker) *, user_workers) \
    F(SWR_SERVER_NATIVE(Worker) *, workers) \
    F(SWR_SERVER_NATIVE(ServerGS) *, gs) \
    F(size_t, upload_max_filesize) \
    F(uint8_t, http_compression_level) \
    F(uint32_t, compression_min_length) \
    F(const SWR_SERVER_NATIVE(Allocator) *, recv_buffer_allocator) \
    F(size_t, recv_buffer_size) \
    F(uint64_t, message_queue_key) \
    F(uint8_t, mode_)

typedef struct swr_v1_server_config {
#define SWR_SERVER_CONFIG_DECLARE(type, name) type name;
    SWR_SERVER_CONFIG_FIELDS(SWR_SERVER_CONFIG_DECLARE)
#undef SWR_SERVER_CONFIG_DECLARE
} swr_v1_server_config;

/* Server::ports: the ListenPorts in listen order, a loan of the Rust-owned list. */
typedef struct swr_v1_server_ports {
    SWR_SERVER_NATIVE(ListenPort) *const *items;
    uint32_t count;
#ifdef __cplusplus
    SWR_SERVER_NATIVE(ListenPort) *const *begin() const { return items; }
    SWR_SERVER_NATIVE(ListenPort) *const *end() const { return items + count; }
    size_t size() const { return count; }
    bool empty() const { return count == 0; }
    SWR_SERVER_NATIVE(ListenPort) *operator[](size_t i) const { return items[i]; }
    SWR_SERVER_NATIVE(ListenPort) *at(size_t i) const { return i < count ? items[i] : throw std::out_of_range("ports"); }
    SWR_SERVER_NATIVE(ListenPort) *front() const { return items[0]; }
    SWR_SERVER_NATIVE(ListenPort) *back() const { return items[count - 1]; }
#endif
} swr_v1_server_ports;

typedef struct swr_v1_server_tables {
    SWR_SERVER_NATIVE(Connection) *connection_list;
    SWR_SERVER_NATIVE(Session) *session_list;
    SWR_SERVER_NATIVE(ServerPortGS) *port_gs_list;
    uint32_t max_connection;
    SWR_SERVER_NATIVE(TimerNode) *enable_accept_timer;
    uint16_t reactor_pipe_num;
    SWR_SERVER_NATIVE(ReactorThread) *reactor_threads;
    swr_v1_server_ports ports;
    void *message_bus;
} swr_v1_server_tables;

void *swr_v1_server_session(const swr_v1_server_state *state, long id);
void *swr_v1_server_connection(const swr_v1_server_state *state, long key, uint32_t kind);
swr_v1_server_state *swr_v1_server_state_new(void);
void swr_v1_server_state_free(swr_v1_server_state *state);
/* `new Server(mode)` and `delete serv`: crates/server/src/server_object.rs allocates and frees the
   record a swoole::Server views. */
SWR_SERVER_NATIVE(Server) *swr_v1_server_new(int mode);
void swr_v1_server_delete(SWR_SERVER_NATIVE(Server) *serv);

#ifdef __cplusplus
} /* extern C */
#endif
#endif
