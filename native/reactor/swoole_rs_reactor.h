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
/* Socket's queued output: handle_send sends the rest of the front data chunk and
 * classifies the result; handle_sendfile corks (sendfile_cork), lets native code
 * send, then classifies and completes the request (sendfile_sent); sendfile_async
 * runs prepare (make and check the request), buffer and enqueue in order. Native
 * code applies the result: pop, store offset, close_wait/send_wait, and warn (1
 * send, 2 sendfile of requested bytes); code is returned. */
typedef struct {
    void *socket;
    bool *kernel_nobufs;
    ssize_t (*send)(void *socket, const void *data, size_t length);
    bool (*cork)(void *socket);
    void (*uncork)(void *socket);
    void (*sent)(void *socket);
} swr_v1_socket_output_hooks;
typedef struct {
    int code;
    bool pop;
    bool set_offset;
    uint32_t offset;
    bool close_wait;
    bool send_wait;
    int warn;
    size_t requested;
} swr_v1_socket_output_result;
swr_v1_socket_output_result swr_v1_socket_handle_send(const swr_v1_socket_output_hooks *hooks, const uint8_t *data, uint32_t length, uint32_t offset);
void swr_v1_socket_sendfile_cork(const swr_v1_socket_output_hooks *hooks, int8_t *corked, off_t begin, off_t end, off_t cork_min);
swr_v1_socket_output_result swr_v1_socket_sendfile_sent(const swr_v1_socket_output_hooks *hooks, ssize_t sent, size_t requested, int8_t *corked, off_t begin, off_t end);
int swr_v1_socket_sendfile_async(void *socket, bool (*prepare)(void *), void (*buffer)(void *), void (*enqueue)(void *));

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
typedef struct { uint32_t phase; } swr_v1_reactor_signal_state;
int32_t swr_v1_reactor_signal_workflow(swr_v1_reactor_signal_state *, int32_t);
typedef struct { uint32_t phase; int32_t action; } swr_v1_reactor_builtin_reply;
swr_v1_reactor_builtin_reply swr_v1_reactor_builtin_step(uint32_t, uint32_t, uint32_t, uint64_t);

typedef struct { uint32_t phase; } swr_v1_reactor_init_state;
typedef struct { int32_t selected; uint32_t observe_kqueue, factory; } swr_v1_reactor_backend_plan;
swr_v1_reactor_backend_plan swr_v1_reactor_backend_type(int32_t, uint32_t, uint32_t, uint32_t);
int32_t swr_v1_reactor_init_next(swr_v1_reactor_init_state *, uint32_t, uint32_t, uint32_t, uint32_t);

typedef struct swr_v1_reactor_deferred swr_v1_reactor_deferred;
int32_t swr_v1_reactor_deferred_push(swr_v1_reactor_deferred **, uintptr_t);
int32_t swr_v1_reactor_deferred_step(swr_v1_reactor_deferred *, uint32_t *, uintptr_t *);
int32_t swr_v1_reactor_deferred_clear(swr_v1_reactor_deferred **, uintptr_t *);

/* End (0) and exit (1) callbacks by key. Rust keeps views[kind] current: that kind's
   entries in ascending key order, readable until the next change. Its other fields
   are Rust's own. */
