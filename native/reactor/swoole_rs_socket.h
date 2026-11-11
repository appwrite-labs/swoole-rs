#ifndef SWOOLE_RS_SOCKET_H
#define SWOOLE_RS_SOCKET_H
/* network::Socket's fields: Rust's SocketRecord (crates/reactor/src/socket_record.rs),
 * in its order and types. native/reactor/swoole_socket.h includes this after network::Address and
 * derives Socket from it, adding only methods. Rust allocates, initialises, makes,
 * accepts, duplicates and frees records; both sides assert the offsets below. The
 * upstream 1-bit flags are bytes, which keeps every value the code assigns. */
#include "swoole_rs_reactor.h"
#include <stddef.h>

struct swr_v1_socket {
    int fd;
    swFdType fd_type;
    swSocketType socket_type;
    int events;
    bool enable_tcp_nodelay;
    bool kernel_nobufs;
    uint8_t removed;
    uint8_t silent_remove;
    uint8_t cloexec;
    uint8_t direct_send;
    uint8_t bound;
    uint8_t listened;
    uint8_t ssl_renegotiation;
    uint8_t ssl_handshake_buffer_set;
    uint8_t ssl_quiet_shutdown;
    uint8_t ssl_closed_;
    uint8_t dtls;
    uint8_t tcp_nopush;
    uint8_t tcp_nodelay;
    uint8_t skip_recv;
    uint8_t recv_wait;
    uint8_t event_hup;
    /* Upstream: when set, a system call interrupted by a signal is not retried but
     * returns -1 with errno EINTR, and the caller handles it. */
    uint8_t dont_restart;
    uint8_t close_notified;
    uint32_t chunk_size;
    /* The write-path group: swr_v1_socket_output, which the reactor's write path works on. */
    swoole::Buffer *out_buffer;
    swoole::TimerNode *send_timer;
    uint32_t buffer_size; /* the user-space memory buffer size */
    uint8_t close_wait;
    uint8_t nonblock;
    uint8_t ssl_send_;
    uint8_t send_wait;
    void *object;
    SSL *ssl;
    /* The TLS state group: swr_v1_socket_tls, which Rust's TLS calls set. */
    uint32_t ssl_state;
    uint8_t ssl_want_read;
    uint8_t ssl_want_write;
    /* The timeout group: swr_v1_socket_timeouts. */
    double dns_timeout;
    double connect_timeout;
    double read_timeout;
    double write_timeout;
    double last_received_time;
    double last_sent_time;
    swoole::Buffer *in_buffer;
    swoole::String *recv_buffer;
    swoole::TimerNode *recv_timer;
    size_t total_recv_bytes;
    size_t total_send_bytes;
    /* Written by getsockname/accept: the exact actual address. */
    swoole::network::Address info;
    /* Rust's atomic count of the read and edit closures holding the fields above. */
    alignas(8) uint32_t borrows;

    swr_v1_socket_output *output() {
        return reinterpret_cast<swr_v1_socket_output *>(&out_buffer);
    }
    swr_v1_socket_tls *tls() {
        return reinterpret_cast<swr_v1_socket_tls *>(&ssl_state);
    }
    const swr_v1_socket_tls *tls() const {
        return reinterpret_cast<const swr_v1_socket_tls *>(&ssl_state);
    }
    swr_v1_socket_timeouts *timeouts() {
        return reinterpret_cast<swr_v1_socket_timeouts *>(&dns_timeout);
    }
    const swr_v1_socket_timeouts *timeouts() const {
        return reinterpret_cast<const swr_v1_socket_timeouts *>(&dns_timeout);
    }
};

/* Socket::default_*_timeout and default_buffer_size; every new record copies the timeouts. */
typedef struct {
    swr_v1_socket_timeouts timeouts;
    uint32_t buffer_size;
} swr_v1_socket_defaults_t;

extern "C" {
extern swr_v1_socket_defaults_t swr_v1_socket_defaults;
}

