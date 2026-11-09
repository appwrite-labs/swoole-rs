// SPDX-License-Identifier: Apache-2.0
// network::Client's C++ half (upstream's src/network/client.cc is deleted). The state,
// every operation, the callbacks, the reactor handlers, the connect timer, the socket IO and
// the input buffer are crates/clients/src/client.rs; this keeps the class's constructor,
// destructor and methods over the members of C++ types (the SSLContext reference, the proxies),
// the one table Rust reaches them through, and the C calls crates/phpclients and the Rust
// core-tests cases (tests/modules/clients/client.rs) make.
#include "swoole_string.h"
#include "swoole_socket.h"
#include "swoole_protocol.h"
#include "swoole_client.h"
#include "swoole_proxy.h"
#include "swoole_async.h"

#include "swoole_rs_clients.h"

static_assert(SW_FD_STREAM_CLIENT == 17 && SW_FD_DGRAM_CLIENT == 18 && SW_SOCK_NONBLOCK == 4 &&
                  SW_SOCK_CLOEXEC == 8 && SW_SEND_BUFFER_SIZE == 65536 && SW_SSL_BUFFER_SIZE == 16384 &&
                  SW_BUFFER_SIZE_UDP == 65536 && SW_CLIENT_BUFFER_SIZE == 65536 && SW_SOCKET_RETRY_COUNT == 10 &&
                  SW_EVENT_READ == 512 && SW_EVENT_WRITE == 1024 &&
                  SW_TIMEOUT_ALL == 15 && SW_WAIT == 2 && SW_CLOSE == 3 && SW_ERROR == 4 && SW_READY == 5 &&
                  SW_SOCK_UNIX_DGRAM == 6 && SW_SSL_STATE_READY == 1 && SW_SSL_STATE_WAIT_STREAM == 2 &&
                  SW_SOCKS5_STATE_HANDSHAKE == 1 && SW_SOCKS5_STATE_READY == 4 && SW_HTTP_PROXY_STATE_READY == 2,
              "crates/clients/src/client.rs's constants");
static_assert(SW_ERROR_DNSLOOKUP_RESOLVE_FAILED == 711 && SW_ERROR_BAD_HOST_ADDR == 722 &&
                  SW_ERROR_OUTPUT_BUFFER_OVERFLOW == 1009 && SW_ERROR_SSL_HANDSHAKE_FAILED == 1018 &&
                  SW_ERROR_SSL_CREATE_SESSION_FAILED == 1020 && SW_ERROR_SOCKET_POLL_TIMEOUT == 6002 &&
                  SW_ERROR_SOCKS5_HANDSHAKE_FAILED == 7005 && SW_ERROR_HTTP_PROXY_HANDSHAKE_ERROR == 7101,
              "crates/clients/src/client.rs's error codes");