typedef struct { int32_t key; uintptr_t token; } swr_v1_reactor_callback_entry;
typedef struct { const swr_v1_reactor_callback_entry *entries; size_t len; } swr_v1_reactor_callback_view;
typedef struct swr_v1_reactor_callbacks { swr_v1_reactor_callback_view views[2]; } swr_v1_reactor_callbacks;
int32_t swr_v1_reactor_callbacks_lookup(const swr_v1_reactor_callbacks *, uint32_t, int32_t, uintptr_t *);
int32_t swr_v1_reactor_callbacks_erase(swr_v1_reactor_callbacks *, uint32_t, int32_t, uintptr_t *);
/* The index of the first entry after key: where a walk goes on once a callback changed the entries. */
size_t swr_v1_reactor_callbacks_after(const swr_v1_reactor_callbacks *, uint32_t, int32_t);
int32_t swr_v1_reactor_callbacks_ensure(swr_v1_reactor_callbacks **, uint32_t, int32_t, uintptr_t, uintptr_t *);
int32_t swr_v1_reactor_callbacks_clear_next(swr_v1_reactor_callbacks **, uintptr_t *);
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
swr_v1_reactor_handlers *swr_v1_reactor_handlers_new(void);
void swr_v1_reactor_handlers_free(swr_v1_reactor_handlers *);
bool swr_v1_reactor_handlers_set(swr_v1_reactor_handlers *, int fd_type, int events, swr_v1_reactor_handler);
bool swr_v1_reactor_handlers_isset(const swr_v1_reactor_handlers *, int fd_type, int events);
swr_v1_reactor_handler swr_v1_reactor_handlers_get(const swr_v1_reactor_handlers *, uint32_t kind, int fd_type, bool error_only);
void swr_v1_reactor_handlers_set_default(swr_v1_reactor_handlers *, uint32_t kind, swr_v1_reactor_handler);
const swr_v1_reactor_handler *swr_v1_reactor_handlers_default(const swr_v1_reactor_handlers *, uint32_t kind);
/* Reactor::wait()'s native effects (swr_v1_reactor_wait); each receives the reactor. */
typedef struct {
    void *reactor;               /* each handler runs as handler(reactor, event) */
    void *const *deferred;       /* Reactor::defer_tasks, read before each kernel wait */
    void (*begin)(void *);
    void (*end)(void *, int timed_out);
    int (*interrupted)(void *); /* nullable: epoll io_uring builds only */
    void (*interrupt)(void *);  /* nullable */
    void (*wait_failed)(void *, int entries); /* entries: poll's */
    void (*handler_failed)(void *, uint32_t kind, const swr_v1_reactor_event *event);
    void (*remove)(void *, void *socket);
    void (*signal)(void *signal);      /* nullable: kqueue only */
    void (*unknown)(void *, int filter); /* kqueue only */
} swr_v1_reactor_wait_ops;
typedef struct { size_t size, alignment, fields[6]; } swr_v1_reactor_backend_layout;
#if defined(__linux__) && !defined(__ANDROID__)
typedef struct swr_v1_reactor_epoll_events swr_v1_reactor_epoll_events;
swr_v1_reactor_epoll_events *swr_v1_reactor_epoll_new(size_t);
void swr_v1_reactor_epoll_free(swr_v1_reactor_epoll_events *);
struct epoll_event *swr_v1_reactor_epoll_data(swr_v1_reactor_epoll_events *);
swr_v1_reactor_backend_layout swr_v1_reactor_epoll_layout(void);
#endif
#if (defined(__APPLE__) && TARGET_OS_OSX) || defined(__FreeBSD__) || defined(__OpenBSD__) || defined(__NetBSD__) || defined(__DragonFly__)
typedef struct swr_v1_reactor_kqueue_events swr_v1_reactor_kqueue_events;
swr_v1_reactor_kqueue_events *swr_v1_reactor_kqueue_new(size_t);
void swr_v1_reactor_kqueue_free(swr_v1_reactor_kqueue_events *);
struct kevent *swr_v1_reactor_kqueue_data(swr_v1_reactor_kqueue_events *);
swr_v1_reactor_backend_layout swr_v1_reactor_kqueue_layout(void);
#endif
typedef struct swr_v1_reactor_poll_snapshot swr_v1_reactor_poll_snapshot;
typedef struct { size_t size, alignment, fd, events, revents; } swr_v1_reactor_poll_layout_info;
swr_v1_reactor_poll_layout_info swr_v1_reactor_poll_layout(void);
/* translate_events_to_poll / translate_events_from_poll. */
int16_t swr_v1_reactor_poll_events_to(int events);
int swr_v1_reactor_poll_events_from(int16_t events);
swr_v1_reactor_poll_snapshot *swr_v1_reactor_poll_new(size_t);
void swr_v1_reactor_poll_free(swr_v1_reactor_poll_snapshot *);
/* Actual target libc pollfd storage, borrowed until owner destruction. */
struct pollfd *swr_v1_reactor_poll_data(swr_v1_reactor_poll_snapshot *);
/* The socket of each pollfd entry, borrowed likewise. */
void **swr_v1_reactor_poll_sockets(swr_v1_reactor_poll_snapshot *);
/* The fd -> socket registry (Reactor::sockets_); null reads as empty. put: 0, or -2 out of memory. */
typedef struct swr_v1_reactor_registry swr_v1_reactor_registry;
swr_v1_reactor_registry *swr_v1_reactor_registry_new(void);
void swr_v1_reactor_registry_free(swr_v1_reactor_registry *);
int swr_v1_reactor_registry_put(swr_v1_reactor_registry *, int fd, void *socket, int events);
void swr_v1_reactor_registry_set(swr_v1_reactor_registry *, int fd, int events); /* if registered */
void swr_v1_reactor_registry_remove(swr_v1_reactor_registry *, int fd);
void *swr_v1_reactor_registry_get(const swr_v1_reactor_registry *, int fd);
size_t swr_v1_reactor_registry_count(const swr_v1_reactor_registry *);
/* Visits a copy of the entries; 0, or -2 out of memory. */
int swr_v1_reactor_registry_foreach(const swr_v1_reactor_registry *, void *context, void (*visit)(void *, int fd, void *socket));
/* Rebuilds the snapshot from the registry; the entry count. */
int swr_v1_reactor_poll_fill(swr_v1_reactor_poll_snapshot *, const swr_v1_reactor_registry *);
typedef struct { int64_t seconds, nanoseconds; uint32_t finite; } swr_v1_reactor_kqueue_timeout_result;
// Independent zero state; requests10running/11EINTR/12uring-handler/13once.
swr_v1_reactor_kqueue_timeout_result swr_v1_reactor_kqueue_timeout(int32_t, uint32_t);
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
typedef struct { uint32_t offset, length, close_wait; } swr_v1_stream_view_action;
int32_t swr_v1_stream_prepare(uint8_t *, size_t);
int32_t swr_v1_stream_length(const uint8_t *, size_t);
swr_v1_stream_view_action swr_v1_stream_view(uint32_t);
/* network::Socket's buffer sizes, setsockopt included. */
bool swr_v1_socket_buffer_size(int fd, bool recv, bool send, uint32_t size);
/* network::Socket's timeout group (dns_timeout, connect_timeout, read_timeout,
 * write_timeout), embedded in this order (Socket static_asserts it). set_timeout
 * writes timeout into each SW_TIMEOUT_* bit of kind unless timeout is 0;
 * swr_v1_socket_timeout is the slot one SW_TIMEOUT_* value names, else NULL. */