static_assert(sizeof(void *) == 8, "the socket record's offsets are LP64's");
#ifdef __linux__
static_assert(sizeof(swoole::network::Address) == 120, "network::Address is Rust's SocketAddress");
#elif defined(__APPLE__)
static_assert(sizeof(swoole::network::Address) == 116, "network::Address is Rust's SocketAddress");
#endif
static_assert(offsetof(swr_v1_socket, events) == 12 && offsetof(swr_v1_socket, enable_tcp_nodelay) == 16 &&
                  offsetof(swr_v1_socket, removed) == 18 && offsetof(swr_v1_socket, close_notified) == 35 &&
                  offsetof(swr_v1_socket, chunk_size) == 36 && offsetof(swr_v1_socket, out_buffer) == 40 &&
                  offsetof(swr_v1_socket, object) == 64 && offsetof(swr_v1_socket, ssl) == 72 &&
                  offsetof(swr_v1_socket, ssl_state) == 80 && offsetof(swr_v1_socket, dns_timeout) == 88 &&
                  offsetof(swr_v1_socket, last_received_time) == 120 && offsetof(swr_v1_socket, in_buffer) == 136 &&
                  offsetof(swr_v1_socket, recv_timer) == 152 && offsetof(swr_v1_socket, total_send_bytes) == 168 &&
                  offsetof(swr_v1_socket, info) == 176 && offsetof(swr_v1_socket, borrows) == 296 &&
                  sizeof(swr_v1_socket) == 304,
              "network::Socket's layout is Rust's SocketRecord");
static_assert(offsetof(swr_v1_socket, send_timer) - offsetof(swr_v1_socket, out_buffer) ==
                      offsetof(swr_v1_socket_output, send_timer) &&
                  offsetof(swr_v1_socket, buffer_size) - offsetof(swr_v1_socket, out_buffer) ==
                      offsetof(swr_v1_socket_output, buffer_size) &&
                  offsetof(swr_v1_socket, close_wait) - offsetof(swr_v1_socket, out_buffer) ==
                      offsetof(swr_v1_socket_output, close_wait) &&
                  offsetof(swr_v1_socket, nonblock) - offsetof(swr_v1_socket, out_buffer) ==
                      offsetof(swr_v1_socket_output, nonblock) &&
                  offsetof(swr_v1_socket, ssl_send_) - offsetof(swr_v1_socket, out_buffer) ==
                      offsetof(swr_v1_socket_output, ssl_send) &&
                  offsetof(swr_v1_socket, send_wait) - offsetof(swr_v1_socket, out_buffer) ==
                      offsetof(swr_v1_socket_output, send_wait),
              "network::Socket embeds swr_v1_socket_output");
static_assert(offsetof(swr_v1_socket, ssl_want_read) - offsetof(swr_v1_socket, ssl_state) ==
                      offsetof(swr_v1_socket_tls, want_read) &&
                  offsetof(swr_v1_socket, ssl_want_write) - offsetof(swr_v1_socket, ssl_state) ==
                      offsetof(swr_v1_socket_tls, want_write),
              "network::Socket embeds swr_v1_socket_tls");
static_assert(offsetof(swr_v1_socket, connect_timeout) - offsetof(swr_v1_socket, dns_timeout) ==
                      offsetof(swr_v1_socket_timeouts, connect) &&
                  offsetof(swr_v1_socket, read_timeout) - offsetof(swr_v1_socket, dns_timeout) ==
                      offsetof(swr_v1_socket_timeouts, read) &&
                  offsetof(swr_v1_socket, write_timeout) - offsetof(swr_v1_socket, dns_timeout) ==
                      offsetof(swr_v1_socket_timeouts, write),
              "network::Socket embeds swr_v1_socket_timeouts");
static_assert(SW_SOCKET_DEFAULT_DNS_TIMEOUT == 60 && SW_SOCKET_DEFAULT_CONNECT_TIMEOUT == 10 &&
                  SW_SOCKET_DEFAULT_READ_TIMEOUT == 60 && SW_SOCKET_DEFAULT_WRITE_TIMEOUT == 60 &&
                  SW_SOCKET_BUFFER_SIZE == 8388608,
              "swr_v1_socket_defaults, which Socket::default_* reference (crates/reactor/src/socket_upstream.rs)");
#endif
