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
swr_v1_port_config *swr_v1_port_config_new(int);
void swr_v1_ports_set_eof(void *port, const char *eof, size_t length, bool find_from_right);
void swr_v1_port_config_free(swr_v1_port_config *);
/* crates/ports/src/lifecycle_native.rs: a copy of the port's context and its reference; stores
 * `context` under `name`, taking the caller's reference; releases the port's context and TLS state. */
swoole::SSLContext const *swr_v1_ports_context_dup(const void *port);
bool swr_v1_ports_sni_add(void *port, const char *name, size_t length, const swoole::SSLContext *context);
void swr_v1_ports_tls_free(swr_v1_port_config *);
}
namespace swoole { namespace detail {
class PortFields {
 protected:
 swr_v1_port_config *port_config_;
 public:
#define SWR_PORT_REF(type,name) decltype(swr_v1_port_config::name) &name;
 SWR_PORT_CONFIG_FIELDS(SWR_PORT_REF)
#undef SWR_PORT_REF
 explicit PortFields(int buffer_size = network::Socket::default_buffer_size): port_config_(swr_v1_port_config_new(buffer_size))
#define SWR_PORT_BIND(type,name) ,name(port_config_->name)
 SWR_PORT_CONFIG_FIELDS(SWR_PORT_BIND)
#undef SWR_PORT_BIND
 { const size_t offsets[] = {offsetof(swr_v1_port_config,object_id),offsetof(swr_v1_port_config,backlog),offsetof(swr_v1_port_config,listening),offsetof(swr_v1_port_config,tcp_defer_accept),offsetof(swr_v1_port_config,tcp_fastopen),offsetof(swr_v1_port_config,tcp_keepidle),offsetof(swr_v1_port_config,tcp_keepinterval),offsetof(swr_v1_port_config,tcp_keepcount),offsetof(swr_v1_port_config,tcp_user_timeout),offsetof(swr_v1_port_config,max_idle_time),offsetof(swr_v1_port_config,socket_buffer_size),offsetof(swr_v1_port_config,buffer_high_watermark),offsetof(swr_v1_port_config,buffer_low_watermark),offsetof(swr_v1_port_config,type),offsetof(swr_v1_port_config,ssl),offsetof(swr_v1_port_config,port),offsetof(swr_v1_port_config,socket),offsetof(swr_v1_port_config,thread_id),offsetof(swr_v1_port_config,heartbeat_idle_time),offsetof(swr_v1_port_config,open_eof_check),offsetof(swr_v1_port_config,open_http_protocol),offsetof(swr_v1_port_config,open_http2_protocol),offsetof(swr_v1_port_config,open_websocket_protocol),offsetof(swr_v1_port_config,open_length_check),offsetof(swr_v1_port_config,open_mqtt_protocol),offsetof(swr_v1_port_config,open_redis_protocol),offsetof(swr_v1_port_config,open_tcp_nodelay),offsetof(swr_v1_port_config,open_tcp_nopush),offsetof(swr_v1_port_config,open_tcp_keepalive),offsetof(swr_v1_port_config,kernel_socket_recv_buffer_size),offsetof(swr_v1_port_config,kernel_socket_send_buffer_size),offsetof(swr_v1_port_config,gs),offsetof(swr_v1_port_config,ptr),offsetof(swr_v1_port_config,websocket_frames),offsetof(swr_v1_port_config,websocket_protocol),offsetof(swr_v1_port_config,host_slot),offsetof(swr_v1_port_config,protocol),offsetof(swr_v1_port_config,onRead),offsetof(swr_v1_port_config,ssl_context),offsetof(swr_v1_port_config,tls)};
 if (!swr_v1_server_record_layout(4,sizeof(swr_v1_port_config),alignof(swr_v1_port_config),offsets,sizeof(offsets)/sizeof(size_t))) std::abort(); }
 swr_v1_port_config *config_handle() const { return port_config_; }
 ~PortFields() { swr_v1_ports_tls_free(port_config_); swr_v1_port_config_free(port_config_); }
 PortFields(const PortFields &)=delete;
 PortFields &operator=(const PortFields &)=delete;
};
}
struct ServerPortGS : PortCounters {
 ServerPortGS() { swr_v1_server_port_shared_init(this); }
};
/* Upstream's ListenPort: its state is the Rust-owned PortFields and its other members are crates/ports
 * exports. The inline members are those native/ and tests/modules/http/server/fixture.cc still call. */
struct ListenPort : detail::PortFields {
 RustStringView host{port_config_->host_slot};
 explicit ListenPort(Server *server) { protocol.private_data_2 = server; }
 // The caller's reference, which ssl_add_sni_cert takes.
 const SSLContext *dup_ssl_context() const { return swr_v1_ports_context_dup(this); }
 void init_protocol();
 const char *get_protocols() const;
 size_t get_connection_num() const;
 int get_port() const { return port; }
 int get_fd() const { return socket ? socket->fd : -1; }
 bool ssl_add_sni_cert(const std::string &name, const SSLContext *ctx) {
  return swr_v1_ports_sni_add(this, name.data(), name.length(), ctx);
 }
 bool ssl_init() const;
 bool set_ssl_key_file(const std::string &file) const { return ssl_context->set_key_file(file); }
 bool set_ssl_cert_file(const std::string &file) const { return ssl_context->set_cert_file(file); }
 void set_ssl_cafile(const std::string &file) const { ssl_context->set_text(SSLContext::CAFILE, file); }
 void set_ssl_verify_peer(bool value) const { ssl_context->fields().verify_peer = value; }
 void set_ssl_allow_self_signed(bool value) const { ssl_context->fields().allow_self_signed = value; }
 void set_ssl_protocols(long protocols) const {
  if ((protocols & SW_SSL_DTLS) && network::Socket::is_dgram(type)) {
   protocols ^= SW_SSL_DTLS;
  }
  ssl_context->fields().protocols = protocols;
 }
};
}
#endif
