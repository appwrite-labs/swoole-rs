#ifndef SWOOLE_RS_CO_SOCKET_H
#define SWOOLE_RS_CO_SOCKET_H
/* coroutine::Socket's fields: Rust's CoSocket (crates/hooks/src/co_socket.rs), in its
 * order and types. swoole_rs_coroutine_socket.h derives Socket from it and adds no
 * fields. Rust initialises, opens, accepts and releases records; both sides assert the
 * offsets below. network::Socket is the reactor's record. */
#include "swoole_rs_hooks.h"
#include "swoole_socket.h"
#include "swoole_protocol.h"
#include <stddef.h>

struct swr_v1_co_socket {
    int errCode;
    const char *errMsg;
    bool open_length_check;
    bool open_eof_check;
    bool http2;
    swoole::Protocol protocol;
    swSocketType type;
    swoole::network::Socket *socket;
    int sock_domain;
    int sock_type;
    int sock_protocol;
    int sock_fd;
    int connect_port;
    int backlog;
    const swoole::Allocator *buffer_allocator;
    size_t buffer_init_size;
    swoole::String *read_buffer;
    swoole::String *write_buffer;
    bool ssl_is_server;
    bool ssl_handshaked;
    bool zero_copy;
    swoole::NameResolver::Context *resolve_context_;
    /* The coroutines, barriers and timers bound to each direction. */
    swr_v1_hook_socket_state *state_;
    void *extra_;
};

extern "C" {
/* ~Socket: runs the destructor callback if close() has not, then frees the buffers,
 * the network::Socket, the bound state and the extra record (strings, proxies, TLS). */
void swr_v1_hook_co_socket_release(swr_v1_co_socket *socket);
/* The public constructors, by how: Socket(domain, type, protocol), Socket(SocketType),
 * Socket(fd, SocketType), Socket(fd, domain, type, protocol). owner is the Socket the
 * network::Socket points back to. Returns what it did, for swr_v1_hook_co_socket_created. */
enum { SWR_CO_SOCKET_BY_FAMILY, SWR_CO_SOCKET_BY_TYPE, SWR_CO_SOCKET_FD_BY_TYPE, SWR_CO_SOCKET_FD_BY_FAMILY };
uint32_t swr_v1_hook_co_socket_new(
    swr_v1_co_socket *socket, void *owner, int how, int fd, int socket_type, int domain, int kind, int protocol);
/* Rust's `Co` is the record's pointer (`#[repr(transparent)]`). */
typedef swr_v1_co_socket *swr_v1_co;
/* Socket::created: a failed socket(2) becomes the last error; the others are warnings. */
void swr_v1_hook_co_socket_created(swr_v1_co socket, uint32_t result);
/* UringSocket's constructors (swoole_rs_coroutine_socket.h): with io_uring, Rust runs the socket's
 * members through the ring (crates/hooks/src/uring_socket.rs). */
void swr_v1_hook_co_socket_uring(swr_v1_co socket);
/* Socket::connect(host, port) (crates/hooks/src/connect.rs), through the socket's proxies. */
bool swr_v1_hook_co_socket_connect_host(swr_v1_co_socket *socket, const char *host, size_t length, int port);
}

static_assert(sizeof(swoole::Protocol) == 64 && offsetof(swoole::Protocol, package_max_length) == 16 &&
                  offsetof(swoole::Protocol, private_data_1) == 24,
              "swoole::Protocol is Rust's Protocol");
static_assert(offsetof(swr_v1_co_socket, protocol) == 24 && offsetof(swr_v1_co_socket, type) == 88 &&
                  offsetof(swr_v1_co_socket, socket) == 96 && offsetof(swr_v1_co_socket, backlog) == 124 &&
                  offsetof(swr_v1_co_socket, buffer_allocator) == 128 &&
                  offsetof(swr_v1_co_socket, ssl_is_server) == 160 &&
                  offsetof(swr_v1_co_socket, resolve_context_) == 168 && offsetof(swr_v1_co_socket, extra_) == 184 && sizeof(swr_v1_co_socket) == 192,
              "coroutine::Socket's fields are Rust's CoSocket");
#endif
