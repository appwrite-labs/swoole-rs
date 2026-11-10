/* SPDX-License-Identifier: Apache-2.0 */
/* liburing's static inline functions. Stays C (docs/zend.md, What stays C):
 * private (the ring internals get_sqe and cq_advance touch change between
 * liburing releases) and budget (each prep_* fills an SQE through a raw pointer). */
#include "shim.h"

struct io_uring_sqe *swr_uring_get_sqe(struct io_uring *ring) {
    return io_uring_get_sqe(ring);
}

unsigned swr_uring_sq_space_left(const struct io_uring *ring) {
    return io_uring_sq_space_left(ring);
}

void swr_uring_cq_advance(struct io_uring *ring, unsigned nr) {
    io_uring_cq_advance(ring, nr);
}

int swr_uring_opcode_supported(const struct io_uring_probe *p, int op) {
    return io_uring_opcode_supported(p, op);
}

void swr_uring_prep_link_timeout(struct io_uring_sqe *sqe, struct __kernel_timespec *ts, unsigned flags) {
    io_uring_prep_link_timeout(sqe, ts, flags);
}

void swr_uring_prep_timeout(struct io_uring_sqe *sqe, struct __kernel_timespec *ts, unsigned count, unsigned flags) {
    io_uring_prep_timeout(sqe, ts, count, flags);
}

void swr_uring_prep_cancel(struct io_uring_sqe *sqe, void *user_data, int flags) {
    io_uring_prep_cancel(sqe, user_data, flags);
}

void swr_uring_prep_open(struct io_uring_sqe *sqe, const char *path, int flags, mode_t mode) {
    io_uring_prep_open(sqe, path, flags, mode);
}

void swr_uring_prep_connect(struct io_uring_sqe *sqe, int fd, const struct sockaddr *addr, socklen_t addrlen) {
    io_uring_prep_connect(sqe, fd, addr, addrlen);
}

void swr_uring_prep_accept(struct io_uring_sqe *sqe, int fd, struct sockaddr *addr, socklen_t *addrlen, int flags) {
    io_uring_prep_accept(sqe, fd, addr, addrlen, flags);
}

void swr_uring_prep_recv(struct io_uring_sqe *sqe, int sockfd, void *buf, size_t len, int flags) {
    io_uring_prep_recv(sqe, sockfd, buf, len, flags);
}

void swr_uring_prep_send(struct io_uring_sqe *sqe, int sockfd, const void *buf, size_t len, int flags) {
    io_uring_prep_send(sqe, sockfd, buf, len, flags);
}

void swr_uring_prep_recvmsg(struct io_uring_sqe *sqe, int fd, struct msghdr *msg, unsigned flags) {
    io_uring_prep_recvmsg(sqe, fd, msg, flags);
}

void swr_uring_prep_sendmsg(struct io_uring_sqe *sqe, int fd, const struct msghdr *msg, unsigned flags) {
    io_uring_prep_sendmsg(sqe, fd, msg, flags);
}

void swr_uring_prep_sendto(struct io_uring_sqe *sqe, int sockfd, const void *buf, size_t len, int flags, const struct sockaddr *addr, socklen_t addrlen) {
    io_uring_prep_sendto(sqe, sockfd, buf, len, flags, addr, addrlen);
}

void swr_uring_prep_readv(struct io_uring_sqe *sqe, int fd, const struct iovec *iovecs, unsigned nr_vecs, __u64 offset) {
    io_uring_prep_readv(sqe, fd, iovecs, nr_vecs, offset);
}

void swr_uring_prep_writev(struct io_uring_sqe *sqe, int fd, const struct iovec *iovecs, unsigned nr_vecs, __u64 offset) {
    io_uring_prep_writev(sqe, fd, iovecs, nr_vecs, offset);
}

void swr_uring_prep_read(struct io_uring_sqe *sqe, int fd, void *buf, unsigned nbytes, __u64 offset) {
    io_uring_prep_read(sqe, fd, buf, nbytes, offset);
}

void swr_uring_prep_write(struct io_uring_sqe *sqe, int fd, const void *buf, unsigned nbytes, __u64 offset) {
    io_uring_prep_write(sqe, fd, buf, nbytes, offset);
}

void swr_uring_prep_splice(struct io_uring_sqe *sqe, int fd_in, int64_t off_in, int fd_out, int64_t off_out, unsigned nbytes, unsigned splice_flags) {
    io_uring_prep_splice(sqe, fd_in, off_in, fd_out, off_out, nbytes, splice_flags);
}

void swr_uring_prep_close(struct io_uring_sqe *sqe, int fd) {
    io_uring_prep_close(sqe, fd);
}

void swr_uring_prep_rename(struct io_uring_sqe *sqe, const char *oldpath, const char *newpath) {
    io_uring_prep_rename(sqe, oldpath, newpath);
}

void swr_uring_prep_mkdir(struct io_uring_sqe *sqe, const char *path, mode_t mode) {
    io_uring_prep_mkdir(sqe, path, mode);
}

void swr_uring_prep_unlink(struct io_uring_sqe *sqe, const char *path, int flags) {
    io_uring_prep_unlink(sqe, path, flags);
}

void swr_uring_prep_fsync(struct io_uring_sqe *sqe, int fd, unsigned fsync_flags) {
    io_uring_prep_fsync(sqe, fd, fsync_flags);
}

void swr_uring_prep_ftruncate(struct io_uring_sqe *sqe, int fd, loff_t len) {
    io_uring_prep_ftruncate(sqe, fd, len);
}

void swr_uring_prep_statx(struct io_uring_sqe *sqe, int dfd, const char *path, int flags, unsigned mask, struct statx *statxbuf) {
    io_uring_prep_statx(sqe, dfd, path, flags, mask, statxbuf);
}

void swr_uring_prep_futex_wait(struct io_uring_sqe *sqe, uint32_t *futex, uint64_t val, uint64_t mask, uint32_t futex_flags, unsigned flags) {
    io_uring_prep_futex_wait(sqe, futex, val, mask, futex_flags, flags);
}

void swr_uring_prep_futex_wake(struct io_uring_sqe *sqe, uint32_t *futex, uint64_t val, uint64_t mask, uint32_t futex_flags, unsigned flags) {
    io_uring_prep_futex_wake(sqe, futex, val, mask, futex_flags, flags);
}

void swr_uring_prep_waitid(struct io_uring_sqe *sqe, idtype_t idtype, id_t id, siginfo_t *infop, int options, unsigned flags) {
    io_uring_prep_waitid(sqe, idtype, id, infop, options, flags);
}

void swr_uring_prep_poll_add(struct io_uring_sqe *sqe, int fd, unsigned poll_mask) {
    io_uring_prep_poll_add(sqe, fd, poll_mask);
}
