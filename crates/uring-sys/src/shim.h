/* SPDX-License-Identifier: Apache-2.0 */
/* liburing, plus its static inline functions as linkable swr_uring_* ones:
 * bindgen sees declarations only. */
#ifndef SWOOLE_RS_URING_SHIM_H
#define SWOOLE_RS_URING_SHIM_H
#ifndef _GNU_SOURCE
#define _GNU_SOURCE
#endif
#include <liburing.h>

struct io_uring_sqe *swr_uring_get_sqe(struct io_uring *ring);
unsigned swr_uring_sq_space_left(const struct io_uring *ring);
void swr_uring_cq_advance(struct io_uring *ring, unsigned nr);
int swr_uring_opcode_supported(const struct io_uring_probe *p, int op);
void swr_uring_prep_link_timeout(struct io_uring_sqe *sqe, struct __kernel_timespec *ts, unsigned flags);
void swr_uring_prep_timeout(struct io_uring_sqe *sqe, struct __kernel_timespec *ts, unsigned count, unsigned flags);
void swr_uring_prep_cancel(struct io_uring_sqe *sqe, void *user_data, int flags);
void swr_uring_prep_open(struct io_uring_sqe *sqe, const char *path, int flags, mode_t mode);
void swr_uring_prep_connect(struct io_uring_sqe *sqe, int fd, const struct sockaddr *addr, socklen_t addrlen);
void swr_uring_prep_accept(struct io_uring_sqe *sqe, int fd, struct sockaddr *addr, socklen_t *addrlen, int flags);
void swr_uring_prep_recv(struct io_uring_sqe *sqe, int sockfd, void *buf, size_t len, int flags);
void swr_uring_prep_send(struct io_uring_sqe *sqe, int sockfd, const void *buf, size_t len, int flags);
void swr_uring_prep_recvmsg(struct io_uring_sqe *sqe, int fd, struct msghdr *msg, unsigned flags);
void swr_uring_prep_sendmsg(struct io_uring_sqe *sqe, int fd, const struct msghdr *msg, unsigned flags);
void swr_uring_prep_sendto(struct io_uring_sqe *sqe, int sockfd, const void *buf, size_t len, int flags,
                           const struct sockaddr *addr, socklen_t addrlen);
void swr_uring_prep_readv(struct io_uring_sqe *sqe, int fd, const struct iovec *iovecs, unsigned nr_vecs,
                          __u64 offset);
void swr_uring_prep_writev(struct io_uring_sqe *sqe, int fd, const struct iovec *iovecs, unsigned nr_vecs,
                           __u64 offset);
void swr_uring_prep_read(struct io_uring_sqe *sqe, int fd, void *buf, unsigned nbytes, __u64 offset);
void swr_uring_prep_write(struct io_uring_sqe *sqe, int fd, const void *buf, unsigned nbytes, __u64 offset);
void swr_uring_prep_splice(struct io_uring_sqe *sqe, int fd_in, int64_t off_in, int fd_out, int64_t off_out,
                           unsigned nbytes, unsigned splice_flags);
void swr_uring_prep_close(struct io_uring_sqe *sqe, int fd);
void swr_uring_prep_rename(struct io_uring_sqe *sqe, const char *oldpath, const char *newpath);
void swr_uring_prep_mkdir(struct io_uring_sqe *sqe, const char *path, mode_t mode);
void swr_uring_prep_unlink(struct io_uring_sqe *sqe, const char *path, int flags);
void swr_uring_prep_fsync(struct io_uring_sqe *sqe, int fd, unsigned fsync_flags);
void swr_uring_prep_ftruncate(struct io_uring_sqe *sqe, int fd, loff_t len);
void swr_uring_prep_statx(struct io_uring_sqe *sqe, int dfd, const char *path, int flags, unsigned mask,
                          struct statx *statxbuf);
void swr_uring_prep_futex_wait(struct io_uring_sqe *sqe, uint32_t *futex, uint64_t val, uint64_t mask,
                               uint32_t futex_flags, unsigned flags);
void swr_uring_prep_futex_wake(struct io_uring_sqe *sqe, uint32_t *futex, uint64_t val, uint64_t mask,
                               uint32_t futex_flags, unsigned flags);
void swr_uring_prep_waitid(struct io_uring_sqe *sqe, idtype_t idtype, id_t id, siginfo_t *infop, int options,
                           unsigned flags);
void swr_uring_prep_poll_add(struct io_uring_sqe *sqe, int fd, unsigned poll_mask);
#endif
