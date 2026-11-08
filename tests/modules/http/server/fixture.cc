// The Servers upstream's core-tests/src/server/http_server.cpp builds (test_base_server,
// test_http_server, test_ssl_http), for the Rust cases in src/main.rs. Building a Server, its
// std::string setters, its callback slots and a timer callback need C++; every assertion and
// callback body is Rust's. Compiled into the hybrid tree's lib-swoole by tests/modules/hooks/system.py.
#include "swoole_server.h"
#include "swoole_timer.h"
#include "swoole_websocket.h"

using namespace swoole;

typedef int (*Receive)(Server *, RecvData *);
typedef void (*Closed)(Server *, DataHead *);
typedef void (*Hook)(Server *);

static ListenPort *listen_port(Server *serv, int type, int port) {
    ListenPort *ls = serv->add_port(static_cast<SocketType>(type), "127.0.0.1", port);
    if (!ls) {
        swoole_warning("listen failed, [error=%d]", swoole_get_last_error());
        exit(2);
    }
    return ls;
}

extern "C" {
// test_base_server's Server, created: null when set_document_root fails.
Server *swr_case_base_server(const char *root, Receive on_receive, Closed on_close, Hook on_worker_start) {
    auto serv = new Server(Server::MODE_BASE);
    serv->worker_num = 1;
    serv->enable_reuse_port = true;
    serv->heartbeat_check_interval = 1;
    serv->enable_static_handler = true;
    if (!serv->set_document_root(root)) {
        return nullptr;
    }
    serv->locations.insert("/examples");
    serv->add_http_compression_type("text/html");
    swoole_set_log_level(SW_LOG_WARNING);
    ListenPort *port = listen_port(serv, SW_SOCK_TCP, 0);
    port->open_http_protocol = true;
    port->open_websocket_protocol = true;
    serv->create();
    serv->onWorkerStart = [on_worker_start](Server *s, Worker *) { on_worker_start(s); };
    serv->onClose = on_close;
    serv->onReceive = on_receive;
    return serv;
}

// test_http_server's Server, created.
Server *swr_case_http_server(int dispatch_mode, bool ssl, int worker_num, int mode, const char *root,
                             Receive on_receive, Closed on_close) {
    auto serv = new Server(static_cast<Server::Mode>(mode));
    serv->user_ = std::string("root");
    serv->group_ = std::string("root");
    serv->chroot_ = std::string("/");
    serv->worker_num = worker_num;
    serv->dispatch_mode = dispatch_mode;
    serv->open_cpu_affinity = true;
    swoole_set_log_level(SW_LOG_WARNING);
    ListenPort *port = listen_port(serv, ssl ? (SW_SOCK_TCP | SW_SOCK_SSL) : SW_SOCK_TCP, 0);
    port->open_http_protocol = true;
    port->open_websocket_protocol = true;
    port->open_tcp_keepalive = true;
    port->tcp_fastopen = true;
    port->tcp_defer_accept = true;
    serv->enable_static_handler = true;
    serv->set_document_root(root);
    serv->locations.insert("/examples");
    serv->create();
    serv->onStart = [](Server *) {};
    serv->onConnect = [](Server *, DataHead *) {};
    serv->onClose = on_close;
    serv->onReceive = on_receive;
    return serv;
}

// test_ssl_http's Server on an SSL port: SW_OK from create, or its error.
int swr_case_ssl_server(Server **out, int mode, int port_number, const char *cert, const char *key, Hook on_start,
                        Hook on_worker_start, Receive on_receive) {
    auto serv = *out = new Server(static_cast<Server::Mode>(mode));
    serv->worker_num = 1;
    swoole_set_log_level(SW_LOG_INFO);
    ListenPort *port = listen_port(serv, SW_SOCK_TCP | SW_SOCK_SSL, port_number);
    port->open_http_protocol = 1;
    port->set_ssl_cert_file(cert);
    port->set_ssl_key_file(key);
    port->ssl_context->http = 1;
    port->ssl_init();
    int created = serv->create();
    serv->onStart = on_start;
    serv->onWorkerStart = [on_worker_start](Server *s, Worker *) { on_worker_start(s); };
    serv->onReceive = on_receive;
    return created;
}

int swr_case_port(Server *serv) {
    return serv->get_primary_port()->get_port();
}
int swr_case_start(Server *serv) {
    return serv->start();
}
bool swr_case_shutdown(Server *serv) {
    return serv->shutdown();
}
pid_t swr_case_master_pid(Server *serv) {
    return serv->get_master_pid();
}
bool swr_case_send(Server *serv, SessionId session_id, const char *data, uint32_t length) {
    return serv->send(session_id, data, length);
}
bool swr_case_close(Server *serv, SessionId session_id) {
    return serv->close(session_id);
}
bool swr_case_sendfile(Server *serv, SessionId session_id, const char *file) {
    return serv->sendfile(session_id, file, strlen(file), 0, 0);
}
String *swr_case_tg_buffer() {
    return sw_tg_buffer();
}

// The request header fields a callback reads.
SessionId swr_case_info(const DataHead *info, int *reactor_id, uint32_t *len, uint16_t *ext_flags) {
    *reactor_id = info->reactor_id;
    *len = info->len;
    *ext_flags = info->ext_flags;
    return info->fd;
}
const char *swr_case_data(const RecvData *req) {
    return req->data;
}
uint8_t *swr_case_websocket_status(Server *serv, SessionId session_id) {
    return &serv->get_connection_by_session_id(session_id)->websocket_status;
}
bool swr_case_close_actively(Server *serv, SessionId session_id) {
    return serv->get_connection_by_session_id(session_id)->close_actively;
}
bool swr_case_healthy(Server *serv, SessionId session_id) {
    return serv->is_healthy_connection(microtime(), serv->get_connection_by_session_id(session_id));
}
uint32_t swr_case_idle_worker_num(Server *serv) {
    return serv->get_idle_worker_num();
}
uint32_t swr_case_worker_num(Server *serv) {
    return serv->worker_num;
}

// The `/ws/close` handshake's timer: a policy close frame after `ms`.
void swr_case_close_after(Server *serv, SessionId session_id, long ms) {
    swoole_timer_after(ms, [serv, session_id](Timer *, TimerNode *) {
        sw_tg_buffer()->clear();
        websocket::pack_close_frame(sw_tg_buffer(), websocket::CLOSE_POLICY_ERROR, SW_STRL("swoole close"), 0);
        serv->send(session_id, sw_tg_buffer()->str, sw_tg_buffer()->length);
    });
}

// The primary port's TLS settings.
void swr_case_ssl_files(Server *serv, const char *cert, const char *key) {
    serv->get_primary_port()->set_ssl_cert_file(cert);
    serv->get_primary_port()->set_ssl_key_file(key);
}
void swr_case_ssl_sni(Server *serv, const char *name, const char *cert, const char *key) {
    ListenPort *port = serv->get_primary_port();
    auto sni_ssl_ctx = port->dup_ssl_context();
    sni_ssl_ctx->set_cert_file(cert);
    sni_ssl_ctx->set_key_file(key);
    port->ssl_add_sni_cert(name, sni_ssl_ctx);
}
void swr_case_ssl_protocols(Server *serv, long protocols) {
    serv->get_primary_port()->set_ssl_protocols(protocols);
}
void swr_case_ssl_client_ca(Server *serv, const char *cafile) {
    ListenPort *port = serv->get_primary_port();
    port->set_ssl_verify_peer(true);
    port->set_ssl_allow_self_signed(true);
    port->set_ssl_cafile(cafile);
}
bool swr_case_ssl_init(Server *serv) {
    return serv->get_primary_port()->ssl_init();
}
}
