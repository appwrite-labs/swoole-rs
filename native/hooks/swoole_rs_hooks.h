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
int64_t swr_v1_hook_transfer_result(int64_t result, uint64_t total);
typedef struct { uint64_t total; bool again; } swr_v1_hook_transfer_plan;
swr_v1_hook_transfer_plan swr_v1_hook_transfer(int64_t result, uint64_t total, uint64_t requested, bool wait);
typedef struct { uint32_t revents; bool wake; } swr_v1_hook_event_plan;
swr_v1_hook_event_plan swr_v1_hook_event_waiter(uint32_t received, uint32_t event);
typedef struct { int32_t fd; int32_t events; int32_t revents; } swr_v1_hook_poll_fd;
/* System::socket_poll over count distinct fds, storing each one's revents. */
bool swr_v1_hook_socket_poll(swr_v1_hook_poll_fd *fds, size_t count, double timeout);
/* What C++ keeps of the native coroutine socket (SocketImpl: coroutine::Socket, or
 * UringSocket with --enable-uring-socket): new and delete. Its members are Rust. */
typedef struct swr_v1_hook_socket_ops {
    void *(*open)(int domain, int type, int protocol); /* new SocketImpl(domain, type, protocol) */
    void *(*wrap)(int fd);                             /* new SocketImpl(fd, SW_SOCK_RAW) */
    void (*free)(void *socket);
} swr_v1_hook_socket_ops;
/* What the hooked functions (Rust, under upstream's swoole_coroutine_<name>) need
 * of native; registered at load (swr_v1_hook_set_host). */
typedef struct swr_v1_hook_host {
    const void *uring;             /* &swr_v1_uring_native with io_uring, else NULL */
    bool uring_statx;              /* HAVE_IOURING_STATX */
    bool uring_ftruncate;          /* HAVE_IOURING_FTRUNCATE */
    bool uring_futex;              /* HAVE_IOURING_FUTEX */
    swr_v1_hook_socket_ops socket;
} swr_v1_hook_host;
void swr_v1_hook_set_host(const swr_v1_hook_host *host);
double swr_v1_hook_timeval(double seconds, double micros);
typedef struct { uint32_t flags; bool conflict; } swr_v1_hook_curl_plan;
swr_v1_hook_curl_plan swr_v1_hook_curl_flags(uint32_t flags, uint32_t native, uint32_t php);
/* The thread's AsyncThreads, created first when it has none (null on failure). */
void *swr_v1_async_threads_ensure(void);
/* coroutine::dns_lookup: each address of domain pushed onto list, in order. */
void swr_v1_hook_dns_lookup(const char *domain, int family, double timeout, void (*push)(void *list, const char *address), void *list);
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
