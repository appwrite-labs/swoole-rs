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
    void *extra_;
};

struct swr_v1_co_protocol_scope {
    swr_v1_co_socket *socket;
    bool eof;
    bool length;
    swoole::Protocol protocol;
};
struct swr_v1_co_timeout_scope {
    swr_v1_co_socket *socket_;
    double timeout;
    swoole::TimeoutType type;
    double original_timeout[16];
    double startup_time;
};
static_assert(sizeof(swr_v1_co_protocol_scope) == 80 && offsetof(swr_v1_co_protocol_scope, protocol) == 16, "Protocol scope ABI");
static_assert(sizeof(swr_v1_co_timeout_scope) == 160 && offsetof(swr_v1_co_timeout_scope, original_timeout) == 24 &&
              offsetof(swr_v1_co_timeout_scope, startup_time) == 152, "Timeout scope ABI");
extern "C" {
swr_v1_co_protocol_scope swr_v1_hook_protocol_scope(swr_v1_co_socket *socket);
void swr_v1_hook_protocol_restore(const swr_v1_co_protocol_scope *scope);
swr_v1_co_timeout_scope swr_v1_hook_timeout_scope(swr_v1_co_socket *socket, double timeout, int kind);
void swr_v1_hook_timeout_restore(const swr_v1_co_timeout_scope *scope);
}

