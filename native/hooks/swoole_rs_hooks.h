/* SPDX-License-Identifier: Apache-2.0 */
#ifndef SWOOLE_RS_HOOKS_H
#define SWOOLE_RS_HOOKS_H
#include <stddef.h>
#include <stdint.h>
#include <errno.h>
#include <stdbool.h>
#include <sys/types.h>
#include <stdio.h>
#include <dirent.h>
#include <sys/stat.h>
#include <sys/statvfs.h>
#include <poll.h>
#include <sys/socket.h>
#include <netdb.h>
#include <sys/types.h>
#include <sys/types.h>
#ifdef __cplusplus
extern "C" {
#endif
typedef bool (*swr_v1_hook_offload)(void (*job)(void *), void *context);
/* System::read_file / write_file: the whole operation on the pool (offload). Each
 * warning reaches warn on the pool thread, formatted as upstream logged it. */
typedef struct swr_v1_hook_file_ops {
    /* swoole_sys_warning("%s", message) with errno = error, logged as from origin. */
    void (*warn)(int error, const char *origin, const char *message);
    /* out's String (created on first use): length bytes of content, room for size; its buffer. */
    char *(*buffer)(void *out, size_t length, size_t size);
} swr_v1_hook_file_ops;
void swr_v1_hook_read_file(
    const char *file, bool lock, void *out, const swr_v1_hook_file_ops *ops, swr_v1_hook_offload offload);
ssize_t swr_v1_hook_write_file(const char *file,
                               const void *data,
                               size_t length,
                               bool lock,
                               int flags,
                               const swr_v1_hook_file_ops *ops,
                               swr_v1_hook_offload offload);
int32_t swr_v1_hook_transition(uint32_t requested, uint32_t installed, uint32_t bit);
uint32_t swr_v1_hook_filter(uint32_t flags, uint32_t unavailable);
bool swr_v1_hook_file_result(bool open, int64_t result);
/* A hooked stream socket name's host and port, as parse_ip_address_ex splits it.
 * Status 0 parsed; 1 bracketed IPv6 without "]:"; 2 no ':' before the last byte. */
typedef struct { int32_t status; size_t host_offset, host_length; int32_t port; } swr_v1_hooks_address_plan;
swr_v1_hooks_address_plan swr_v1_hooks_parse_address(const uint8_t *name, size_t length);
/* PHP's sleep family, whole: argument checks, duration and the sleep. Pass
 * coroutine_sleep (System::sleep) inside a coroutine, null outside one. Status 0
 * done; 1 argument `value` invalid; 2 interrupted, time left in seconds and
 * nanoseconds; 3 warn (nanosleep: then true; sleep_until: target passed, then false). */
typedef int32_t (*swr_v1_hooks_coroutine_sleep)(double seconds);
typedef struct { int32_t status; int64_t value, seconds, nanoseconds; } swr_v1_hooks_sleep_outcome;
swr_v1_hooks_sleep_outcome swr_v1_hooks_sleep(int64_t seconds, swr_v1_hooks_coroutine_sleep coroutine_sleep);
swr_v1_hooks_sleep_outcome swr_v1_hooks_usleep(int64_t microseconds, swr_v1_hooks_coroutine_sleep coroutine_sleep);
swr_v1_hooks_sleep_outcome swr_v1_hooks_nanosleep(int64_t seconds, int64_t nanoseconds, swr_v1_hooks_coroutine_sleep coroutine_sleep);
swr_v1_hooks_sleep_outcome swr_v1_hooks_sleep_until(double timestamp, double top, swr_v1_hooks_coroutine_sleep coroutine_sleep);
double swr_v1_hook_backoff(double current, double initial, double maximum);
int64_t swr_v1_hook_transfer_result(int64_t result, uint64_t total);
typedef struct { uint64_t total; bool again; } swr_v1_hook_transfer_plan;
swr_v1_hook_transfer_plan swr_v1_hook_transfer(int64_t result, uint64_t total, uint64_t requested, bool wait);
typedef struct { uint32_t revents; bool wake; } swr_v1_hook_event_plan;
swr_v1_hook_event_plan swr_v1_hook_event_waiter(uint32_t received, uint32_t event);
typedef struct { int32_t result; int32_t last_error; int32_t error; } swr_v1_hook_wait_event_outcome;
/* The EventWaiter's reactor socket and coroutine. add: make_socket(fd, SW_FD_CO_EVENT)
 * carrying waiter, added for events; null with the last error in *error if refused (the
 * socket released, not its fd). park: co->yield_ex(timeout), 0 or the last error.
 * remove: off the reactor, released, not its fd. */
typedef struct {
    void *(*current)(void);
    void *(*add)(int fd, int events, void *waiter, int *error);
    int (*park)(void *co, double timeout);
    void (*remove)(void *socket);
} swr_v1_hook_event_wait_ops;
swr_v1_hook_wait_event_outcome swr_v1_hook_wait_event(int32_t fd, int32_t events, double timeout, const swr_v1_hook_event_wait_ops *ops);
typedef struct {
    void *(*current)(void);
    bool (*busy)(void);
    void (*listen)(int signo, bool on);
    void (*waiting)(bool on);
    bool (*yield_for)(void *co, double timeout);
    void (*resume)(void *co);
} swr_v1_hook_signal_ops;
typedef struct { int32_t signo; int32_t last_error; } swr_v1_hook_signalled;
void swr_v1_hook_signal_wake(int signo);
swr_v1_hook_signalled swr_v1_hook_wait_signal(const int32_t *signals, size_t count, double timeout, const swr_v1_hook_signal_ops *ops);
typedef struct { int32_t fd; int32_t events; int32_t revents; } swr_v1_hook_poll_fd;
typedef struct {
    void *(*add)(int fd, int events, void *task);
    void (*remove)(void *socket);
    void *(*timer_add)(double timeout, void *task);
    void (*timer_del)(void *timer);
    void (*defer)(void *task);
    void *(*current)(void);
    void (*park)(void *co);
    void (*resume)(void *co);
} swr_v1_hook_poll_ops;
/* System::init_reactor's SW_FD_CO_POLL and SW_FD_CO_EVENT handlers, registered through set;
 * wake defers co->resume() on this thread's reactor. */
void swr_v1_hook_init_reactor(void *reactor,
                              void (*set)(void *reactor, int fd_type, int events, int (*handler)(void *, void *)),
                              void (*wake)(void *co));
void swr_v1_hook_poll_completed(void *task);
void swr_v1_hook_poll_timeout(void *task);
bool swr_v1_hook_socket_poll(swr_v1_hook_poll_fd *fds, size_t count, double timeout, const swr_v1_hook_poll_ops *ops);
/* The native coroutine socket (SocketImpl: coroutine::Socket, or UringSocket with
 * --enable-uring-socket): new, delete, and the calls whose body is the class's. */
typedef struct swr_v1_hook_socket_ops {
    void *(*open)(int domain, int type, int protocol); /* new SocketImpl(domain, type, protocol) */
    void *(*wrap)(int fd);                             /* new SocketImpl(fd, SW_SOCK_RAW) */
    void (*free)(void *socket);
    ssize_t (*send)(void *socket, const void *buf, size_t n);
    ssize_t (*recv)(void *socket, void *buf, size_t n);
    ssize_t (*peek)(void *socket, void *buf, size_t n);
    ssize_t (*write)(void *socket, const void *buf, size_t n);
    ssize_t (*read)(void *socket, void *buf, size_t n);
    ssize_t (*sendmsg)(void *socket, const struct msghdr *msg, int flags);
    ssize_t (*recvmsg)(void *socket, struct msghdr *msg, int flags);
    bool (*connect)(void *socket, const struct sockaddr *addr, socklen_t length);
    void *(*accept)(void *socket);             /* accept(0) */
    bool (*poll)(void *socket, uint32_t event); /* poll(event) */
} swr_v1_hook_socket_ops;
/* What the hooked functions (Rust, under upstream's swoole_coroutine_<name>) need
 * of native; registered at load (swr_v1_hook_set_host). */
typedef struct swr_v1_hook_host {
    bool (*in_coroutine)(void);    /* a reactor and a coroutine are running */
    swr_v1_hook_offload offload;   /* swoole::coroutine::async */
    int (*sleep)(double seconds);  /* System::sleep */
    const void *uring;             /* &Iouring::host with io_uring, else NULL */
    bool uring_statx;              /* HAVE_IOURING_STATX */
    bool uring_ftruncate;          /* HAVE_IOURING_FTRUNCATE */
    void (*set_last_error)(int error);
    const swr_v1_hook_poll_ops *poll; /* System's, for the hooked poll */
    swr_v1_hook_socket_ops socket;
} swr_v1_hook_host;
void swr_v1_hook_set_host(const swr_v1_hook_host *host);
void swr_v1_hook_dns_cache_expire(int64_t expire);
void swr_v1_hook_dns_cache_capacity(size_t capacity);
void swr_v1_hook_dns_cache_clear(void);
float swr_v1_hook_dns_cache_hit_ratio(void);
bool swr_v1_hook_dns_cache_get(int domain, const char *name, size_t length, void *out, void (*assign)(void *out, const char *address, size_t length));
void swr_v1_hook_dns_cache_set(int domain, const char *name, size_t length, const char *address, size_t address_length);
double swr_v1_hook_timeval(double seconds, double micros);
typedef struct { uint32_t flags; bool conflict; } swr_v1_hook_curl_plan;
swr_v1_hook_curl_plan swr_v1_hook_curl_flags(uint32_t flags, uint32_t native, uint32_t php);
/* coroutine::async over the AsyncThreads pool. The event is upstream's AsyncEvent
 * (layout asserted in system.cc); dispatch is async::dispatch. */
typedef struct {
    void *(*current)(void);                       /* Coroutine::get_current_safe() */
    void *(*dispatch)(const void *event);         /* async::dispatch */
    void (*park)(void *co);                       /* co->yield() */
    bool (*park_for)(void *co, double timeout);   /* co->yield_ex(timeout) */
    void (*resume)(void *co);
    int (*last_error)(void);
} swr_v1_hook_async_ops;
bool swr_v1_hook_async(void *event, void (*handler)(void *event), double timeout, const swr_v1_hook_async_ops *ops);
bool swr_v1_hook_async_job(void (*job)(void *), void *context, const swr_v1_hook_async_ops *ops);
int32_t swr_v1_hook_dns_error(int32_t error, int32_t asynchronous_timeout, int32_t resolve_timeout);
double swr_v1_hook_sleep_timeout(double seconds, double minimum);
typedef struct { size_t length, size; int64_t offset; char *str; } swr_v1_hook_read_buffer;
/* A wait barrier: retries the transfer once the socket is ready; true to keep waiting. */
typedef bool (*swr_v1_hook_barrier)(void *barrier_state);
/* The coroutine Socket's connection state, Rust-owned behind an opaque handle (#439) in its record. */
typedef struct swr_v1_hook_socket_state swr_v1_hook_socket_state;
typedef struct { bool connected, read, write; } swr_v1_hook_socket_flags_t;
/* The record makes its own state; the hooks gates' socket facades use these. */
swr_v1_hook_socket_state *swr_v1_hook_socket_state_new(void);
void swr_v1_hook_socket_state_free(swr_v1_hook_socket_state *state);
swr_v1_hook_socket_flags_t swr_v1_hook_socket_flags(const swr_v1_hook_socket_state *state);
void swr_v1_hook_socket_set_flags(swr_v1_hook_socket_state *state, swr_v1_hook_socket_flags_t flags);
void swr_v1_hook_socket_connect(swr_v1_hook_socket_state *state);
/* Socket::connect(host, port), whole (crates/hooks/src/connect.rs). family is AF_INET/AF_INET6
 * for an IPv4/IPv6 socket, else 0; proxy 0 none, 1 SOCKS5, 2 HTTP. set_err(-1) is set_err();
 * set_err_text with null text uses swoole_strerror(code). set_host stores the connect target
 * (0), the TLS host name (1) or the proxy's target, making the proxy the connect target (2).
 * socks5_step: -1 failed, 0 continue, 1 ready. */
typedef struct {
    void *context;
    int domain, family, proxy;
    bool http_handshake, ssl;
    bool (*available)(void *);
    void (*set_err)(void *, int code);
    void (*set_err_text)(void *, int code, const char *text, size_t length);
    int (*last_error)(void);
    void (*set_host)(void *, int which, const char *host, size_t length, int port);
    const char *(*connect_host)(void *, size_t *length);
    bool (*reinit)(void *, int type);
    bool (*assign)(void *);
    bool (*bad_host)(void);
    bool (*lookup)(void *);
    bool (*connect)(void *);
    ssize_t (*send)(void *, const char *buffer, size_t length);
    ssize_t (*recv)(void *, char *buffer, size_t length);
    ssize_t (*socks5_negotiate)(void *, const char **buffer);
    int (*socks5_step)(void *, const char *reply, size_t length);
    size_t (*http_request)(void *, const char **buffer);
    ssize_t (*http_response)(void *, bool *accepted);
    void (*http_done)(void *, bool received);
    void (*client)(void *);
    bool (*handshake)(void *);
} swr_v1_hook_connect_ops;
bool swr_v1_hook_connect_host(const swr_v1_hook_connect_ops *ops, const char *host, size_t length, int port);
/* Socket::ssl_handshake up to marking it handshaked. mode 0 client (step: ssl_connect() < 0 ? -1
 * : ssl_state), 1 server (step: ssl_accept()), 2 a step that finishes by itself (1 done, 0 again,
 * -1 failed). wait(fresh) waits to read under a timer of its own, else the handshake's one timer. */
typedef struct {
    void *context;
    int mode;
    bool handshaked, context_ready, verify_peer;
    bool (*available)(void *);
    void (*set_err)(void *, int code);
    bool (*context_create)(void *);
    bool (*create)(void *);
    int (*step)(void *);
    bool (*wait)(void *, bool fresh);
    bool (*verify)(void *);
} swr_v1_hook_tls_ops;
bool swr_v1_hook_ssl_handshake(const swr_v1_hook_tls_ops *ops);
/* TimeoutSetter: save and set (restore false), or restore, each SW_TIMEOUT_* type in types;
 * original holds timeout_type_list's four entries. */
typedef struct {
    double (*get)(void *, int type);
    void (*set)(void *, double timeout, int type);
} swr_v1_hook_timeout_ops;
void swr_v1_hook_timeouts(const swr_v1_hook_timeout_ops *ops, void *socket, double timeout, int types, double *original,
                          bool restore);
/* The coroutine bound for reading or writing (event SW_EVENT_READ/WRITE; bind accepts both bits).
   bound_co follows get_bound_co; event_name returns 0 reading, 1 writing, 2 reading or writing. */
void *swr_v1_hook_socket_co(const swr_v1_hook_socket_state *state, uint32_t event);
void swr_v1_hook_socket_bind(swr_v1_hook_socket_state *state, uint32_t event, void *co);
void *swr_v1_hook_socket_bound_co(const swr_v1_hook_socket_state *state, uint32_t event);
uint32_t swr_v1_hook_socket_event_name(const swr_v1_hook_socket_state *state, uint32_t event);
#ifdef __cplusplus
}
// Callables stay on the native stack. Rust returns before every IO, errno
// classifier, timer operation or coroutine suspension; exceptions remain native.
static_assert(sizeof(swr_v1_hook_curl_plan) == 8, "curl plan ABI");
static_assert(sizeof(swr_v1_hook_event_plan) == 8, "event waiter ABI");
static_assert(sizeof(swr_v1_hook_transfer_plan) == 16, "transfer ABI");
#endif
#endif
