// SPDX-License-Identifier: Apache-2.0
// network::Client's C++ half (upstream's src/network/client.cc is deleted). The state and
// every operation are crates/clients/src/client.rs; this keeps what Rust can't express: the
// class's constructor, destructor and methods over the members of C++ types (std::function
// callbacks, shared_ptr<SSLContext>, the proxies, String buffers, AsyncEvent), the C++
// exception a callback throws, the one table Rust reaches them through, and the C calls
// crates/phpclients and the Rust core-tests cases (tests/modules/clients/client.rs) make.
#include "swoole_string.h"
#include "swoole_socket.h"
#include "swoole_timer.h"
#include "swoole_protocol.h"
#include "swoole_client.h"
#include "swoole_proxy.h"
#include "swoole_async.h"

#include <exception>
#include "swoole_rs_clients.h"

static_assert(SW_FD_STREAM_CLIENT == 17 && SW_FD_DGRAM_CLIENT == 18 && SW_SOCK_NONBLOCK == 4 &&
                  SW_SOCK_CLOEXEC == 8 && SW_SEND_BUFFER_SIZE == 65536 && SW_SSL_BUFFER_SIZE == 16384 &&
                  SW_BUFFER_SIZE_UDP == 65536 && SW_EVENT_READ == 512 && SW_EVENT_WRITE == 1024 &&
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

// A callback that throws stops the Rust operation; the exception is rethrown once it returns.
static thread_local std::exception_ptr client_thrown;

template <typename T>
static T rethrown(T result) {
    if (client_thrown) {
        std::rethrow_exception(std::exchange(client_thrown, nullptr));
    }
    return result;
}

static Client *of(void *cli) {
    return static_cast<Client *>(cli);
}

// The warnings come from functions named as upstream's, which the log lines name.
static void Client_tcp_connect_async(const Client *) {
    swoole_warning("onConnect/onError/onReceive/onClose callback have not set");
}

static void Client_udp_connect(const Client *cli, int which) {
    switch (which) {
    case SWR_CLIENTS_WARN_ON_RECEIVE:
        swoole_warning("`onReceive` callback have not set");
        break;
    case SWR_CLIENTS_WARN_ON_ERROR:
        swoole_warning("`onError` callback have not set");
        break;
    case SWR_CLIENTS_WARN_BIND:
        swoole_sys_warning("bind(%s) failed", cli->socket->info.addr.un.sun_path);
        break;
    default:
        swoole_warning("DTLS support require openssl-1.1 or later");
        break;
    }
}

static void Client_onStreamRead(const Client *cli) {
    swoole_sys_warning("Read from socket[%d] failed", cli->socket->fd);
}

static void Client_onWrite(const Client *cli) {
    swoole_sys_warning("getsockopt(%d) failed", cli->socket->fd);
}

static void Client_onTimeout(Timer *, TimerNode *tnode) {
    swr_v1_clients_on_timeout(static_cast<Client *>(tnode->data)->state_);
    rethrown(0);
}

static void Client_onResolveCompleted(AsyncEvent *event) {
    auto *req = dynamic_cast<GethostbynameRequest *>(event->data.get());
    swr_v1_clients_on_resolved(static_cast<Client *>(event->object)->state_, event->error, req->addr.c_str());
    rethrown(0);
}

static int Client_onPackage(const Protocol *, Socket *conn, const RecvData *rdata) {
    auto *cli = static_cast<Client *>(conn->object);
    try {
        cli->onReceive(cli, rdata->data, rdata->info.len);
    } catch (...) {
        client_thrown = std::current_exception();
        return SW_ERR;
    }
    return conn->close_wait ? SW_ERR : SW_OK;
}

static const std::function<void(Client *)> &callback(const Client *cli, int which) {
    switch (which) {
    case SWR_CLIENTS_ON_CONNECT: return cli->onConnect;
    case SWR_CLIENTS_ON_ERROR: return cli->onError;
    case SWR_CLIENTS_ON_CLOSE: return cli->onClose;
    case SWR_CLIENTS_ON_BUFFER_FULL: return cli->onBufferFull;
    default: return cli->onBufferEmpty;
    }
}

static const swr_v1_clients_native native = {
    swr_v1_clients_make_socket,
    [](void *cli, bool deferred) {
        if (deferred) {
            of(cli)->socket->free();
        } else {
            delete of(cli)->socket;
        }
    },
    [](void *cli, int which) -> bool {
        return which == SWR_CLIENTS_ON_RECEIVE ? bool(of(cli)->onReceive) : bool(callback(of(cli), which));
    },
    [](void *cli, int which, const char *data, size_t length) -> bool {
        try {
            if (which == SWR_CLIENTS_ON_RECEIVE) {
                of(cli)->onReceive(of(cli), data, length);
            } else {
                callback(of(cli), which)(of(cli));
            }
            return true;
        } catch (...) {
            client_thrown = std::current_exception();
            return false;
        }
    },
    [](void *cli, int which) {
        switch (which) {
        case SWR_CLIENTS_WARN_CALLBACKS: Client_tcp_connect_async(of(cli)); break;
        case SWR_CLIENTS_WARN_READ: Client_onStreamRead(of(cli)); break;
        case SWR_CLIENTS_WARN_GETSOCKOPT: Client_onWrite(of(cli)); break;
        default: Client_udp_connect(of(cli), which); break;
        }
    },
    swr_v1_clients_kernel_timeout,
    swr_v1_clients_assign,
    swr_v1_clients_connect_socket,
    [](void *cli, int op, char *data, size_t length, int flags) -> ssize_t {
        Socket *socket = of(cli)->socket;
        switch (op) {
        case SWR_CLIENTS_IO_RECV: return socket->recv(data, length, flags);
        case SWR_CLIENTS_IO_RECV_SYNC: return socket->recv_sync(data, length, flags);
        case SWR_CLIENTS_IO_SEND_SYNC: return socket->send_sync(data, length, flags);
        case SWR_CLIENTS_IO_SENDTO: return socket->sendto(of(cli)->server_addr, data, length, flags);
        case SWR_CLIENTS_IO_RECVFROM: return socket->recvfrom(data, length, flags, &of(cli)->remote_addr);
        default: return socket->recvfrom_sync(data, length, flags, &of(cli)->remote_addr);
        }
    },
    [](void *cli, const char *file, int64_t offset, size_t length, bool is_async) -> int {
        Socket *socket = of(cli)->socket;
        if ((is_async ? socket->sendfile_async(file, offset, length) : socket->sendfile_sync(file, offset, length)) <
            0) {
            swoole_set_last_error(errno);
            return SW_ERR;
        }
        return SW_OK;
    },
    swr_v1_clients_event,
    [](void *cli, const char *data, size_t length) -> int {
        return swoole_event_write(of(cli)->socket, data, length);
    },
    [](void *cli) -> uint32_t { return of(cli)->socket->get_out_buffer_length(); },
    [](void *reactor, void *event) -> int {
        return Reactor::_writable_callback(static_cast<Reactor *>(reactor), static_cast<Event *>(event));
    },
    swr_v1_clients_socket_error,
    [](void *cli, double timeout) -> void * { return swoole_timer_add(timeout, false, Client_onTimeout, cli); },
    [](void *timer) { swoole_timer_del(static_cast<TimerNode *>(timer)); },
    [](void *cli, const char *host, int domain) -> int {
        AsyncEvent ev{};
        ev.data = std::make_shared<GethostbynameRequest>(host, domain);
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
                cli->ssl_context->http_v2 = cli->http2;
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
            if (!cli->ssl_context->tls_host_name.empty()) {
                SSL_set_tlsext_host_name(socket->ssl, cli->ssl_context->tls_host_name.c_str());
            }
#endif
            return SW_OK;
        case SWR_CLIENTS_SSL_CONNECT: return socket->ssl_connect();
        case SWR_CLIENTS_SSL_VERIFY_PEER:
            return cli->ssl_context->verify_peer | (cli->ssl_context->allow_self_signed << 1);
        case SWR_CLIENTS_SSL_VERIFY: return cli->ssl_verify(arg);
        case SWR_CLIENTS_SSL_CLOSE: socket->ssl_close(); return SW_OK;
        default:
#ifdef SW_SUPPORT_DTLS
            cli->ssl_context->protocols = SW_SSL_DTLS;
            return SW_OK;
#else
            Client_udp_connect(cli, -1);
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
    [](void *cli, int op, size_t length, swr_v1_clients_buffer_view *view) {
        String *&buffer = of(cli)->buffer;
        switch (op) {
        case SWR_CLIENTS_BUFFER_INIT:
            if (!buffer) {
                buffer = new String(SW_CLIENT_BUFFER_SIZE);
            }
            break;
        case SWR_CLIENTS_BUFFER_GROW: buffer->length += length; break;
        case SWR_CLIENTS_BUFFER_CLEAR: buffer->clear(); break;
        case SWR_CLIENTS_BUFFER_FREE:
            delete buffer;
            buffer = nullptr;
            break;
        default: {
            String *viewed = op == SWR_CLIENTS_BUFFER_SHARED ? sw_tg_buffer() : buffer;
            *view = {viewed->str, viewed->length, viewed->size};
            break;
        }
        }
    },
    [](void *_cli, bool eof) -> ssize_t {
        Client *cli = of(_cli);
        Socket *socket = cli->socket;
        const ssize_t n = eof ? cli->protocol.recv_with_eof_protocol(socket, cli->buffer)
                              : cli->protocol.recv_with_length_protocol(socket, cli->buffer);
        // A callback may delete the client; only its deferred socket remains.
        return socket->object == nullptr || client_thrown ? 0 : n;
    },
};

static int Client_onStreamRead(Reactor *, Event *event) {
    return rethrown(swr_v1_clients_on_read(static_cast<Client *>(event->socket->object)->state_));
}

static int Client_onDgramRead(Reactor *, Event *event) {
    return rethrown(swr_v1_clients_on_dgram_read(static_cast<Client *>(event->socket->object)->state_));
}

static int Client_onWrite(Reactor *reactor, Event *event) {
    return rethrown(swr_v1_clients_on_write(static_cast<Client *>(event->socket->object)->state_, reactor, event));
}

static int Client_onError(Reactor *reactor, Event *event) {
    return rethrown(swr_v1_clients_on_error(static_cast<Client *>(event->socket->object)->state_, reactor, event));
}

void Client::init_reactor(Reactor *reactor) {
    reactor->set_handler(SW_FD_STREAM_CLIENT, SW_EVENT_READ, Client_onStreamRead);
    reactor->set_handler(SW_FD_DGRAM_CLIENT, SW_EVENT_READ, Client_onDgramRead);
    reactor->set_handler(SW_FD_STREAM_CLIENT, SW_EVENT_WRITE, Client_onWrite);
    reactor->set_handler(SW_FD_STREAM_CLIENT, SW_EVENT_ERROR, Client_onError);
}

Client::Client(SocketType _type, bool _async) : state_(swr_v1_clients_new(this, &native, this, reinterpret_cast<void *>(Client_onPackage), _type, _async)) {
    if (socket == nullptr) {
        swoole_sys_warning("socket() failed");
        return;
    }
}

Client::~Client() {
    swr_v1_clients_free(state_);
}

int Client::connect(const char *_host, int _port, double _timeout, int _sock_flag) {
    return rethrown(swr_v1_clients_connect(state_, _host, _port, _timeout, _sock_flag));
}

ssize_t Client::send(const char *_data, size_t _length, int _flags) {
    return rethrown(swr_v1_clients_send(state_, _data, _length, _flags));
}

int Client::sendfile(const char *_filename, off_t _offset, size_t _length) {
    return swr_v1_clients_sendfile(state_, _filename, _offset, _length);
}

ssize_t Client::recv(char *_data, size_t _length, int _flags) {
    return swr_v1_clients_recv(state_, _data, _length, _flags);
}

int Client::close() {
    return rethrown(swr_v1_clients_close(state_));
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
    // Rust's send, not send(): a throw here must not cross the Rust frame that called this.
    auto send_fn = [this](const char *buf, size_t len) { return swr_v1_clients_send(state_, buf, len, 0); };
    return socks5_proxy->handshake(recv_data, length, send_fn);
}

int Client::enable_ssl_encrypt() {
    if (ssl_context) {
        return SW_ERR;
    }
    ssl_context = std::make_shared<SSLContext>();
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
    if (!ssl_context->tls_host_name.empty() && !socket->ssl_check_host(ssl_context->tls_host_name.c_str())) {
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

static void client_event(Client *cli, int event, const char *data, size_t length) {
    cli->listener(cli->object, event, data, length);
}

void swoole_client_async_listen(void *_cli, void *object, uint32_t events, swoole_client_listener listener) {
    auto *cli = static_cast<Client *>(_cli);
    cli->object = object;
    cli->listener = listener;
    if (events & (1u << SWOOLE_CLIENT_ON_CONNECT)) {
        cli->onConnect = [](Client *cli) { client_event(cli, SWOOLE_CLIENT_ON_CONNECT, nullptr, 0); };
    }
    if (events & (1u << SWOOLE_CLIENT_ON_RECEIVE)) {
        cli->onReceive = [](Client *cli, const char *data, size_t length) {
            client_event(cli, SWOOLE_CLIENT_ON_RECEIVE, data, length);
        };
    }
    if (events & (1u << SWOOLE_CLIENT_ON_CLOSE)) {
        cli->onClose = [](Client *cli) { client_event(cli, SWOOLE_CLIENT_ON_CLOSE, nullptr, 0); };
    }
    if (events & (1u << SWOOLE_CLIENT_ON_ERROR)) {
        cli->onError = [](Client *cli) { client_event(cli, SWOOLE_CLIENT_ON_ERROR, nullptr, swoole_get_last_error()); };
    }
    if (events & (1u << SWOOLE_CLIENT_ON_BUFFER_FULL)) {
        cli->onBufferFull = [](Client *cli) { client_event(cli, SWOOLE_CLIENT_ON_BUFFER_FULL, nullptr, 0); };
    }
    if (events & (1u << SWOOLE_CLIENT_ON_BUFFER_EMPTY)) {
        cli->onBufferEmpty = [](Client *cli) { client_event(cli, SWOOLE_CLIENT_ON_BUFFER_EMPTY, nullptr, 0); };
    }
}

bool swoole_client_has_on_error(void *cli) {
    return static_cast<Client *>(cli)->onError != nullptr;
}

const swr_v1_tls_owner *swoole_client_ssl_context(void *cli) {
    auto &context = static_cast<Client *>(cli)->ssl_context;
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
