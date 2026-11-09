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
extern "C" {
void swr_v1_ports_set_eof(void *port, const char *eof, size_t length, bool find_from_right);
/* crates/ports/src/lifecycle_native.rs: a copy of the port's context and its reference; stores
 * `context` under `name`, taking the caller's reference. */
swoole::SSLContext const *swr_v1_ports_context_dup(const void *port);
bool swr_v1_ports_sni_add(void *port, const char *name, size_t length, const swoole::SSLContext *context);
}
namespace swoole {
struct ServerPortGS : PortCounters {
 ServerPortGS() { swr_v1_server_port_shared_init(this); }
};
/* Upstream's ListenPort, as crates/server allocates it (server_object.rs): a box of the pointer to
 * its Rust-owned settings. The inline members are those tests/modules/http/server/fixture.cc calls. */
struct ListenPort {
 swr_v1_port_config *const port_config_;
 ListenPort() = delete;
 ListenPort(const ListenPort &) = delete;
 ~ListenPort() = delete;
 swr_v1_port_config *config() const { return port_config_; }
 // The caller's reference, which ssl_add_sni_cert takes.
 const SSLContext *dup_ssl_context() const { return swr_v1_ports_context_dup(this); }
 void init_protocol();
 int get_port() const { return port_config_->port; }
 int get_fd() const { return port_config_->socket ? port_config_->socket->fd : -1; }
 bool ssl_add_sni_cert(const std::string &name, const SSLContext *ctx) {
  return swr_v1_ports_sni_add(this, name.data(), name.length(), ctx);
 }
 bool ssl_init() const;
 bool set_ssl_key_file(const std::string &file) const { return port_config_->ssl_context->set_key_file(file); }
 bool set_ssl_cert_file(const std::string &file) const { return port_config_->ssl_context->set_cert_file(file); }
 void set_ssl_cafile(const std::string &file) const { port_config_->ssl_context->set_text(SSLContext::CAFILE, file); }
 void set_ssl_verify_peer(bool value) const { port_config_->ssl_context->fields().verify_peer = value; }
 void set_ssl_allow_self_signed(bool value) const { port_config_->ssl_context->fields().allow_self_signed = value; }
 void set_ssl_protocols(long protocols) const {
  if ((protocols & SW_SSL_DTLS) && network::Socket::is_dgram(port_config_->type)) {
   protocols ^= SW_SSL_DTLS;
  }
  port_config_->ssl_context->fields().protocols = protocols;
 }
};
}
#endif
