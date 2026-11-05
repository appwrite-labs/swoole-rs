#ifndef SWOOLE_RS_SERVER_STATE_H
#define SWOOLE_RS_SERVER_STATE_H
#include <stdbool.h>
#include <stddef.h>
#include <stdint.h>
#include <time.h>

#ifdef __cplusplus
namespace swoole {
struct Connection; struct Session; struct ServerPortGS; struct ReactorThread;
struct EventData; struct Worker; struct ServerGS; struct TimerNode; struct Allocator; struct Manager;
class Factory; class UnixSocket;
namespace network { struct Socket; }
}
#define SWR_SERVER_NATIVE(type) swoole::type
extern "C" {
#else
#define SWR_SERVER_NATIVE(type) void
#endif

typedef struct swr_v1_server_state swr_v1_server_state;
typedef struct { const uint8_t *name; size_t length; int32_t id; int32_t accepted_types; } swr_v1_server_command_info;
bool swr_v1_server_command_add(const swr_v1_server_state *, const uint8_t *, size_t, int32_t, int32_t, void *);
bool swr_v1_server_command_find(const swr_v1_server_state *, const uint8_t *, size_t, swr_v1_server_command_info *);
void swr_v1_server_command_attach(const swr_v1_server_state *, int32_t, void *);
void *swr_v1_server_command_handler(const swr_v1_server_state *, int32_t);
int64_t swr_v1_server_command_request(const swr_v1_server_state *, void *);
void *swr_v1_server_command_callback(const swr_v1_server_state *, int64_t, bool);
bool swr_v1_server_command_at(const swr_v1_server_state *, size_t, swr_v1_server_command_info *);
void *swr_v1_server_command_drain(const swr_v1_server_state *, bool);
const void *swr_v1_server_static_owner(const swr_v1_server_state *, uint32_t);
const void *swr_v1_server_static_owner_clone(const void *);
void swr_v1_server_static_owner_free(const void *);
bool swr_v1_server_static_mime(const void *, const uint8_t *, size_t);
size_t swr_v1_server_static_config(const swr_v1_server_state *, uint32_t, uint32_t, const uint8_t *, size_t);
const uint8_t *swr_v1_server_static_text(const swr_v1_server_state *, uint32_t, size_t, size_t *);
void swr_v1_server_shared_init(void *storage);
void swr_v1_server_port_shared_init(void *storage);
size_t swr_v1_server_port_connections(const void *storage, uint32_t workers);
bool swr_v1_server_record_layout(uint32_t kind, size_t size, size_t alignment, const size_t *offsets, size_t count);
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
    F(int, manager_alarm) \
    F(uint64_t, message_queue_key)

typedef struct swr_v1_server_config {
#define SWR_SERVER_CONFIG_DECLARE(type, name) type name;
    SWR_SERVER_CONFIG_FIELDS(SWR_SERVER_CONFIG_DECLARE)
#undef SWR_SERVER_CONFIG_DECLARE
} swr_v1_server_config;

typedef struct swr_v1_server_tables {
    SWR_SERVER_NATIVE(Connection) *connection_list;
    SWR_SERVER_NATIVE(Session) *session_list;
    SWR_SERVER_NATIVE(ServerPortGS) *port_gs_list;
    uint32_t max_connection;
    SWR_SERVER_NATIVE(TimerNode) *enable_accept_timer;
    uint16_t reactor_pipe_num;
    SWR_SERVER_NATIVE(ReactorThread) *reactor_threads;
} swr_v1_server_tables;

int swr_v1_server_session_fd(const swr_v1_server_state *state, long id);
void *swr_v1_server_session(const swr_v1_server_state *state, long id);
void *swr_v1_server_connection(const swr_v1_server_state *state, long key, uint32_t kind);
swr_v1_server_tables *swr_v1_server_tables_get(swr_v1_server_state *state);
swr_v1_server_state *swr_v1_server_state_new(void);
void swr_v1_server_state_free(swr_v1_server_state *state);
swr_v1_server_config *swr_v1_server_config_get(swr_v1_server_state *state);
void swr_v1_server_config_init(swr_v1_server_state *state, uint32_t cpu_num, uint32_t max_sockets, long timezone, const void *allocator);

#ifdef __cplusplus
} /* extern C */
#endif
#endif
