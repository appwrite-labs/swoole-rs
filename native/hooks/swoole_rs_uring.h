/* SPDX-License-Identifier: Apache-2.0 */
/* The io_uring engine (crates/hooks/src/uring.rs), built with --enable-iouring.
 * Rust owns the ring, its queues and every request; the host table gives it the
 * coroutine switches, the thread's engine and the log lines. */
#ifndef SWOOLE_RS_URING_H
#define SWOOLE_RS_URING_H
/* The archive exports these only when the build enables io_uring. */
#ifdef SW_USE_IOURING
#include <stddef.h>
#include <stdint.h>
#include <stdbool.h>
#include <poll.h>
#include <sys/types.h>
#include <sys/socket.h>
#include <sys/stat.h>
#include <sys/uio.h>
#ifdef __cplusplus
extern "C" {
#endif
typedef struct swr_v1_uring swr_v1_uring;
typedef struct {
    void *(*current)(void);
    swr_v1_uring *(*instance)(void);
    void (*suspend)(void *coroutine, void *event);
    void (*resume)(void *coroutine);
    bool (*event_running)(void);
    void (*submit_failed)(void);
    void (*timeout_full)(int ring_fd);
} swr_v1_uring_host;
typedef struct {
    uint32_t entries, flag, workers;
    bool probe_futex, probe_ftruncate;
} swr_v1_uring_config;
/* Status 0 ready; 1 liburing older than built against (fatal); 2 ring creation
 * failed, errno set, no engine; 3 probe failed, 4 no futex, 5 no ftruncate
 * (fatal); 6 the async worker limit failed, errno set. */
typedef struct {
    swr_v1_uring *uring;
    int status, major, minor, runtime_major, runtime_minor;
} swr_v1_uring_opened;
swr_v1_uring_opened swr_v1_uring_open(swr_v1_uring_config config, const swr_v1_uring_host *host);
void swr_v1_uring_close(swr_v1_uring *uring);
int swr_v1_uring_fd(const swr_v1_uring *uring);
uint32_t swr_v1_uring_task_num(const swr_v1_uring *uring);
size_t swr_v1_uring_waiting_num(const swr_v1_uring *uring);
float swr_v1_uring_sq_usage_percent(const swr_v1_uring *uring);
void swr_v1_uring_submit(swr_v1_uring *uring);
bool swr_v1_uring_wakeup(swr_v1_uring *uring);
bool swr_v1_uring_cancel(const swr_v1_uring_host *host, void *event);
int swr_v1_uring_open_file(const swr_v1_uring_host *host, const char *pathname, int flags, mode_t mode);
int swr_v1_uring_socket(const swr_v1_uring_host *host, int domain, int kind, int protocol, unsigned int flags);
int swr_v1_uring_connect(const swr_v1_uring_host *host, int fd, const struct sockaddr *addr, socklen_t len, double timeout);
int swr_v1_uring_accept(const swr_v1_uring_host *host, int fd, struct sockaddr *addr, socklen_t *len, int flags, double timeout);
int swr_v1_uring_bind(int fd, const struct sockaddr *addr, socklen_t len);
int swr_v1_uring_listen(int fd, int backlog);
int swr_v1_uring_sleep(const swr_v1_uring_host *host, double seconds);
ssize_t swr_v1_uring_recv(const swr_v1_uring_host *host, int fd, void *buf, size_t len, int flags, double timeout);
ssize_t swr_v1_uring_send(const swr_v1_uring_host *host, int fd, const void *buf, size_t len, int flags, double timeout);
ssize_t swr_v1_uring_recvmsg(const swr_v1_uring_host *host, int fd, struct msghdr *message, int flags, double timeout);
ssize_t swr_v1_uring_sendmsg(const swr_v1_uring_host *host, int fd, const struct msghdr *message, int flags, double timeout);
ssize_t swr_v1_uring_recvfrom(const swr_v1_uring_host *host, int fd, void *buf, size_t n, struct sockaddr *addr, socklen_t *len, double timeout);
/* splice_pages: MSG_SPLICE_PAGES when the build's headers define it, else 0. */
ssize_t swr_v1_uring_sendfile(const swr_v1_uring_host *host, int out_fd, int in_fd, off_t *offset, size_t size, double timeout, int splice_pages);
ssize_t swr_v1_uring_readv(const swr_v1_uring_host *host, int fd, const struct iovec *iov, int count, double timeout);
ssize_t swr_v1_uring_writev(const swr_v1_uring_host *host, int fd, const struct iovec *iov, int count, double timeout);
int swr_v1_uring_shutdown(const swr_v1_uring_host *host, int fd, int how);
int swr_v1_uring_close_fd(const swr_v1_uring_host *host, int fd);
ssize_t swr_v1_uring_read(const swr_v1_uring_host *host, int fd, void *buf, size_t size, double timeout);
ssize_t swr_v1_uring_write(const swr_v1_uring_host *host, int fd, const void *buf, size_t size, double timeout);
int swr_v1_uring_rename(const swr_v1_uring_host *host, const char *oldpath, const char *newpath);
int swr_v1_uring_mkdir(const swr_v1_uring_host *host, const char *pathname, mode_t mode);
int swr_v1_uring_unlink(const swr_v1_uring_host *host, const char *pathname);
int swr_v1_uring_rmdir(const swr_v1_uring_host *host, const char *pathname);
int swr_v1_uring_fsync(const swr_v1_uring_host *host, int fd);
int swr_v1_uring_fdatasync(const swr_v1_uring_host *host, int fd);
int swr_v1_uring_ftruncate(const swr_v1_uring_host *host, int fd, off_t length);
int swr_v1_uring_fstat(const swr_v1_uring_host *host, int fd, struct stat *stat);
int swr_v1_uring_stat(const swr_v1_uring_host *host, const char *path, struct stat *stat);
int swr_v1_uring_futex_wait(const swr_v1_uring_host *host, uint32_t *futex);
int swr_v1_uring_futex_wakeup(const swr_v1_uring_host *host, uint32_t *futex);
pid_t swr_v1_uring_waitpid(const swr_v1_uring_host *host, pid_t pid, int *stat_loc, int options, double timeout);
int swr_v1_uring_poll(const swr_v1_uring_host *host, struct pollfd *fd, int timeout);
#ifdef __cplusplus
}
#endif
#endif
#endif
