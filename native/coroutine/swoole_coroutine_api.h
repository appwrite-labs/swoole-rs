/* SPDX-License-Identifier: Apache-2.0 */
/* Swoole's include/swoole_coroutine_api.h, owned here: the swoole_coroutine_* functions
 * (all Rust exports) that C++, C and the compiled core-tests still call through this
 * header. The hook headers (native/hooks/swoole_{file,socket}_hook.h) declare their own. */
#pragma once

#include <sys/socket.h>
#include <sys/stat.h>
#include <sys/types.h>
#include <netdb.h>
#include <stdint.h>

#ifdef __cplusplus
extern "C" {
#endif

long swoole_coroutine_create(void (*routine)(void *), void *arg);
uint8_t swoole_coroutine_is_in(void);
long swoole_coroutine_get_id(void);
void swoole_coroutine_usleep(int usec);

int swoole_coroutine_open(const char *pathname, int flags, mode_t mode);
ssize_t swoole_coroutine_read(int fd, void *buf, size_t count);
ssize_t swoole_coroutine_write(int fd, const void *buf, size_t count);
off_t swoole_coroutine_lseek(int fd, off_t offset, int whence);
int swoole_coroutine_fstat(int fd, struct stat *statbuf);
int swoole_coroutine_close(int fd);
int swoole_coroutine_fsync(int fd);
int swoole_coroutine_ftruncate(int fd, off_t length);

int swoole_coroutine_socket(int domain, int type, int protocol);
int swoole_coroutine_getaddrinfo(const char *name, const char *service, const struct addrinfo *req, struct addrinfo **pai);
struct hostent *swoole_coroutine_gethostbyname(const char *name);

#ifdef __cplusplus
}
#endif