#include "swoole_rs_corebase_string_slot.h"
struct swr_v1_co_socket_extra;
struct swr_v1_co_socket_extra_prepared {
    swr_v1_co_socket_extra *owner;
    const swr_v1_core_string *error;
    const swr_v1_core_string *connect_host;
    const swr_v1_core_string *ssl_host_name;
    /* The SSLContext's owner, which the TLS members read; set with the context. */
    const void **tls;
};
extern "C" {
swr_v1_co_socket_extra_prepared swr_v1_hook_socket_extra_new();
void swr_v1_hook_socket_extra_free(swr_v1_co_socket_extra *owner);

/* ~Socket after its destructor callback: frees the buffers, the bound state and the
 * network::Socket. */
void swr_v1_hook_co_socket_release(swr_v1_co_socket *socket);
void swr_v1_hook_co_socket_default_err(swr_v1_co_socket *socket, int last_error);
void swr_v1_hook_co_socket_borrowed_err(swr_v1_co_socket *socket, int error, const char *message);
int swr_v1_hook_co_socket_owned_err(swr_v1_co_socket *socket, int error, const char *bytes, size_t length);
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
/* UringSocket's constructors (swoole_socket_impl.h): with io_uring, Rust runs the socket's
 * members through the ring (crates/hooks/src/uring_socket.rs). */
void swr_v1_hook_co_socket_uring(swr_v1_co_socket *socket);

/* What the Rust operations need from C++ (crates/hooks/src/co_io.rs), installed once. */
typedef struct {
    void (*set_last_error)(int);
    const char *(*strerror)(int);
    /* Coroutine::get_current_safe(). */
    void *(*current)(void);
    /* A coroutine's id, and Coroutine::print_socket_bound_error. */
    long (*cid)(void *co);
    void (*bound_error)(int fd, const char *event, long cid);
    /* swoole_timer_add(timeout, false, timer_callback, socket), or null. */
    void *(*timer_add)(swr_v1_co_socket *, double timeout);
    void (*timer_del)(void *node);
    /* System::sleep(0.01). */
    void (*sleep)(void);
    /* swoole_event_add(events) when add, else swoole_event_set(events). */
    bool (*reactor)(swr_v1_co_socket *, uint32_t events, bool add);
    /* Yields co with cancel(event) as its cancel function. */
    void (*suspend)(swr_v1_co_socket *, void *co, uint32_t event);
    /* Moves a write's bytes to the write buffer unless they are there or the socket is zero-copy. */
    void (*buffer)(swr_v1_co_socket *, char **buffer, size_t length);
    /* remove_read_event/remove_write_event. */
    void (*remove)(swr_v1_co_socket *, uint32_t event);
    /* Resumes a coroutine. */
    void (*resume)(void *co);
    /* close()'s release: the destructor callback, once; the socket may be gone after. */
    void (*release)(swr_v1_co_socket *);
    /* socket->ssl_shutdown() and socket->check_liveness(). */
    void (*ssl_shutdown)(swr_v1_co_socket *);
    bool (*liveness)(swr_v1_co_socket *);
    /* The read String: 0 get, 1 store, 2 extend, 3 reduce; refreshes view. */
    void (*read_buffer)(swr_v1_co_socket *, swr_v1_hook_read_buffer *view, uint32_t action, size_t size);
    ssize_t (*recv)(swr_v1_co_socket *, char *, size_t, bool all);
    /* The native protocol callback/trace and packet error diagnostics. */
    ssize_t (*package_length)(swr_v1_co_socket *, uint32_t size, uint32_t *header_length);
    void (*packet_fail)(swr_v1_co_socket *, uint32_t kind, ssize_t length);
    /* new Socket(conn, server), the accepting constructor, and delete of a Socket. */
    void *(*accepted)(swr_v1_co_socket *server, swr_v1_socket *conn);
    void (*destroy)(void *socket);
} swr_v1_hook_co_socket_host_t;
/* Complete packet/buffer reads, including availability, buffer and timer. */
ssize_t swr_v1_hook_co_socket_recv_packet(swr_v1_co_socket *socket, double timeout);
/* The lifecycle (crates/hooks/src/lifecycle.rs): shutdown(how), cancel(event), close()
   (the socket may be gone after it returns true), check_liveness() and connect(addr). */
bool swr_v1_hook_co_socket_shutdown(swr_v1_co_socket *socket, int how);
bool swr_v1_hook_co_socket_cancel(swr_v1_co_socket *socket, uint32_t event);
bool swr_v1_hook_co_socket_close(swr_v1_co_socket *socket);
bool swr_v1_hook_co_socket_check_liveness(swr_v1_co_socket *socket);
bool swr_v1_hook_co_socket_connect(swr_v1_co_socket *socket, const struct sockaddr *addr, socklen_t length);
/* recvfrom(buffer, length) through the socket's own recvfrom into its
   network::Socket's peer address. */
ssize_t swr_v1_hook_co_socket_recvfrom(
    swr_v1_co_socket *socket, void *buffer, size_t length, ssize_t (*call)(swr_v1_co_socket *, void *, size_t, sockaddr *, socklen_t *));
void swr_v1_hook_co_socket_host(const swr_v1_hook_co_socket_host_t *host);
/* set_err(e), check_return_value, is_available, get_bound_cid, check_bound_co,
   get_event_str (a static string) and TimeoutController::has_timedout (startup is
   its clock, now microtime()). */
void swr_v1_hook_co_socket_set_err(swr_v1_co_socket *socket, int error);
void swr_v1_hook_co_socket_check(swr_v1_co_socket *socket, ssize_t result);
bool swr_v1_hook_co_socket_available(swr_v1_co_socket *socket, uint32_t event);
long swr_v1_hook_co_socket_bound_cid(const swr_v1_co_socket *socket, uint32_t event);
void swr_v1_hook_co_socket_check_bound(const swr_v1_co_socket *socket, uint32_t event);
const char *swr_v1_hook_co_socket_event_str(const swr_v1_co_socket *socket, uint32_t event);
bool swr_v1_hook_co_socket_timed_out(swr_v1_co_socket *socket, double timeout, double *startup, double now, int kind);
/* Socket::timer_callback: node, one of the socket's timers, fired. */
void swr_v1_hook_co_socket_timer_fired(swr_v1_co_socket *socket, void *node);
/* wait_event for the current coroutine: barrier (when given) stays installed for the wait;
   buffer, when not null, is a write's buffer slot. */
bool swr_v1_hook_co_socket_wait(swr_v1_co_socket *socket,
                                uint32_t event,
                                swr_v1_hook_barrier barrier,
                                void *barrier_state,
                                char **buffer,
                                size_t length);
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
