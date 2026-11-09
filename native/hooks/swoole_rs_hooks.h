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
typedef struct { int32_t fd; int32_t events; int32_t revents; } swr_v1_hook_poll_fd;
/* System::socket_poll over count distinct fds, storing each one's revents. */
bool swr_v1_hook_socket_poll(swr_v1_hook_poll_fd *fds, size_t count, double timeout);
/* The thread's AsyncThreads, created first when it has none (null on failure). */
void *swr_v1_async_threads_ensure(void);
/* coroutine::dns_lookup: each address of domain pushed onto list, in order. */
void swr_v1_hook_dns_lookup(const char *domain, int family, double timeout, void (*push)(void *list, const char *address), void *list);
/* The coroutine Socket's connection state, Rust-owned behind an opaque handle (#439) in its record. */
typedef struct swr_v1_hook_socket_state swr_v1_hook_socket_state;
#ifdef __cplusplus
}
#endif
#endif
