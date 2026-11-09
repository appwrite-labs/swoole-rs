/* Coroutine socket hooks for thirdparty/php84/ftp/ftp.c: the libc calls it makes
 * become the hooked swoole_coroutine_* functions (crates/hooks/src/hooked.rs).
 * thirdparty/hiredis/net.c, which only the C++ gates' CMake build compiles, also
 * sets its socket's timeouts. */
#ifndef SW_SOCKET_HOOK_H_
#define SW_SOCKET_HOOK_H_
#include <poll.h>
#include <sys/socket.h>
#include <unistd.h>

int swoole_coroutine_socket(int domain, int type, int protocol);
ssize_t swoole_coroutine_send(int sockfd, const void *buf, size_t len, int flags);
ssize_t swoole_coroutine_recv(int sockfd, void *buf, size_t len, int flags);
int swoole_coroutine_accept(int sockfd, struct sockaddr *addr, socklen_t *addrlen);
int swoole_coroutine_poll(struct pollfd *fds, nfds_t nfds, int timeout);
int swoole_coroutine_close(int fd);
int swoole_coroutine_socket_set_timeout(int fd, int which, double timeout);
int swoole_coroutine_socket_set_connect_timeout(int fd, double timeout);

#define socket(domain, type, protocol) swoole_coroutine_socket(domain, type, protocol)
#define send(sockfd, buf, len, flags) swoole_coroutine_send(sockfd, buf, len, flags)
#define recv(sockfd, buf, len, flags) swoole_coroutine_recv(sockfd, buf, len, flags)
#define accept(sockfd, addr, addrlen) swoole_coroutine_accept(sockfd, addr, addrlen)
#define poll(fds, nfds, timeout) swoole_coroutine_poll(fds, nfds, timeout)
#define close(fd) swoole_coroutine_close(fd)
#endif
