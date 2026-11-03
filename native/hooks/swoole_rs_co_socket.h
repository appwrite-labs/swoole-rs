#ifndef SWOOLE_RS_CO_SOCKET_H
#define SWOOLE_RS_CO_SOCKET_H
/* coroutine::Socket's scalar fields: Rust's CoSocket (crates/hooks/src/co_socket.rs),
 * in its order and types. swoole_coroutine_socket.h derives Socket from it and keeps
 * only the fields C++ owns (strings, proxies, the TLS context, the destructor
 * callback). Rust initialises, opens, accepts and releases records; both sides
 * assert the offsets below. network::Socket is the reactor's record. */
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
};

extern "C" {
/* The constructors write the whole record first; fini releases what they made
 * (native code frees the buffers and the network::Socket). */
void swr_v1_hook_co_socket_fini(swr_v1_co_socket *socket);
/* The public constructors, by how: Socket(domain, type, protocol), Socket(SocketType),
 * Socket(fd, SocketType), Socket(fd, domain, type, protocol). owner is the Socket the
 * network::Socket points back to. Returns SWR_CO_SOCKET_OPENED unless socket(2) failed
 * (errno says why), SWR_CO_SOCKET_NODELAY_FAILED when TCP_NODELAY could not be set,
 * and set_nonblock's fd_option warnings from bit 8. */
enum { SWR_CO_SOCKET_BY_FAMILY, SWR_CO_SOCKET_BY_TYPE, SWR_CO_SOCKET_FD_BY_TYPE, SWR_CO_SOCKET_FD_BY_FAMILY };
enum { SWR_CO_SOCKET_OPENED = 1, SWR_CO_SOCKET_NODELAY_FAILED = 2 };
uint32_t swr_v1_hook_co_socket_new(
    swr_v1_co_socket *socket, void *owner, int how, int fd, int socket_type, int domain, int kind, int protocol);
/* reinit_sock: false (errno set, nothing changed) if socket(2) failed; else *old is
 * the replaced network::Socket for native code to free. */
bool swr_v1_hook_co_socket_reinit(swr_v1_co_socket *socket, void *owner, int socket_type, swr_v1_socket **old);
/* The accepting constructor's fields; native code shares the TLS context. */
uint32_t swr_v1_hook_co_socket_accepted(swr_v1_co_socket *socket,
                                        void *owner,
                                        swr_v1_socket *conn,
                                        const swr_v1_co_socket *server);
/* pop_read_buffer / pop_write_buffer. */
void *swr_v1_hook_co_socket_pop_buffer(swr_v1_co_socket *socket, bool write);
/* getpeername: 0, or the errno to set. */
int swr_v1_hook_co_socket_peer_name(const swr_v1_co_socket *socket, swoole::network::Address *address);
}

static_assert(sizeof(swoole::Protocol) == 64 && offsetof(swoole::Protocol, package_max_length) == 16 &&
                  offsetof(swoole::Protocol, private_data_1) == 24,
              "swoole::Protocol is Rust's Protocol");
static_assert(offsetof(swr_v1_co_socket, protocol) == 24 && offsetof(swr_v1_co_socket, type) == 88 &&
                  offsetof(swr_v1_co_socket, socket) == 96 && offsetof(swr_v1_co_socket, backlog) == 124 &&
                  offsetof(swr_v1_co_socket, buffer_allocator) == 128 &&
                  offsetof(swr_v1_co_socket, ssl_is_server) == 160 &&
                  offsetof(swr_v1_co_socket, resolve_context_) == 168 && sizeof(swr_v1_co_socket) == 184,
              "coroutine::Socket's fields are Rust's CoSocket");
#endif
