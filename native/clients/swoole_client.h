/* SPDX-License-Identifier: Apache-2.0 */
/* Swoole's include/swoole_client.h, owned here: network::Client over the Rust state
 * (crates/clients/src/client.rs) and its C++ members, which native/clients reaches,
 * and the SyncClient and AsyncClient wrappers upstream's core-tests build on.
 * Upstream's file is deleted, so `#include "swoole_client.h"` finds this copy. */

#pragma once

#include "swoole_string.h"
#include "swoole_socket.h"
#include "swoole_reactor.h"
#include "swoole_protocol.h"
#include "swoole_proxy.h"
#include "swoole_rs_clients.h"
#include "swoole_rs_clients_record.h"

namespace swoole {
namespace network {

// The state and every operation are Rust's (crates/clients/src/client.rs); these are the
// members of C++ types it reaches through client.cc's table.
class Client : public swr_v1_clients_record {
  public:
    std::unique_ptr<Socks5Proxy> socks5_proxy = nullptr;
    std::unique_ptr<HttpProxy> http_proxy = nullptr;

    std::shared_ptr<SSLContext> ssl_context = nullptr;

    std::function<void(Client *cli)> onConnect = nullptr;
    std::function<void(Client *cli)> onError = nullptr;
    std::function<void(Client *cli, const char *, size_t)> onReceive = nullptr;
    std::function<void(Client *cli)> onClose = nullptr;
    std::function<void(Client *cli)> onBufferFull = nullptr;
    std::function<void(Client *cli)> onBufferEmpty = nullptr;

    static void init_reactor(Reactor *reactor);
    Client(SocketType _type, bool async);
    ~Client();

    // Last: its construction makes the socket, after every member above.
    swr_v1_clients *const state_;
    // Upstream's fields, as views of the state.
    uint32_t &buffer_high_watermark{*swr_v1_clients_watermark(state_, true)};
    uint32_t &buffer_low_watermark{*swr_v1_clients_watermark(state_, false)};
    const struct {
        const Client *client;
        operator bool() const {
            return client->flag(SWR_CLIENTS_ASYNC_CONNECT);
        }
    } async_connect{this};

    bool is_closed() const { return swr_v1_clients_flag(state_, SWR_CLIENTS_CLOSED); }
    bool flag(uint32_t name) const { return swr_v1_clients_flag(state_, name); }
    void set_flag(uint32_t name, bool value) { swr_v1_clients_set_flag(state_, name, value); }

    Socket *get_socket() const {
        return socket;
    }

    bool ready() const {
        return socket != nullptr;
    }

    SocketType get_socket_type() const {
        return socket->socket_type;
    }

    const std::string &get_http_proxy_host_name() const {
        if (ssl_context && !ssl_context->tls_host_name.empty()) {
            return ssl_context->tls_host_name;
        }
        return http_proxy->target_host;
    }

    int connect(const char *_host, int _port, double _timeout = -1, int _sock_flag = 0);
    ssize_t send(const char *_data, size_t _length, int _flags = 0);
    int sendfile(const char *_filename, off_t _offset = 0, size_t _length = 0);
    ssize_t recv(char *_data, size_t _length, int _flags = 0);
    int bind(const std::string &addr, int port) const;
    int sleep();
    int wakeup();
    int sendto(const std::string &host, int port, const char *data, size_t len) const;
    int get_peer_name(Address *addr) const;
    int shutdown(int _how = SHUT_RDWR);
    int close();
    bool socks5_handshake(const char *recv_data, size_t length);
    void set_timeout(double timeout, TimeoutType type = SW_TIMEOUT_ALL) const;
    bool has_timedout() const;
    void set_socks5_proxy(const std::string &host, int port, const std::string &user = "", const std::string &pwd = "");
    void set_http_proxy(const std::string &host, int port, const std::string &user = "", const std::string &pwd = "");