typedef struct { double dns, connect, read, write; } swr_v1_socket_timeouts;
void swr_v1_socket_set_timeout(swr_v1_socket_timeouts *timeouts, double timeout, int kind);
struct sockaddr;
ssize_t swr_v1_socket_recvfrom(int fd, void *buffer, size_t length, int flags, struct sockaddr *address, socklen_t *address_length, uint32_t retries);
int swr_v1_socket_accept(int fd, struct sockaddr *address, socklen_t *address_length, bool nonblock, bool *listener_cloexec);
/* sendfile's range check (fstat included) and chunk size: 0 with *end set, 1
 * fstat failed (errno set), 2 empty file, 3 range outside the file. */
int swr_v1_socket_sendfile_range(int file_fd, off_t begin, size_t length, off_t *end);
size_t swr_v1_socket_sendfile_chunk(off_t begin, off_t end);
/* network::Socket's TLS state group (ssl_state, ssl_want_read, ssl_want_write),
 * embedded in this order and these types (Socket static_asserts it). The calls below
 * that take it set it. */
typedef struct {
    uint32_t state;
    uint8_t want_read;
    uint8_t want_write;
} swr_v1_socket_tls;
/* Socket::ssl_clear_error: ERR_clear_error and clear the want flags.
 * Socket::what_event_want: the event a pending TLS read or write waits for when ssl
 * is set, else default_event. */
void swr_v1_socket_ssl_clear(swr_v1_socket_tls *tls);
int swr_v1_socket_tls_event(const swr_v1_socket_tls *tls, bool ssl, int default_event);
/* _ssl_read_x509_file: a certificate as PEM into at most length bytes, warning and
 * returning -1 on failure; frees cert when consume. */
int swr_v1_socket_ssl_read_x509(struct x509_st *cert, char *buffer, size_t length, bool consume);
/* --fix-halfclose's close_notify and interrupted-sendfile repairs (SW_HALFCLOSE_TLS_SHUTDOWN). */
void swr_v1_socket_set_halfclose_repairs(bool repaired);
/* Socket::free, which defers through a std::function. */
void swr_v1_socket_set_free(void (*free)(struct swr_v1_socket *));
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
/* Sendfile requests own their file and range; Rust sends and frees them. */
typedef struct swr_v1_os_file_record swr_v1_os_file_record;
typedef struct swr_v1_socket_file_request swr_v1_socket_file_request;
typedef struct { int8_t corked; off_t begin; off_t end; } swr_v1_socket_file_fields;
typedef struct {
    swr_v1_socket_file_request *owner;
    swr_v1_os_file_record *file;
    swr_v1_socket_file_fields *fields;
} swr_v1_socket_file_prepared;
swr_v1_socket_file_prepared swr_v1_socket_file_new(const char *filename, off_t offset);
void swr_v1_socket_file_free(swr_v1_socket_file_request *owner);
int swr_v1_socket_file_check(swr_v1_socket_file_request *owner, size_t length);
/* Socket::sendfile_async's request, checked with check_sendfile_parameters' warnings
 * (null when it fails), and the typed queue's disposer for it. */
