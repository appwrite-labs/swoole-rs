// SPDX-License-Identifier: Apache-2.0
// network::Client's C++ half (upstream's src/network/client.cc is deleted). The state,
// every operation, the callbacks, the reactor handlers, the connect timer, the socket IO and
// the input buffer are crates/clients/src/client.rs; this keeps the class's constructor,
// destructor and method forwarders, the DNS lookup Rust reaches through its table, and the C
// calls crates/phpclients and the Rust core-tests cases (tests/modules/clients/client.rs) make.
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
                  SW_SOCKS5_STATE_HANDSHAKE == 1 && SW_SOCKS5_STATE_READY == 4 && SW_HTTP_PROXY_STATE_READY == 2 &&
                  SW_SSL_CLIENT == 2 && SW_SSL_DTLS == 128,
              "crates/clients/src/client.rs's constants");
static_assert(SW_ERROR_DNSLOOKUP_RESOLVE_FAILED == 711 && SW_ERROR_BAD_HOST_ADDR == 722 &&
                  SW_ERROR_OUTPUT_BUFFER_OVERFLOW == 1009 && SW_ERROR_SSL_HANDSHAKE_FAILED == 1018 &&
                  SW_ERROR_SSL_CREATE_SESSION_FAILED == 1020 && SW_ERROR_SOCKET_POLL_TIMEOUT == 6002 &&
                  SW_ERROR_SOCKS5_HANDSHAKE_FAILED == 7005 && SW_ERROR_HTTP_PROXY_HANDSHAKE_ERROR == 7101,
              "crates/clients/src/client.rs's error codes");

namespace swoole {
namespace network {

static void Client_onResolveCompleted(AsyncEvent *event) {
    const auto addr = swr_v1_core_string_view(swr_v1_core_resolver_view(event->data).address);
    swr_v1_clients_on_resolved(static_cast<Client *>(event->object)->state_, event->error, addr.data);
}

static const swr_v1_clients_native native = {
    [](void *cli, const char *host, int domain) -> int {
        GethostbynameRequest req(host, domain);
        AsyncEvent ev{};
        ev.data = req.state_handle();
        ev.object = cli;
        ev.handler = async::handler_gethostbyname;
        ev.callback = Client_onResolveCompleted;
        return async::dispatch(&ev) == nullptr ? SW_ERR : SW_OK;
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

int Client::sendto(const std::string &host, int port, const char *data, size_t len) const {
    return swr_v1_clients_sendto(state_, host.c_str(), host.length(), port, data, len, [](int) {
        swoole_warning("only supports SWOOLE_SOCK_(UDP/UDP6/UNIX_DGRAM)");
    });
}

int Client::get_peer_name(Address *addr) const {
    return swr_v1_clients_get_peer_name(state_, addr);
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