    int enable_ssl_encrypt();
    int ssl_handshake() const;
    int ssl_verify(int allow_self_signed) const;

#ifdef SSL_CTRL_SET_TLSEXT_HOSTNAME
    void set_tls_host_name(const std::string &str) const {
        ssl_context->tls_host_name = str;
        // if user set empty ssl_host_name, disable it, otherwise the underlying may set it automatically
        ssl_context->disable_tls_host_name = ssl_context->tls_host_name.empty();
    }
#endif
};

class SyncClient {
  protected:
    Client client;
    bool connected = false;
    bool created;
    bool async = false;
    SocketType type;

  public:
    explicit SyncClient(SocketType _type, bool _async = false) : client(_type, _async), async(_async), type(_type) {
        created = client.socket != nullptr;
    }

    virtual bool connect(const char *host, int port, double timeout = -1) {
        if (connected || !created) {
            return false;
        }
        if (client.connect(host, port, timeout, client.socket->is_dgram()) < 0) {
            return false;
        }
        connected = true;
        return true;
    }

    void set_stream_protocol() {
        client.open_length_check = true;
        client.protocol.get_package_length = Protocol::default_length_func;
        client.protocol.package_length_type = 'N';
        client.protocol.package_length_size = 4;
        client.protocol.package_body_offset = 4;
        client.protocol.package_length_offset = 0;
    }

    void set_package_max_length(uint32_t max_length) {
        client.protocol.package_max_length = max_length;
    }

    bool enable_ssl_encrypt() {
        if (client.enable_ssl_encrypt() < 0) {
            return false;
        }
        if (connected) {
            return client.ssl_handshake() == SW_OK;
        } else {
            return true;
        }
    }

    ssize_t send(const std::string &data) {
        return client.send(data.c_str(), data.length(), 0);
    }

    ssize_t send(const char *buf, size_t len) {
        return client.send(buf, len, 0);
    }

    bool sendfile(const char *filename, off_t offset = 0, size_t length = 0) {
        return client.sendfile(filename, offset, length) == SW_OK;
    }

    ssize_t recv(char *buf, size_t len, int flags = 0) {
        return client.recv(buf, len, flags);
    }

    bool close() {
        if (!created || client.is_closed()) {
            return false;
        }
        client.close();
        created = false;
        return true;
    }

    Client *get_client() {
        return &client;
    }

    virtual ~SyncClient() {
        if (created) {
            close();
        }
    }
};

class AsyncClient : public SyncClient {
  protected:
    std::function<void(AsyncClient *)> _onConnect = nullptr;
    std::function<void(AsyncClient *)> _onError = nullptr;
    std::function<void(AsyncClient *)> _onClose = nullptr;
    std::function<void(AsyncClient *, const char *data, size_t length)> _onReceive = nullptr;

  public:
    explicit AsyncClient(SocketType _type) : SyncClient(_type, true) {}

    bool connect(const char *host, int port, double timeout = -1) override {
        client.object = this;
        client.onConnect = [](Client *cli) {
            auto *ac = (AsyncClient *) cli->object;
            ac->_onConnect(ac);
        };
        client.onError = [](Client *cli) {
            auto *ac = (AsyncClient *) cli->object;
            ac->_onError(ac);
        };
        client.onClose = [](Client *cli) {
            auto *ac = (AsyncClient *) cli->object;
            ac->_onClose(ac);
        };
        client.onReceive = [](Client *cli, const char *data, size_t length) {
            auto *ac = (AsyncClient *) cli->object;
            ac->_onReceive(ac, data, length);
        };
        return SyncClient::connect(host, port, timeout);
    }

    void on_connect(const std::function<void(AsyncClient *)> &fn) {
        _onConnect = fn;
    }

    void on_error(const std::function<void(AsyncClient *)> &fn) {
        _onError = fn;
    }

    void on_close(const std::function<void(AsyncClient *)> &fn) {
        _onClose = fn;
    }

    void on_receive(const std::function<void(AsyncClient *, const char *data, size_t length)> &fn) {
        _onReceive = fn;
    }
};

}  // namespace network
}  // namespace swoole
