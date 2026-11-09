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
struct iovec;
/* Rust-owned IOVector descriptor storage, used by every Rust reactor build. */
typedef struct { struct iovec *original, *iterator; int32_t status; } swr_v1_reactor_iov_allocation;
swr_v1_reactor_iov_allocation swr_v1_reactor_iov_owned_new(const struct iovec *, int32_t);
void swr_v1_reactor_iov_owned_free(struct iovec *);
size_t swr_v1_reactor_iov_owned_length(const struct iovec *, int32_t);
typedef struct { size_t offset_bytes, advance, consumed; int32_t remain_count, index, action; } swr_v1_reactor_iov_progress_reply;
swr_v1_reactor_iov_progress_reply swr_v1_reactor_iov_owned_progress(struct iovec *, intptr_t, int32_t, int32_t, size_t);

struct pollfd;
struct epoll_event;
struct kevent;

typedef struct swr_v1_reactor_deferred swr_v1_reactor_deferred;
/* Reactor::defer_tasks and destroy_callbacks: FIFO (call, data) queues, created on first push.
 * push is 0, or -2 when it cannot allocate. */
int32_t swr_v1_reactor_deferred_push(swr_v1_reactor_deferred **, void (*call)(void *), void *data,
                                     void (*dispose)(void *));

/* Each backend's whole wait() loop in one call (#336, #391). Rust runs the loop and its readiness
   order; these are the native effects, each passed `context` (prepare/remove also the socket). */
enum { SWR_REACTOR_POLL, SWR_REACTOR_EPOLL, SWR_REACTOR_KQUEUE };
/* Reactor handler tables (read/write/error per fd type, write and error defaults).
 * Handlers are ReactorHandler: int (*)(Reactor *, Event *). get kind: 0 read, 1 write,
 * 2 error; error_only false adds get_error_handler's read and write fallbacks. */
/* network::Socket's write-path fields, embedded in Socket in this order and with these
 * types (Socket static_asserts it), and the reactor's buffered write over them
 * (Reactor::write_func). The data, or iov[count], is what to queue; offset is what the
 * send took. Outcome 0: value is the result. Otherwise return SW_ERR after: 1 the
 * "data packet is too large" warning, 2 the output buffer overflow warning, 3
 * swoole_set_last_error(errno), 4 Buffer::check(status). */
typedef struct swr_v1_buffer swr_v1_buffer;
typedef struct {
    void *out_buffer;
    void *send_timer;
    uint32_t buffer_size;
    uint8_t close_wait;
    uint8_t nonblock;
    uint8_t ssl_send;
    uint8_t send_wait;
} swr_v1_socket_output;
typedef struct {
    const uint8_t *data;
    const struct iovec *iov;
    size_t count;
    size_t length;
} swr_v1_reactor_write_payload;
typedef struct {
    void *context;
    void (*set_nonblock)(void *);
    ssize_t (*send)(void *); /* keeps the kernel's errno */
    swr_v1_buffer *(*create_buffer)(void *); /* out_buffer = new Buffer(chunk_size) */
    void (*watch_writable)(void *); /* add_write_event unless already writable */
} swr_v1_reactor_write_ops;
typedef struct {
    ssize_t value;
    int32_t outcome;
    int32_t status;
} swr_v1_reactor_write_result;
swr_v1_reactor_write_result swr_v1_reactor_write(swr_v1_socket_output *output, bool *kernel_nobufs, swr_v1_buffer *buffer,
    uint32_t default_buffer_size, const swr_v1_reactor_write_payload *payload, const swr_v1_reactor_write_ops *ops);
/* swoole::Event; the wait loop reads and writes the socket record (swoole_rs_socket.h). */
typedef struct {
    int fd;
    int16_t reactor_id;
    int type;
    void *socket;
} swr_v1_reactor_event;
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
/* Reactor::_writable_callback and drain_write_buffer over the socket's block and its
 * output queue (null when there's none); the native effects below. drain also needs
 * wait_writable and removed. */
