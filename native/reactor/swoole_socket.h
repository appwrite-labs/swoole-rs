/* SPDX-License-Identifier: Apache-2.0 */
/* Swoole's include/swoole_socket.h, owned here: network::Address, IOVector and Socket's
 * methods over Rust's socket record (swoole_rs_socket.h). Address's methods call crates/clients
 * (upstream's src/network/address.cc is deleted). Upstream's file is deleted, so
 * `#include "swoole_socket.h"` finds this copy on the Rust prefix's include path. */
#pragma once

#include "swoole.h"
#include "swoole_ssl.h"
#include "swoole_buffer.h"
#include "swoole_string.h"

#include "swoole_rs_reactor.h"
#include "swoole_rs_clients.h"

#include <poll.h>
#include <sys/types.h>
#include <sys/socket.h>
#include <sys/un.h>
#include <sys/uio.h>

#include <netinet/in.h>
#include <netinet/ip6.h>
#include <netinet/tcp.h>
#include <netinet/udp.h>
#include <arpa/inet.h>

#include <stdexcept>
#include <string>
#include <vector>

#ifndef SOCK_NONBLOCK
#define SOCK_NONBLOCK O_NONBLOCK
#endif

enum {
    SW_BAD_SOCKET = -1,
};

struct swr_v1_core_resolver;

namespace swoole {
struct GethostbynameRequest;

namespace network {
struct SendfileTask {
    off_t offset;
    size_t length;
    char filename[0];
};

struct Address {
    union {
        sockaddr ss;
        sockaddr_in inet_v4;
        sockaddr_in6 inet_v6;
        sockaddr_un un;
    } addr;
    socklen_t len;
    SocketType type;

    bool assign(SocketType _type, const std::string &_host, int _port = 0, bool _resolve_name = true) {
        type = _type;
        const int error = swr_v1_clients_address_assign(&addr, &len, _type, _host.c_str(), _host.length(), _port,
                                                        _resolve_name ? swr_v1_clients_gethostbyname : nullptr);
        if (error != 0) {
            swoole_set_last_error(error);
            return false;
        }
        return true;
    }
    bool assign(const std::string &url) {
        const auto plan = swr_v1_clients_url(reinterpret_cast<const uint8_t *>(url.data()), url.size());
        if (plan.outcome == SWR_CLIENTS_URL_PORT_RANGE) {
            throw std::out_of_range("stoi");
        }
        if (plan.outcome == SWR_CLIENTS_URL_INVALID) {
            swoole_error_log(SW_LOG_NOTICE, SW_ERROR_BAD_HOST_ADDR, "Invalid address '%s'", url.c_str());
            return false;
        }
        type = static_cast<SocketType>(plan.kind);
        return assign(type, url.substr(plan.host_offset, plan.host_length), plan.port);
    }

    int get_port() const { return swr_v1_clients_address_port(this); }
    void set_port(int _port) { swr_v1_clients_address_set_port(this, _port); }
    const char *get_addr() const { return swr_v1_clients_address_host(this); }
    bool is_loopback_addr() const { return swr_v1_clients_address_loopback(this); }
    bool empty() const { return swr_v1_clients_address_empty(this); }

    in_addr *addr_v4() {
        return &addr.inet_v4.sin_addr;
    }

    in6_addr *addr_v6() {
        return &addr.inet_v6.sin6_addr;
    }

    static const char *type_str(SocketType type) { return swr_v1_clients_address_type(type); }
    static const char *addr_str(int family, const void *addr) { return swr_v1_clients_address_string(family, addr); }
    static bool verify_ip(int family, const std::string &str) { return swr_v1_clients_address_verify(family, str.c_str()); }
    static bool verify_port(int port, bool for_connect = false) { return swr_v1_clients_port(port, for_connect); }
};
static_assert(INET6_ADDRSTRLEN == 46, "Rust Address scratch capacity");

struct IOVector {
  private:
    swr_v1_reactor_iov *owner_;
    explicit IOVector(swr_v1_reactor_iov_prepared prepared);
  public:
    iovec *&iov;
    iovec *&iov_iterator;
    int &count;
    int &remain_count;
    int &index;
    size_t &offset_bytes;
    IOVector(const iovec *_iov, int _iovcnt);
    IOVector(const IOVector &) = delete;
    IOVector &operator=(const IOVector &) = delete;
    ~IOVector() { swr_v1_reactor_iov_destroy(owner_); }
    void update_iterator(ssize_t n) { swr_v1_reactor_iov_advance(owner_, n); }
    iovec *get_iterator() const { return swr_v1_reactor_iov_view(owner_).iterator; }
    size_t length() { return swr_v1_reactor_iov_length(owner_); }
    int get_remain_count() const { return swr_v1_reactor_iov_view(owner_).remain_count; }
    int get_index() const { return swr_v1_reactor_iov_view(owner_).index; }
    size_t get_offset_bytes() const { return swr_v1_reactor_iov_view(owner_).offset_bytes; }
    swr_v1_reactor_iov *handle() const { return owner_; }
};

}  // namespace network
}  // namespace swoole

