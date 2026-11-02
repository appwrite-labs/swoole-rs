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
#include <sys/types.h>
#include <sys/types.h>
#ifdef __cplusplus
extern "C" {
#endif
typedef bool (*swr_v1_hook_offload)(void (*job)(void *), void *context);
FILE *swr_v1_hook_fopen(const char *pathname, const char *mode, swr_v1_hook_offload offload);
FILE *swr_v1_hook_fdopen(int fd, const char *mode, swr_v1_hook_offload offload);
FILE *swr_v1_hook_freopen(const char *pathname, const char *mode, FILE *stream, swr_v1_hook_offload offload);
size_t swr_v1_hook_fread(void *ptr, size_t size, size_t nmemb, FILE *stream, swr_v1_hook_offload offload);
size_t swr_v1_hook_fwrite(const void *ptr, size_t size, size_t nmemb, FILE *stream, swr_v1_hook_offload offload);
char *swr_v1_hook_fgets(char *s, int size, FILE *stream, swr_v1_hook_offload offload);
int swr_v1_hook_fputs(const char *s, FILE *stream, swr_v1_hook_offload offload);
int swr_v1_hook_feof(FILE *stream, swr_v1_hook_offload offload);
int swr_v1_hook_fflush(FILE *stream, swr_v1_hook_offload offload);
int swr_v1_hook_fclose(FILE *stream, swr_v1_hook_offload offload);
DIR *swr_v1_hook_opendir(const char *name, swr_v1_hook_offload offload);
struct dirent *swr_v1_hook_readdir(DIR *dirp, swr_v1_hook_offload offload);
int swr_v1_hook_closedir(DIR *dirp, swr_v1_hook_offload offload);
int swr_v1_hook_open(const char *pathname, int flags, mode_t mode, swr_v1_hook_offload offload);
int swr_v1_hook_fstat(int fd, struct stat *statbuf, swr_v1_hook_offload offload);
int swr_v1_hook_stat(const char *path, struct stat *statbuf, swr_v1_hook_offload offload);
int swr_v1_hook_lstat(const char *path, struct stat *statbuf, swr_v1_hook_offload offload);
int swr_v1_hook_unlink(const char *pathname, swr_v1_hook_offload offload);
int swr_v1_hook_mkdir(const char *pathname, mode_t mode, swr_v1_hook_offload offload);
int swr_v1_hook_rmdir(const char *pathname, swr_v1_hook_offload offload);
int swr_v1_hook_rename(const char *oldpath, const char *newpath, swr_v1_hook_offload offload);
int swr_v1_hook_fsync(int fd, swr_v1_hook_offload offload);
int swr_v1_hook_fdatasync(int fd, swr_v1_hook_offload offload);
int swr_v1_hook_ftruncate(int fd, off_t length, swr_v1_hook_offload offload);
off_t swr_v1_hook_lseek(int fd, off_t offset, int whence, swr_v1_hook_offload offload);
ssize_t swr_v1_hook_readlink(const char *pathname, char *buf, size_t len, swr_v1_hook_offload offload);
int swr_v1_hook_statvfs(const char *path, struct statvfs *buf, swr_v1_hook_offload offload);
int swr_v1_hook_access(const char *pathname, int mode, swr_v1_hook_offload offload);
bool swr_v1_hook_no_coro(bool reactor, bool coroutine);
int32_t swr_v1_hook_transition(uint32_t requested, uint32_t installed, uint32_t bit);
uint32_t swr_v1_hook_filter(uint32_t flags, uint32_t unavailable);
bool swr_v1_hook_file_result(bool open, int64_t result);
typedef struct { int32_t result; bool invalid; } swr_v1_hook_flock_outcome;
swr_v1_hook_flock_outcome swr_v1_hook_coroutine_flock(int32_t fd, int32_t operation, int (*coroutine_sleep)(double), double initial, double maximum);
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
enum swr_hook_timer_action { SWR_HOOK_TIMER_ENABLE = 1, SWR_HOOK_TIMER_ADD = 2, SWR_HOOK_TIMER_SENTINEL = 4, SWR_HOOK_TIMER_DELETE = 8, SWR_HOOK_TIMER_CLEAR = 16 };
uint32_t swr_v1_hook_timer(uint32_t operation, bool enabled, bool present, bool sentinel, double timeout);
enum swr_hook_wait_action { SWR_HOOK_WAIT_DONE = 0, SWR_HOOK_WAIT_CALLBACK = 1, SWR_HOOK_WAIT_SLEEP = 2, SWR_HOOK_WAIT_CANCELED = -1, SWR_HOOK_WAIT_INVALID = -2 };
typedef struct { double delay; uint32_t phase; int32_t action; } swr_v1_hook_wait_plan;
swr_v1_hook_wait_plan swr_v1_hook_wait_for(uint32_t phase, double delay, bool result, double initial, double maximum);
typedef struct { uint32_t revents; bool wake; } swr_v1_hook_event_plan;
swr_v1_hook_event_plan swr_v1_hook_event_waiter(uint32_t received, uint32_t event);
typedef struct { int32_t result; int32_t last_error; int32_t error; } swr_v1_hook_wait_event_outcome;
swr_v1_hook_wait_event_outcome swr_v1_hook_wait_event(int32_t fd, int32_t events, double timeout, int (*wait)(int fd, int events, double timeout, int *revents));
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
void swr_v1_hook_poll_ready(void *task, int fd, int event);
void swr_v1_hook_poll_completed(void *task);
void swr_v1_hook_poll_timeout(void *task);
bool swr_v1_hook_socket_poll(swr_v1_hook_poll_fd *fds, size_t count, double timeout, const swr_v1_hook_poll_ops *ops);
int swr_v1_hook_poll(struct pollfd *fds, nfds_t nfds, int timeout, const swr_v1_hook_poll_ops *ops);
void swr_v1_hook_dns_cache_expire(int64_t expire);
void swr_v1_hook_dns_cache_capacity(size_t capacity);
void swr_v1_hook_dns_cache_clear(void);
float swr_v1_hook_dns_cache_hit_ratio(void);
bool swr_v1_hook_dns_cache_get(int domain, const char *name, size_t length, void *out, void (*assign)(void *out, const char *address, size_t length));
void swr_v1_hook_dns_cache_set(int domain, const char *name, size_t length, const char *address, size_t address_length);
uint32_t swr_v1_hook_socket_status(bool closed, bool close_wait);
uint32_t swr_v1_hook_check_result(int64_t result, bool has_error);
double swr_v1_hook_timeval(double seconds, double micros);
uint32_t swr_v1_hook_timeout_action(double timeout, double startup);
typedef struct { double remaining; bool expired; } swr_v1_hook_timeout_plan;
swr_v1_hook_timeout_plan swr_v1_hook_timeout_remaining(double timeout, double elapsed, double minimum);
typedef struct { uint32_t flags; bool conflict; } swr_v1_hook_curl_plan;
swr_v1_hook_curl_plan swr_v1_hook_curl_flags(uint32_t flags, uint32_t native, uint32_t php);
enum swr_hook_async_action { SWR_HOOK_ASYNC_REJECT = 0, SWR_HOOK_ASYNC_YIELD = 1, SWR_HOOK_ASYNC_COMPLETE = 2, SWR_HOOK_ASYNC_CANCEL = 3, SWR_HOOK_ASYNC_RESUME = 4 };
int32_t swr_v1_hook_async_action(uint32_t phase, bool cancellable, bool success);
int32_t swr_v1_hook_dns_error(int32_t error, int32_t asynchronous_timeout, int32_t resolve_timeout);
double swr_v1_hook_sleep_timeout(double seconds, double minimum);
/* Socket::recv_packet after its availability check, read timer and get_read_buffer.
   buffer is the read buffer: swoole::String's leading fields. fail: 0 no package
   length, 1 too large (warning naming the length), 2 no EOF within the maximum. */
typedef struct { size_t length, size; int64_t offset; char *str; } swr_v1_hook_read_buffer;
typedef struct {
    void *context;
    swr_v1_hook_read_buffer *buffer;
    bool length_check, eof_check;
    const char *eof;
    uint8_t eof_len;
    char length_type;
    uint8_t length_size;
    uint16_t length_offset;
    uint32_t max_length;
    ssize_t (*recv)(void *, char *, size_t);
    ssize_t (*recv_all)(void *, char *, size_t);
    ssize_t (*package_length)(void *, const char *, uint32_t, uint32_t *header_len);
    void (*extend)(void *, size_t);
    void (*reduce)(void *, int64_t);
    void (*fail)(void *, uint32_t kind, ssize_t length);
    bool (*timed_out)(void *);
} swr_v1_hook_packet_ops;
ssize_t swr_v1_hook_recv_packet(const swr_v1_hook_packet_ops *ops);
/* Socket::readv_all/writev_all after the availability check and the timer's creation.
   wait installs barrier (called with barrier_state) and runs timer.start() && wait_event. */
typedef bool (*swr_v1_hook_barrier)(void *barrier_state);
typedef struct {
    void *context;
    ssize_t (*transfer)(void *, bool first);
    bool (*would_block)(void *);
    size_t (*remaining)(void *);
    void (*fail)(void *);
    bool (*wait)(void *, swr_v1_hook_barrier barrier, void *barrier_state);
    void (*check)(void *, ssize_t);
} swr_v1_hook_vector_ops;
ssize_t swr_v1_hook_vector_all(const swr_v1_hook_vector_ops *ops);
/* The coroutine Socket's transfers: swr_v1_hook_io retries one call while the socket would
   block; swr_v1_hook_io_all is recv_all/send_all. wait installs barrier (when given) and waits;
   buffer, when not null, is the caller's buffer slot that a write's wait may move. */
typedef struct {
    void *context;
    bool retry_interrupted;
    ssize_t (*call)(void *, char *buffer, size_t length);
    bool (*would_block)(void *);
    bool (*start)(void *);
    bool (*wait)(void *, swr_v1_hook_barrier barrier, void *barrier_state, char **buffer);
    void (*check)(void *, ssize_t);
    void (*fail)(void *);
} swr_v1_hook_transfer_ops;
ssize_t swr_v1_hook_io(const swr_v1_hook_transfer_ops *ops, char *buffer, size_t length);
ssize_t swr_v1_hook_io_all(const swr_v1_hook_transfer_ops *ops, char *buffer, size_t length);
/* The coroutine Socket's connection state, Rust-owned behind an opaque handle (#439). */
typedef struct swr_v1_hook_socket_state swr_v1_hook_socket_state;
typedef struct { bool connected, read, write; } swr_v1_hook_socket_flags_t;
swr_v1_hook_socket_state *swr_v1_hook_socket_state_new(void);
void swr_v1_hook_socket_state_free(swr_v1_hook_socket_state *state);
swr_v1_hook_socket_flags_t swr_v1_hook_socket_flags(const swr_v1_hook_socket_state *state);
void swr_v1_hook_socket_set_flags(swr_v1_hook_socket_state *state, swr_v1_hook_socket_flags_t flags);
void swr_v1_hook_socket_connect(swr_v1_hook_socket_state *state);
/* Server setup (crates/hooks/src/setup.rs): bind (host: availability first, then
 * record the name), listen (SW_BACKLOG for a non-positive backlog, stored in
 * *stored) and accept (wait and accept again when it would block). fail(-1) is
 * set_err(); call is the operation's network::Socket call. */
typedef struct {
    void *context;
    bool (*available)(void *, int event);
    void (*fail)(void *, int error);
    bool (*wait)(void *, int event);
    int (*name)(void *);
    int (*call)(void *, int argument);
} swr_v1_hook_setup_ops;
bool swr_v1_hook_bind(const swr_v1_hook_setup_ops *ops, bool host);
bool swr_v1_hook_listen(const swr_v1_hook_setup_ops *ops, int backlog, int *stored);
bool swr_v1_hook_accept(const swr_v1_hook_setup_ops *ops);
/* Coroutine Socket::sendfile's loop over [*offset, end): 0 all sent, 1 a send
 * returned zero, 2 one failed (errno set), 3 the wait failed. */
typedef struct {
    void *context;
    ssize_t (*send)(void *, off_t *offset, size_t length);
    bool (*wait)(void *);
} swr_v1_hook_sendfile_ops;
int swr_v1_hook_sendfile(const swr_v1_hook_sendfile_ops *ops, off_t *offset, size_t end);
/* connect(host)/sendto(host)'s address loop: up to two assigns with a resolve
 * between them on SW_ERROR_BAD_HOST_ADDR. 0 assigned (or fell through), 1 an
 * assign failed otherwise, 2 the resolve failed. */
typedef struct {
    void *context;
    bool (*assign)(void *);
    bool (*bad_host)(void);
    bool (*resolve)(void *);
} swr_v1_hook_resolve_ops;
int swr_v1_hook_resolve(const swr_v1_hook_resolve_ops *ops);
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
/* The socket's read and write timer slots (crates/hooks/src/timers.rs): the slot for event's
 * direction (SW_EVENT_READ, else write), and on expiry the cleared slot's event, or 0 for neither. */
void **swr_v1_hook_socket_timer(swr_v1_hook_socket_state *state, uint32_t event);
uint32_t swr_v1_hook_socket_timeout(swr_v1_hook_socket_state *state, void *node);
/* TimeoutSetter: save and set (restore false), or restore, each SW_TIMEOUT_* type in types;
 * original holds timeout_type_list's four entries. */
typedef struct {
    double (*get)(void *, int type);
    void (*set)(void *, double timeout, int type);
} swr_v1_hook_timeout_ops;
void swr_v1_hook_timeouts(const swr_v1_hook_timeout_ops *ops, void *socket, double timeout, int types, double *original,
                          bool restore);
/* Socket::connect(addr, addrlen): connect(2) on fd, waiting to write when in progress; so_error
 * reads SO_ERROR into errCode, error returns errCode. */
typedef struct {
    void *context;
    swr_v1_hook_socket_state *state;
    int fd;
    bool (*available)(void *);
    void (*set_err)(void *, int code);
    bool (*closed)(void *);
    bool (*wait)(void *);
    int (*so_error)(void *);
    int (*error)(void *);
    void (*name)(void *);
} swr_v1_hook_connect_addr_ops;
bool swr_v1_hook_connect_addr(const swr_v1_hook_connect_addr_ops *ops, const struct sockaddr *addr, socklen_t length);
/* The coroutine bound for reading or writing (event SW_EVENT_READ/WRITE; bind accepts both bits).
   bound_co follows get_bound_co; event_name returns 0 reading, 1 writing, 2 reading or writing. */
void *swr_v1_hook_socket_co(const swr_v1_hook_socket_state *state, uint32_t event);
void swr_v1_hook_socket_bind(swr_v1_hook_socket_state *state, uint32_t event, void *co);
void *swr_v1_hook_socket_bound_co(const swr_v1_hook_socket_state *state, uint32_t event);
uint32_t swr_v1_hook_socket_event_name(const swr_v1_hook_socket_state *state, uint32_t event);
/* Socket::cancel/close. release marks the descriptor closed and runs the destructor
   callback once; the socket may be gone afterwards. resume wakes the coroutine bound
   to the event (read or write). */
typedef struct {
    void *context;
    swr_v1_hook_socket_state *state;
    uint32_t read, write;
    int32_t close_wait_error;
    bool (*closed)(void *);
    void (*shutdown)(void *);
    bool (*bound)(void *, uint32_t event);
    void (*set_err)(void *, int32_t);
    void (*resume)(void *, uint32_t event);
    void (*close_wait)(void *);
    void (*release)(void *);
} swr_v1_hook_socket_ops;
bool swr_v1_hook_cancel(const swr_v1_hook_socket_ops *ops, uint32_t event);
bool swr_v1_hook_close(const swr_v1_hook_socket_ops *ops);
/* Socket::shutdown(how) after set_err(0): 0, or the error to set. closed is is_closed();
   ssl_shutdown (null without TLS) closes the session first, with context; syscall is shutdown(2). */
int32_t swr_v1_hook_shutdown(int (*syscall)(int, int), int32_t fd, int32_t how, bool closed, swr_v1_hook_socket_state *state,
                             void (*ssl_shutdown)(void *), void *context);
/* Socket::wait_event for the current coroutine co, and the readable/writable callbacks' choice of
   coroutine to resume (null for none). barrier (when given) stays installed for the wait; buffer,
   when not null, is a write's buffer slot. status reports close_wait, else has_kernel_nobufs(),
   then the TLS wants, removed, and events' SW_EVENT_READ/WRITE bits. */
enum {
    SWR_V1_HOOK_WAIT_CLOSE_WAIT = 1,
    SWR_V1_HOOK_WAIT_NOBUFS = 2,
    SWR_V1_HOOK_WAIT_WANT_READ = 4,
    SWR_V1_HOOK_WAIT_WANT_WRITE = 8,
    SWR_V1_HOOK_WAIT_REMOVED = 16,
};
typedef struct {
    void *context;
    swr_v1_hook_socket_state *state;
    int32_t close_wait_error;
    uint32_t (*status)(void *);
    bool (*closed)(void *);
    int32_t (*error)(void *);
    void (*set_err)(void *, int32_t);
    void (*sleep)(void *);
    bool (*reactor)(void *, uint32_t events, bool add);
    void (*suspend)(void *, void *co, uint32_t event);
    void (*buffer)(void *, char **buffer, size_t length);
    void (*remove)(void *, uint32_t event);
} swr_v1_hook_wait_ops;
bool swr_v1_hook_socket_wait(const swr_v1_hook_wait_ops *ops, void *co, uint32_t event, swr_v1_hook_barrier barrier,
                             void *barrier_state, char **buffer, size_t length);
void *swr_v1_hook_socket_ready(swr_v1_hook_socket_state *state, uint32_t event, bool hup);
/* Socket::recv_with_buffer after the availability check, over the read buffer's fields, and
   recv_line over recv_with_buffer (read). recv may yield; the fields stay current across it. */
typedef struct {
    char *str;
    size_t size;
    size_t *length;
    off_t *offset;
} swr_v1_hook_buffer_fields;
typedef ssize_t (*swr_v1_hook_read)(void *, char *buffer, size_t length);
ssize_t swr_v1_hook_recv_buffered(const swr_v1_hook_buffer_fields *buffer, swr_v1_hook_read recv, void *context, char *out,
                                  size_t length);
ssize_t swr_v1_hook_recv_line(swr_v1_hook_read read, void *context, char *out, size_t limit);
int32_t swr_v1_hook_connect_action(int32_t result, bool interrupted, bool in_progress);
bool swr_v1_hook_connect_option(int32_t result, int32_t error);
int32_t swr_v1_hook_liveness_error(bool alive, int32_t error, int32_t reset);
#ifdef __cplusplus
}
// Callables stay on the native stack. Rust returns before every IO, errno
// classifier, timer operation or coroutine suspension; exceptions remain native.
static_assert(sizeof(swr_v1_hook_curl_plan) == 8, "curl plan ABI");
static_assert(sizeof(swr_v1_hook_timeout_plan) == 16, "timeout plan ABI");
static_assert(sizeof(swr_v1_hook_wait_plan) == 16, "wait cursor ABI");
static_assert(sizeof(swr_v1_hook_event_plan) == 8, "event waiter ABI");
static_assert(sizeof(swr_v1_hook_transfer_plan) == 16, "transfer ABI");
#endif
#endif
