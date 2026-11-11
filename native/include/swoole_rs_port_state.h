#ifndef SWOOLE_RS_PORT_STATE_H
#define SWOOLE_RS_PORT_STATE_H
#include "swoole_rs_server_state.h"
#include "swoole_rs_corebase_string_slot.h"
#include <cstdlib>
namespace swoole { class Server; struct ServerPortGS; struct SSLContext; class Reactor; struct Event; struct ListenPort;
typedef int (*PortReadFn)(Reactor *, ListenPort *, Event *); }
#define SWR_PORT_CONFIG_FIELDS(X) \
    X(uint16_t, object_id) \
    X(uint16_t, backlog) \
    X(bool, listening) \
    X(int, tcp_defer_accept) \
    X(int, tcp_fastopen) \
    X(int, tcp_keepidle) \
    X(int, tcp_keepinterval) \
    X(int, tcp_keepcount) \
    X(int, tcp_user_timeout) \
    X(double, max_idle_time) \
    X(int, socket_buffer_size) \
    X(uint32_t, buffer_high_watermark) \
    X(uint32_t, buffer_low_watermark) \
    X(swoole::SocketType, type) \
    X(uint8_t, ssl) \
    X(int, port) \
    X(swoole::network::Socket *, socket) \
    X(pthread_t, thread_id) \
    X(uint16_t, heartbeat_idle_time) \
    X(bool, open_eof_check) \
    X(bool, open_http_protocol) \
    X(bool, open_http2_protocol) \
    X(bool, open_websocket_protocol) \
    X(bool, open_length_check) \
    X(bool, open_mqtt_protocol) \
    X(bool, open_redis_protocol) \
    X(bool, open_tcp_nodelay) \
    X(bool, open_tcp_nopush) \
    X(bool, open_tcp_keepalive) \
    X(int, kernel_socket_recv_buffer_size) \
    X(int, kernel_socket_send_buffer_size) \
    X(swoole::ServerPortGS *, gs) \
    X(void *, ptr) \
    X(uint8_t, websocket_frames) \
    X(void *, websocket_protocol) \
    X(const swr_v1_core_string *, host_slot) \
    X(swoole::Protocol, protocol) \
    X(swoole::PortReadFn, onRead) \
    X(swoole::SSLContext *, ssl_context) \
    X(void *, tls)
struct swr_v1_port_config {
#define SWR_PORT_FIELD(type,name) type name;
 SWR_PORT_CONFIG_FIELDS(SWR_PORT_FIELD)
#undef SWR_PORT_FIELD
};
namespace swoole {
struct ServerPortGS : PortCounters {
 ServerPortGS() { swr_v1_server_port_shared_init(this); }
};
/* Upstream's ListenPort, as crates/server allocates it (server_object.rs): the box of its
 * Rust-owned settings. */
struct ListenPort {
 ListenPort() = delete;
 ListenPort(const ListenPort &) = delete;
 ~ListenPort() = delete;
 swr_v1_port_config *config() const {
  return reinterpret_cast<swr_v1_port_config *>(const_cast<ListenPort *>(this));
 }
};
}
#endif