#include "swoole_rs_socket.h"

namespace swoole {
namespace network {
// The fields are Rust's record (swoole_rs_socket.h); Rust allocates and frees it.
struct Socket : swr_v1_socket {
    // SocketRecord::new: zeroed, with the default timeouts. Records Rust frees come from Rust.
    Socket() : swr_v1_socket{} {
        *timeouts() = swr_v1_socket_defaults.timeouts;
    }

    // for reactor
    int handle_send();
    int handle_sendfile();
    // socket option [kernel space]
    bool set_buffer_size(uint32_t _buffer_size) const {
        return swr_v1_socket_buffer_size(fd, true, true, _buffer_size);
    }
    bool set_recv_buffer_size(uint32_t _buffer_size) const {
        return swr_v1_socket_buffer_size(fd, true, false, _buffer_size);
    }
    bool set_send_buffer_size(uint32_t _buffer_size) const {
        return swr_v1_socket_buffer_size(fd, false, true, _buffer_size);
    }
    bool set_kernel_read_timeout(double timeout);
    bool set_kernel_write_timeout(double timeout);

    bool set_kernel_timeout(double timeout) {
        return set_kernel_read_timeout(timeout) && set_kernel_write_timeout(timeout);
    }

    // socket option [user space]
    bool has_timedout() const;
    bool set_nonblock() {
        return set_fd_option(1, -1);
    }

    bool set_block() {
        return set_fd_option(0, -1);
    }

    bool set_fd_option(int _nonblock, int _cloexec);

    int set_option(int level, int optname, int optval) const {
        return setsockopt(fd, level, optname, &optval, sizeof(optval));
    }

    int set_option(int level, int optname, const void *optval, socklen_t optlen) const {
        return setsockopt(fd, level, optname, optval, optlen);
    }

    int get_option(int level, int optname, void *optval, socklen_t *optlen) const {
        return getsockopt(fd, level, optname, optval, optlen);
    }

    int get_option(int level, int optname, int *optval) const {
        socklen_t optlen = sizeof(*optval);
        return get_option(level, optname, optval, &optlen);
    }

    int get_fd() const {
        return fd;
    }

    const char *get_addr() const {
        return info.get_addr();
    }

    int get_port() const {
        return info.get_port();
    }

    uint32_t get_out_buffer_length() const {
        return out_buffer ? out_buffer->length() : 0;
    }

    int move_fd() {
        int sock_fd = fd;
        fd = SW_BAD_SOCKET;
        return sock_fd;
    }

    int set_reuse_addr(int enable = 1) const {
        return set_option(SOL_SOCKET, SO_REUSEADDR, enable);
    }

    int set_reuse_port(int enable = 1) const {
#ifdef SO_REUSEPORT
        return set_option(SOL_SOCKET, SO_REUSEPORT, enable);
#endif
        return -1;
    }

    bool check_liveness();

    int sendfile_sync(const char *filename, off_t offset, size_t length);

    ssize_t recv(void *_buf, size_t _n, int _flags);
    ssize_t send(const void *_buf, size_t _n, int _flags);
    ssize_t peek(void *_buf, size_t _n, int _flags) const;
    ssize_t readv(IOVector *io_vector);
    ssize_t writev(IOVector *io_vector);

    ssize_t writev(const iovec *iov, size_t iovcnt) const {
        return ::writev(fd, iov, iovcnt);
    }


    void clean() const;
    ssize_t send_async(const void *_data, size_t _len);
    ssize_t recv_sync(void *_data, size_t _len, int flags = 0);
    ssize_t writev_sync(const iovec *iov, size_t iovcnt);

    int connect(const Address &sa) const {
        return ::connect(fd, &sa.addr.ss, sa.len);
    }

    int connect(const Address *sa) const {
        return ::connect(fd, &sa->addr.ss, sa->len);
    }