namespace swoole {
namespace network {

static Client *of(void *cli) {
    return static_cast<Client *>(cli);
}

static void Client_onResolveCompleted(AsyncEvent *event) {
    const auto addr = swr_v1_core_string_view(swr_v1_core_resolver_view(event->data).address);
    swr_v1_clients_on_resolved(static_cast<Client *>(event->object)->state_, event->error, addr.data);
}

static const swr_v1_clients_native native = {
    swr_v1_clients_make_socket,
    swr_v1_clients_kernel_timeout,
    swr_v1_clients_assign,
    swr_v1_clients_connect_socket,
    swr_v1_clients_event,
    swr_v1_clients_socket_error,
    [](void *cli, const char *host, int domain) -> int {
        GethostbynameRequest req(host, domain);
        AsyncEvent ev{};
        ev.data = req.state_handle();
        ev.object = cli;
        ev.handler = async::handler_gethostbyname;
        ev.callback = Client_onResolveCompleted;
        return async::dispatch(&ev) == nullptr ? SW_ERR : SW_OK;
    },
    [](void *_cli, int op, int arg) -> int {
        Client *cli = of(_cli);
        Socket *socket = cli->socket;
        switch (op) {
        case SWR_CLIENTS_SSL_HAS_CONTEXT: return cli->ssl_context != nullptr;
        case SWR_CLIENTS_SSL_PREPARE:
            if (!cli->ssl_context->ready()) {
                cli->ssl_context->fields().http_v2 = cli->http2;
                if (!cli->ssl_context->create()) {
                    return SW_ERR;
                }
            }
            return SW_OK;
        case SWR_CLIENTS_SSL_SESSION:
            if (socket->ssl_create(cli->ssl_context.get(), SW_SSL_CLIENT) < 0) {
                return SW_ERR;
            }
#ifdef SSL_CTRL_SET_TLSEXT_HOSTNAME
            if (auto *name = cli->ssl_context->text(SSLContext::TLS_HOST_NAME); *name) {
                SSL_set_tlsext_host_name(socket->ssl, name);
            }
#endif
            return SW_OK;
        case SWR_CLIENTS_SSL_CONNECT: return socket->ssl_connect();
        case SWR_CLIENTS_SSL_VERIFY_PEER:
            return cli->ssl_context->fields().verify_peer | (cli->ssl_context->fields().allow_self_signed << 1);
        case SWR_CLIENTS_SSL_VERIFY: return cli->ssl_verify(arg);
        case SWR_CLIENTS_SSL_CLOSE: socket->ssl_close(); return SW_OK;
        default:
#ifdef SW_SUPPORT_DTLS
            cli->ssl_context->fields().protocols = SW_SSL_DTLS;
            return SW_OK;
#else
            return SW_ERR;
#endif
        }
    },
    [](void *cli, int proxy) -> int {
        if (proxy == SWR_CLIENTS_PROXY_SOCKS5) {
            return of(cli)->socks5_proxy ? of(cli)->socks5_proxy->state : -1;
        }
        return of(cli)->http_proxy ? of(cli)->http_proxy->state : -1;
    },
    [](void *cli, int proxy, int state) {
        if (proxy == SWR_CLIENTS_PROXY_SOCKS5) {
            of(cli)->socks5_proxy->state = state;
        } else {
            of(cli)->http_proxy->state = state;
        }
    },
    [](void *cli, int proxy, const char *host, int port, int *proxy_port) -> const char * {
        if (proxy == SWR_CLIENTS_PROXY_SOCKS5 && of(cli)->socks5_proxy) {
            of(cli)->socks5_proxy->target_host = host;
            of(cli)->socks5_proxy->target_port = port;
            *proxy_port = of(cli)->socks5_proxy->port;
            return of(cli)->socks5_proxy->host.c_str();
        }
        if (proxy == SWR_CLIENTS_PROXY_HTTP && of(cli)->http_proxy) {
            of(cli)->http_proxy->target_host = host;
            of(cli)->http_proxy->target_port = port;
            *proxy_port = of(cli)->http_proxy->port;
            return of(cli)->http_proxy->host.c_str();
        }
        return nullptr;
    },
    [](void *cli, const char **data) -> ssize_t {
        *data = of(cli)->socks5_proxy->buf;
        return of(cli)->socks5_proxy->pack_negotiate_request();
    },
    [](void *cli, const char *data, size_t length) -> bool { return of(cli)->socks5_handshake(data, length); },
    [](void *cli, const char **data) -> size_t {
        auto proxy_buf = sw_tg_buffer();
        auto host_name = of(cli)->get_http_proxy_host_name();
        size_t n = swr_v1_http_proxy_connect(of(cli)->http_proxy->handle(), host_name.data(), host_name.size(),
                                             proxy_buf->str, proxy_buf->size);
        swoole_trace_log(SW_TRACE_HTTP_CLIENT, "proxy request: <<EOF\n%.*sEOF", (int) n, proxy_buf->str);
        *data = proxy_buf->str;
        return n;
    },
    [](void *cli, bool shared, size_t length) -> bool {
        String *buffer = shared ? sw_tg_buffer() : of(cli)->buffer;
        buffer->length = shared ? length : buffer->length + length;
        return swr_v1_http_proxy_accept(buffer->str, buffer->length);
    },
};

Client::Client(SocketType _type, bool _async) : state_(swr_v1_clients_new(this, &native, this, _type, _async)) {
    if (socket == nullptr) {
        swoole_sys_warning("socket() failed");
        return;
    }
}

Client::~Client() {
    swr_v1_clients_free(state_);
}

int Client::connect(const char *_host, int _port, double _timeout, int _sock_flag) {
    return swr_v1_clients_connect(state_, _host, _port, _timeout, _sock_flag);
}

ssize_t Client::send(const char *_data, size_t _length, int _flags) {
    return swr_v1_clients_send(state_, _data, _length, _flags);
}

int Client::sendfile(const char *_filename, off_t _offset, size_t _length) {
    return swr_v1_clients_sendfile(state_, _filename, _offset, _length);
}

ssize_t Client::recv(char *_data, size_t _length, int _flags) {
    return swr_v1_clients_recv(state_, _data, _length, _flags);
}

int Client::close() {
    return swr_v1_clients_close(state_);
}

int Client::shutdown(int _how) {
    return swr_v1_clients_shutdown(state_, _how);
}

int Client::sleep() {
    return swr_v1_clients_sleep(state_);
}

int Client::wakeup() {
    return swr_v1_clients_wakeup(state_);
}

int Client::ssl_handshake() const {
    return swr_v1_clients_ssl_handshake(state_);
}

int Client::bind(const std::string &addr, int port) const {
    return swr_v1_clients_bind(state_, addr.c_str(), addr.length(), port, [](int fd) {
        swoole_sys_warning("setsockopt(%d, SO_REUSEADDR) failed", fd);
    });
}

void Client::set_timeout(double timeout, TimeoutType type) const {
    socket->set_timeout(timeout, type);
}

bool Client::has_timedout() const {
    return socket->has_timedout();
}

void Client::set_socks5_proxy(const std::string &host, int port, const std::string &user, const std::string &pwd) {
    socks5_proxy.reset(Socks5Proxy::create(get_socket_type(), host, port, user, pwd));
}

void Client::set_http_proxy(const std::string &host, int port, const std::string &user, const std::string &pwd) {
    http_proxy.reset(new HttpProxy());
    swr_v1_http_proxy_configure(
        http_proxy->handle(), host.data(), host.size(), port, user.data(), user.size(), pwd.data(), pwd.size());
}

int Client::sendto(const std::string &host, int port, const char *data, size_t len) const {
    return swr_v1_clients_sendto(state_, host.c_str(), host.length(), port, data, len, [](int) {
        swoole_warning("only supports SWOOLE_SOCK_(UDP/UDP6/UNIX_DGRAM)");
    });
}

int Client::get_peer_name(Address *addr) const {
    return swr_v1_clients_get_peer_name(state_, addr);
}

bool Client::socks5_handshake(const char *recv_data, size_t length) {
    return socks5_proxy->handshake(recv_data, length, [this](const char *buf, size_t len) { return send(buf, len); });
}

int Client::enable_ssl_encrypt() {
    if (ssl_context) {
        return SW_ERR;
    }
    ssl_context.reset(SSLContext::make());
#ifdef SW_SUPPORT_DTLS
    return swr_v1_clients_enable_ssl(state_);
#else
    set_flag(SWR_CLIENTS_OPEN_SSL, true);
    swoole_warning("DTLS support require openssl-1.1 or later");
    return SW_ERR;
#endif
}

int Client::ssl_verify(int allow_self_signed) const {
    if (!socket->ssl_verify(allow_self_signed)) {
        return SW_ERR;
    }
#ifdef SSL_CTRL_SET_TLSEXT_HOSTNAME
    auto *name = ssl_context->text(SSLContext::TLS_HOST_NAME);
    if (*name && !socket->ssl_check_host(name)) {
        return SW_ERR;
    }
#endif
    return SW_OK;
}

}  // namespace network
}  // namespace swoole

using swoole::network::Client;

void *swoole_client_create(int type, bool async, int *error) {
    auto *cli = new Client(static_cast<swoole::SocketType>(type), async);
    if (cli->socket == nullptr) {
        *error = errno;
        delete cli;
        errno = *error;
        return nullptr;
    }
    return cli;
}

void swoole_client_free(void *cli) {
    delete static_cast<Client *>(cli);
}

void *swoole_client_state(void *cli) {
    return static_cast<Client *>(cli)->state_;
}

const swr_v1_tls_owner *swoole_client_ssl_context(void *cli) {
    auto *context = static_cast<Client *>(cli)->ssl_context.get();
    return context ? context->handle() : nullptr;
}

void swoole_client_proxy(void *_cli,
                         bool http,
                         const char *host,
                         size_t host_length,
                         int64_t port,
                         const char *user,
                         size_t user_length,
                         const char *password,
                         size_t password_length) {
    auto *cli = static_cast<Client *>(_cli);
    const std::string h(host, host_length), u(user, user_length), p(password, password_length);
    if (http) {
        cli->set_http_proxy(h, port, u, p);
    } else {
        cli->set_socks5_proxy(h, port, u, p);
    }
}
