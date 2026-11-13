#ifndef SWOOLE_RS_REACTOR_H
#define SWOOLE_RS_REACTOR_H
#include <stdint.h>
#include <stddef.h>
#include <stdbool.h>
#include <sys/types.h>
#include <sys/socket.h>
#ifdef __APPLE__
#include <TargetConditionals.h>
#endif
struct iovec;
#ifdef __cplusplus
namespace swoole { class Reactor; namespace network { struct Socket; } }
typedef ssize_t (*swr_v1_reactor_write_fn)(swoole::Reactor *, swoole::network::Socket *, const void *, size_t);
typedef ssize_t (*swr_v1_reactor_writev_fn)(swoole::Reactor *, swoole::network::Socket *, const struct iovec *, size_t);
typedef int (*swr_v1_reactor_close_fn)(swoole::Reactor *, swoole::network::Socket *);
extern "C" {

#endif
#ifndef __cplusplus
typedef ssize_t (*swr_v1_reactor_write_fn)(void *, void *, const void *, size_t);
typedef ssize_t (*swr_v1_reactor_writev_fn)(void *, void *, const struct iovec *, size_t);
typedef int (*swr_v1_reactor_close_fn)(void *, void *);
#endif

struct pollfd;
struct epoll_event;
struct kevent;

typedef struct swr_v1_reactor_deferred swr_v1_reactor_deferred;
/* Reactor::defer_tasks and destroy_callbacks: FIFO (call, data) queues, created on first push.
 * push is 0, or -2 when it cannot allocate. */
int32_t swr_v1_reactor_deferred_push(swr_v1_reactor_deferred **, void (*call)(void *), void *data,
                                     void (*dispose)(void *));

/* network::Socket's write-path fields, embedded in Socket in this order and with these
 * types (Socket static_asserts it). */
typedef struct swr_v1_buffer swr_v1_buffer;
typedef struct {
    void *out_buffer;
    void *send_timer;
    uint32_t buffer_size;
    bool close_wait;
    bool nonblock;
    bool ssl_send;
    bool send_wait;
} swr_v1_socket_output;
/* Reactor handler tables (read/write/error per fd type, write and error defaults).
 * Handlers are ReactorHandler: int (*)(Reactor *, Event *). get kind: 0 read, 1 write,
 * 2 error; error_only false adds get_error_handler's read and write fallbacks. */
typedef struct swr_v1_reactor_handlers swr_v1_reactor_handlers;
typedef int (*swr_v1_reactor_handler)(void *, void *);
swr_v1_reactor_handler swr_v1_reactor_handlers_get(const swr_v1_reactor_handlers *, uint32_t kind, int fd_type, bool error_only);
void swr_v1_reactor_handlers_set_default(swr_v1_reactor_handlers *, uint32_t kind, swr_v1_reactor_handler);
const swr_v1_reactor_handler *swr_v1_reactor_handlers_default(const swr_v1_reactor_handlers *, uint32_t kind);
/* translate_events_to_poll. */
int16_t swr_v1_reactor_poll_events_to(int events);
/* The fd -> socket registry (Reactor::sockets_); null reads as empty. */
typedef struct swr_v1_reactor_registry swr_v1_reactor_registry;
void *swr_v1_reactor_registry_get(const swr_v1_reactor_registry *, int fd);
size_t swr_v1_reactor_registry_count(const swr_v1_reactor_registry *);
/* network::Socket's buffer sizes, setsockopt included. */
bool swr_v1_socket_buffer_size(int fd, bool recv, bool send, uint32_t size);
/* network::Socket's timeout group (dns_timeout, connect_timeout, read_timeout,
 * write_timeout), embedded in this order (Socket static_asserts it). */
typedef struct { double dns, connect, read, write; } swr_v1_socket_timeouts;
/* network::Socket's TLS state group (ssl_state, ssl_want_read, ssl_want_write),
 * embedded in this order and these types (Socket static_asserts it). The calls below
 * that take it set it. */
typedef struct {
    uint32_t state;
    bool want_read;
    bool want_write;
} swr_v1_socket_tls;
/* _ssl_read_x509_file: a certificate as PEM into at most length bytes, warning and
 * returning -1 on failure; frees cert when consume. */
int swr_v1_socket_ssl_read_x509(struct x509_st *cert, char *buffer, size_t length, bool consume);
/* An IOVector's cursor. */
typedef struct {
    struct iovec *iterator;
    int remain_count;
    int index;
    size_t offset_bytes;
} swr_v1_socket_iov_state;
/* The descriptor allocation owns its cursor; payloads remain borrowed. */
typedef struct swr_v1_reactor_iov swr_v1_reactor_iov;
typedef struct {
    struct iovec *original;
    int32_t count;
    swr_v1_socket_iov_state state;
} swr_v1_reactor_iov_fields;
typedef struct {
    swr_v1_reactor_iov *owner;
    swr_v1_reactor_iov_fields *fields;
    int32_t status;
} swr_v1_reactor_iov_prepared;
swr_v1_reactor_iov_prepared swr_v1_reactor_iov_prepare(const struct iovec *source, int32_t count);
void swr_v1_reactor_iov_destroy(swr_v1_reactor_iov *owner);
swr_v1_socket_iov_state swr_v1_reactor_iov_view(const swr_v1_reactor_iov *owner);
size_t swr_v1_reactor_iov_length(const swr_v1_reactor_iov *owner);
void swr_v1_reactor_iov_advance(swr_v1_reactor_iov *owner, intptr_t n);
/* network::Socket's swSocketType <-> (domain, type), and the type a host names
 * with how many leading bytes convert_to_type erases. */
typedef struct { int socket_type; size_t erase; } swr_v1_socket_host_type_plan;
int swr_v1_socket_type(int domain, int kind);
/* Each swSocketType's classes (Socket::is_stream ... is_local), indexed by the type. */
enum {
    SWR_SOCKET_STREAM = 1,
    SWR_SOCKET_DGRAM = 1 << 1,
    SWR_SOCKET_INET4 = 1 << 2,
    SWR_SOCKET_INET6 = 1 << 3,
    SWR_SOCKET_TCP = 1 << 4,
    SWR_SOCKET_LOCAL = 1 << 5,
};
extern const uint8_t swr_v1_socket_type_classes[16];
static inline bool swr_socket_type_is(int socket_type, uint8_t classes) {
    return (unsigned) socket_type < 16 && (swr_v1_socket_type_classes[socket_type] & classes);
}
bool swr_v1_socket_domain_and_type(int socket_type, int *domain, int *kind);
swr_v1_socket_host_type_plan swr_v1_socket_host_type(const uint8_t *host, size_t length);

/* Rust's reactor state (crates/reactor's ReactorState), which begins with swr_v1_reactor_head:
 * a Reactor is that state. swoole_event_init/free and the Reactor's constructor and destructor
 * are Rust's; nothing in C or C++ allocates one. */
typedef struct swr_v1_reactor_state swr_v1_reactor_state;
/* Reactor's idle and future tasks (Swoole\Event::cycle): null until set. */
typedef struct { void (*callback)(void *); void *data; } swr_v1_reactor_task;
/* The state's own views of itself; fields points at the head's type_. */
typedef struct {
    swr_v1_reactor_state *owner; void *fields;
    swr_v1_reactor_registry *registry; swr_v1_reactor_handlers *handlers;
} swr_v1_reactor_prepared;
/* Rust's ReactorPrepared then ReactorFields, laid out as one struct (asserted on both sides). */
typedef struct {
    swr_v1_reactor_prepared state_;
    int type_; void *ptr; int native_handle; uint32_t max_event_num;
    bool ready_, running, start, once, wait_exit, destroyed, bailout, timed_out;
    int32_t timeout_msec; uint16_t id; uint32_t max_socket; time_t last_malloc_trim_time;
    swr_v1_reactor_write_fn write;
    swr_v1_reactor_writev_fn writev;
    swr_v1_reactor_close_fn close;
    swr_v1_reactor_deferred *defer_tasks, *destroy_callbacks;
    swr_v1_reactor_task idle_task, future_task;
} swr_v1_reactor_head;
/* Reactor's end callbacks (set_end_callback, or erase_end_callback without fn) and exit
 * conditions (set_exit_condition, or remove_exit_condition without fn: whether one was set). */
void swr_v1_reactor_set_end(swr_v1_reactor_state *, int id, void (*callback)(void));
bool swr_v1_reactor_set_exit(swr_v1_reactor_state *, int id, bool (*condition)(size_t *event_num));
bool swr_v1_reactor_isset_exit(const swr_v1_reactor_state *, int id);
#ifdef __cplusplus
}
static_assert(offsetof(swr_v1_reactor_head, type_) == sizeof(swr_v1_reactor_prepared) &&
                  (sizeof(void *) != 8 || (sizeof(swr_v1_reactor_prepared) == 32 && sizeof(swr_v1_reactor_head) == 160)),
              "crates/reactor's ReactorState layout");
#endif
#endif