swr_v1_socket_file_request *swr_v1_socket_file_checked(const char *filename, off_t offset, size_t length);
void swr_v1_socket_file_dispose(void *token);
size_t swr_v1_socket_file_length(const swr_v1_socket_file_request *owner);
void swr_v1_socket_file_cork(swr_v1_socket_file_request *owner, const swr_v1_socket_output_hooks *hooks);
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
int swr_v1_socket_catch_error(int error, bool *kernel_nobufs);
/* catch_write_pipe_error: EMSGSIZE, and ENOBUFS on Linux (setting *kernel_nobufs),
 * reduce the size; anything else is swr_v1_socket_catch_error's. */
int swr_v1_socket_catch_pipe_error(int error, bool *kernel_nobufs);
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

/* One owner for Reactor public slots, registry, handlers and kernel resources. */
typedef struct swr_v1_reactor_state swr_v1_reactor_state;
typedef struct {
    int kind; void *ptr; int native_handle; uint32_t max_event_num;
    bool ready, running, start, once, wait_exit, destroyed, bailout, timed_out;
    int32_t timeout_msec; uint16_t id; uint32_t max_socket; time_t last_malloc_trim_time;
    swr_v1_reactor_write_fn write;
    swr_v1_reactor_writev_fn writev;
    swr_v1_reactor_close_fn close;
} swr_v1_reactor_fields;
typedef struct {
    swr_v1_reactor_state *owner; swr_v1_reactor_fields *fields;
    swr_v1_reactor_registry *registry; swr_v1_reactor_handlers *handlers;
} swr_v1_reactor_prepared;
typedef struct { void *events; void **sockets; uint32_t backend; } swr_v1_reactor_backend_view;
swr_v1_reactor_prepared swr_v1_reactor_state_new(void);
void swr_v1_reactor_state_free(swr_v1_reactor_state *);
int swr_v1_reactor_state_open(swr_v1_reactor_state *, int kind, int count);
void swr_v1_reactor_state_close_backend(swr_v1_reactor_state *);
swr_v1_reactor_backend_view swr_v1_reactor_state_backend(swr_v1_reactor_state *);
/* Reactor::wait() on the state's backend: 0 when it stops (or poll fails), -1 when epoll/kqueue fail. */
int32_t swr_v1_reactor_wait(swr_v1_reactor_state *, const swr_v1_reactor_wait_ops *);
int swr_v1_reactor_state_descriptor(const swr_v1_reactor_state *);
int swr_v1_reactor_state_socket(swr_v1_reactor_state *, void *socket, uint32_t op, int events);
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
static_assert(sizeof(swr_v1_reactor_builtin_reply) == 8, "reactor builtin reply ABI");
static_assert(sizeof(swr_v1_reactor_init_state) == 4, "reactor init state ABI");
static_assert(sizeof(swr_v1_reactor_backend_plan) == 12, "reactor backend plan ABI");
static_assert(sizeof(swr_v1_stream_view_action) == 12, "stream view ABI");
#include <functional>
namespace swoole {
class Reactor;
// The private member keeps Native payload effects outside Rust. Its placement
// before sockets_ preserves Native member destruction order after Reactor body.
class ReactorCallbacks {
    swr_v1_reactor_callbacks *owner_ = nullptr;
    template<class Function> Function *ensure(uint32_t kind, int32_t key);
    void clear() noexcept;
    void copy_from(const ReactorCallbacks &);
  public:
    using End = std::function<void(Reactor *)>;
    using Exit = std::function<bool(Reactor *, size_t &)>;
    ReactorCallbacks() = default;
    ReactorCallbacks(const ReactorCallbacks &);
    ReactorCallbacks &operator=(const ReactorCallbacks &);
    ~ReactorCallbacks() noexcept;
    void set_end(int32_t, const End &);
    void set_exit(int32_t, const Exit &);
    void erase_end(int32_t);
    size_t erase_exit(int32_t);
    bool has_exit(int32_t) const;
    void execute_end(Reactor *) const;
    bool if_exit(Reactor *, size_t &) const;
};
}
#endif
#endif