    int connect(const std::string &host, int port) const {
        Address addr;
        addr.assign(socket_type, host, port);
        return connect(addr);
    }

    int connect_sync(const Address &sa);

    ReturnCode ssl_accept();
    ssize_t ssl_recv(void *_buf, size_t _n);
    ssize_t ssl_send(const void *_buf, size_t _n);
    ssize_t ssl_readv(IOVector *io_vector);
    ssize_t ssl_writev(IOVector *io_vector);
    STACK_OF(X509) * ssl_get_peer_cert_chain() const;
    std::vector<std::string> ssl_get_peer_cert_chain(int limit) const;
    X509 *ssl_get_peer_certificate() const;
    int ssl_get_peer_certificate(char *buf, size_t n) const;
    bool ssl_get_peer_certificate(String *buf) const;
    bool ssl_verify(bool allow_self_signed) const;
    bool ssl_shutdown();
    static const char *ssl_get_error_reason(int *reason);

    ssize_t recvfrom_sync(char *_buf, size_t _len, int flags, Address *sa);
    ssize_t recvfrom_sync(char *_buf, size_t _len, int flags, sockaddr *addr, socklen_t *addr_len);

    bool cork();
    bool uncork();

    bool isset_writable_event() const {
        return events & SW_EVENT_WRITE;
    }

    int wait_event(int timeout_ms, int _events) const;
    void free();

    static inline bool is_dgram(SocketType type) {
        return swr_socket_type_is(type, SWR_SOCKET_DGRAM);
    }

    static inline bool is_stream(SocketType type) {
        return swr_socket_type_is(type, SWR_SOCKET_STREAM);
    }

    static inline bool is_inet4(SocketType type) {
        return swr_socket_type_is(type, SWR_SOCKET_INET4);
    }

    static inline bool is_inet6(SocketType type) {
        return swr_socket_type_is(type, SWR_SOCKET_INET6);
    }

    static inline bool is_tcp(SocketType type) {
        return swr_socket_type_is(type, SWR_SOCKET_TCP);
    }

    static inline bool is_local(SocketType type) {
        return swr_socket_type_is(type, SWR_SOCKET_LOCAL);
    }

    bool is_stream() const {
        return is_stream(socket_type);
    }

    bool is_tcp() const {
        return is_tcp(socket_type);
    }

    bool is_dgram() const {
        return is_dgram(socket_type);
    }

    bool is_inet4() const {
        return is_inet4(socket_type);
    }

    bool is_inet6() const {
        return is_inet6(socket_type);
    }

    bool is_local() const {
        return is_local(socket_type);
    }

    ssize_t write(const void *_buf, size_t _len) const {
        return ::write(fd, _buf, _len);
    }

    ssize_t read(void *_buf, size_t _len) const {
        return ::read(fd, _buf, _len);
    }

    ssize_t read_sync(void *_buf, size_t _len);

    ssize_t write_sync(const void *_buf, size_t _len);

    int shutdown(int _how) const {
        return ::shutdown(fd, _how);
    }

    ssize_t sendto_sync(const Address &dst_addr, const void *_buf, size_t _n, int flags = 0);

    ssize_t sendto(const char *dst_host, int dst_port, const void *data, size_t len, int flags = 0) const {
        Address addr;
        if (!addr.assign(socket_type, dst_host, dst_port)) {
            return SW_ERR;
        }
        return sendto(addr, data, len, flags);
    }

    ssize_t sendto(const Address &dst_addr, const void *data, size_t len, int flags = 0) const {
        return ::sendto(fd, data, len, flags, &dst_addr.addr.ss, dst_addr.len);
    }

    static SocketType convert_to_type(int domain, int type) {
        return static_cast<SocketType>(swr_v1_socket_type(domain, type));
    }
    static SocketType convert_to_type(std::string &host);
    static int get_domain_and_type(SocketType type, int *sock_domain, int *sock_type) {
        return swr_v1_socket_domain_and_type(type, sock_domain, sock_type) ? SW_OK : SW_ERR;
    }
};


}  // namespace network

network::Socket *make_socket(int fd, FdType fd_type);
network::Socket *make_socket(SocketType socket_type, FdType fd_type, int flags);
network::Socket *make_socket(
    SocketType type, FdType fd_type, int sock_domain, int sock_type, int socket_protocol, int flags);
network::Socket *make_server_socket(SocketType socket_type,
                                    const char *address,
                                    int port = 0,
                                    int backlog = SW_BACKLOG);
}  // namespace swoole
#include "swoole_rs_socket_methods.h"