typedef struct {
    void *context;
    int (*front_kind)(void *); /* typed_front().kind or front()->type */
    int (*handle_send)(void *);
    int (*handle_sendfile)(void *);
    int (*close)(void *);
    int (*trigger_close)(void *);
    void (*delete_timer)(void *); /* swoole_timer_del(send_timer) */
    void (*remove_write_event)(void *);
    int (*wait_writable)(void *);
    bool (*removed)(void *);
} swr_v1_reactor_flush_ops;
int swr_v1_reactor_flush(swr_v1_socket_output *output, swr_v1_buffer *buffer, const swr_v1_reactor_flush_ops *ops);
void swr_v1_reactor_drain(swr_v1_socket_output *output, swr_v1_buffer *buffer, const swr_v1_reactor_flush_ops *ops);
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
    uint8_t want_read;
    uint8_t want_write;
} swr_v1_socket_tls;
/* Socket::ssl_clear_error: ERR_clear_error and clear the want flags. */
void swr_v1_socket_ssl_clear(swr_v1_socket_tls *tls);
/* _ssl_read_x509_file: a certificate as PEM into at most length bytes, warning and
 * returning -1 on failure; frees cert when consume. */
int swr_v1_socket_ssl_read_x509(struct x509_st *cert, char *buffer, size_t length, bool consume);
/* sw_reactor()->set_exit_condition, remove_exit_condition and set_end_callback. */
void swr_v1_event_set_exit_condition(int id, bool (*condition)(size_t *event_num));
void swr_v1_event_remove_exit_condition(int id);
void swr_v1_event_set_end_callback(int id, void (*callback)(void));
/* The swr_v1_reactor_change_log the C++ passes. */
void swr_v1_reactor_log(const char *origin, int code, const char *message);
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
swr_v1_reactor_iov *swr_v1_reactor_iov_create(const struct iovec *source, int32_t count, int32_t *status);
void swr_v1_reactor_iov_destroy(swr_v1_reactor_iov *owner);
swr_v1_socket_iov_state swr_v1_reactor_iov_view(const swr_v1_reactor_iov *owner);
size_t swr_v1_reactor_iov_length(const swr_v1_reactor_iov *owner);
void swr_v1_reactor_iov_advance(swr_v1_reactor_iov *owner, intptr_t n);
/* Socket::wait_for and the *_sync methods: Rust runs the loop (prepare, retry
 * classification, ENOBUFS backoff, poll, timeout) and each kind's progress rule;
 * native code gives the socket's flags and performs each I/O call. Returns the
 * bytes moved or -1 with the last error and errno set. */
enum {
    SWR_SOCKET_SYNC_SENDFILE = 3, /* all of length; a zero return is an error (Socket::sendfile_sync) */
};
typedef struct {
    void *socket;
    int fd;
    bool dont_restart;
    bool *kernel_nobufs;
    void (*prepare)(void *socket);
    int16_t (*events)(void *socket, int event);
    ssize_t (*io)(void *socket, void *buffer, size_t length, int flags);
    void (*set_last_error)(int error);
    void (*dispatch)(void);
} swr_v1_socket_sync_hooks;
ssize_t swr_v1_socket_sync(const swr_v1_socket_sync_hooks *hooks, int kind, void *buffer, size_t length, int flags, int event, int timeout_ms);
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
/* Logs message as from origin: code 0 is swoole_sys_warning (errno is the kernel's), else
 * swoole_error_log(SW_LOG_WARNING, code, ...); then the backtrace on error. */
typedef void (*swr_v1_reactor_change_log)(const char *origin, int code, const char *message);
/* Reactor::add (op 0), set (1) or del (2) on the state's backend, checks and registry included:
 * 0, -1 after logging the failure, or -2 when add cannot allocate. */
int swr_v1_reactor_change(swr_v1_reactor_state *, void *socket, uint32_t op, int events, swr_v1_reactor_change_log log);
/* Reactor's interest helpers over swr_v1_reactor_change: add_event (kind 0) and del_event (1) of
 * event, add_read_event (2), add_write_event (3), remove_read_event (4), remove_write_event (5). */
int swr_v1_reactor_interest(swr_v1_reactor_state *, void *socket, uint32_t kind, int event, swr_v1_reactor_change_log log);
#ifdef __cplusplus
}
static_assert(offsetof(swr_v1_reactor_head, type_) == sizeof(swr_v1_reactor_prepared) &&
                  (sizeof(void *) != 8 || (sizeof(swr_v1_reactor_prepared) == 32 && sizeof(swr_v1_reactor_head) == 160)),
              "crates/reactor's ReactorState layout");
#endif
#endif
