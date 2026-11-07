/* Swoole\Runtime (Rust, crates/phpruntime): the class and SWOOLE_HOOK_*
 * constants, the hook flags, PHPCoroutine::enable_hook's family transitions,
 * the function-table hooks and the sleep family's replacements (their
 * zif_swoole_* handlers), its methods and the hooked stream functions; each
 * raises a trapped bailout last. C++ lends what it alone reaches: the stream
 * wrappers, the PDO drivers, Swoole's coroutine state, callables and logger,
 * and the handlers that stay C++. */
#ifndef SWOOLE_RS_PHPRUNTIME_H
#define SWOOLE_RS_PHPRUNTIME_H
#include <stdbool.h>
#include <stddef.h>
#include <stdint.h>
#include <sys/socket.h>
#include <sys/time.h>
#include <sys/types.h>
#include <sys/uio.h>
#ifdef __cplusplus
extern "C" {
#endif
struct _zend_class_entry;
struct _zend_execute_data;
struct _zval_struct;
struct _zend_fcall_info_cache;
struct swr_v1_runtime_callable_call {
    void *callable;
    uint32_t argc;
    struct _zval_struct *argv;
    bool result;
};
typedef void (*swr_v1_phpruntime_handler)(struct _zend_execute_data *execute_data, struct _zval_struct *return_value);
struct swr_v1_phpruntime_ops {
    uint32_t features;
    struct _zend_class_entry *exception;
    uint8_t (*in_coroutine)(void);
    int32_t (*coroutine_sleep)(double seconds);
    void (*set_hook_flags)(uint32_t flags);
    uint32_t (*get_hook_flags)(void);
    uint8_t (*main_thread)(void);
    uint32_t (*active_threads)(void);
    void (*stream_ops)(uint8_t stdio, uint8_t hook);
    void (*pdo)(uint32_t flag, uint8_t blocking);
    void *(*callable)(struct _zval_struct *name);
    void (*callable_free)(void *callable);
    void *(*callable_dup)(void *callable);
    /* call->result = callable->call(argc, argv, nullptr); a bailout goes on. */
    void (*callable_call)(void *call);
    struct _zend_fcall_info_cache *(*callable_fcc)(void *callable);
    void (*fatal_error)(int32_t level, const char *message);
    void (*error)(int32_t level, const char *message);
    swr_v1_phpruntime_handler proc_open;
    swr_v1_phpruntime_handler proc_close;
    swr_v1_phpruntime_handler proc_get_status;
    swr_v1_phpruntime_handler proc_terminate;
    swr_v1_phpruntime_handler gethostbyname;
    swr_v1_phpruntime_handler exit;
    swr_v1_phpruntime_handler putenv;
    /* Bodies for swr_zend_try: openssl_x509_read, Coroutine::get_current_safe()
     * and php_swoole_convert_to_fd. */
    void (*x509_read)(void *call);    /* struct swr_v1_phpruntime_x509_call */
    void (*coroutine_check)(void *unused);
    void (*convert_to_fd)(void *call); /* struct swr_v1_phpruntime_fd_call */
    void (*check_reactor)(void);
};
struct swr_v1_phpruntime_x509_call {
    const char *pem;
    size_t length;
    struct _zval_struct *retval;
};
struct swr_v1_phpruntime_fd_call {
    struct _zval_struct *value;
    int fd;
};
void swr_v1_phpruntime_minit(int32_t module_number,
                             const struct swr_v1_phpruntime_ops *ops);
/* RINIT: 1 on the main thread (the plain wrapper then saves the original stream ops). */
uint8_t swr_v1_phpruntime_rinit(void);
/* PHPCoroutine::enable_hook(flags), 0 disabling every hook; a trapped bailout is raised last. */
bool swr_v1_phpruntime_enable_hook(uint32_t flags);
/* The hook flags that select C++'s PDO drivers and native curl (crates/phpruntime's flags). */
enum {
    SWR_HOOK_NATIVE_CURL = 1u << 12,
    SWR_HOOK_PDO_PGSQL = 1u << 16,
    SWR_HOOK_PDO_ODBC = 1u << 17,
    SWR_HOOK_PDO_ORACLE = 1u << 18,
    SWR_HOOK_PDO_SQLITE = 1u << 19,
    SWR_HOOK_PDO_FIREBIRD = 1u << 20,
};

/* The coroutine socket streams: the "socket/coroutine" operations and
 * php_swoole_create_stream_from_socket. */
struct _php_stream;
struct _php_stream *swr_v1_phpruntime_stream_from_socket(int fd, int domain, int kind, int protocol);

/* Defined by thirdparty/php/streams/plain_wrapper.c, which keeps PHP's originals:
 * RINIT, and the stream_ops entry. */
void php_swoole_runtime_rinit(void);
void php_swoole_runtime_stream_ops(uint8_t stdio, uint8_t hook);

/* What the coroutine streams and client classes need of the native coroutine socket (a SocketImpl *;
 * its virtual calls reach UringSocket), defined by src/coroutine/system.cc. make
 * and close are bodies for swr_zend_try over a swr_v1_runtime_socket_call. */
struct swr_v1_co_socket;
struct swr_v1_tls_owner;
struct swr_v1_core_proxy;
struct swr_v1_runtime_socket_call {
    void *socket; /* make's result, close's socket */
    int fd, domain, type, protocol; /* make: without fd when fd < 0 */
    bool family; /* make: by domain, type and protocol, else by swSocketType type */
    bool result; /* close's */
};
struct swr_v1_runtime_socket_ops {
    void (*make)(void *call);
    void (*close)(void *call);
    void (*free)(void *socket);
    struct swr_v1_co_socket *(*base)(void *socket);
    /* op: 0 send_sync, 1 recv_sync, 2 send_all, 3 send_once, 4 recv, 5 recv_once,
     * 6 peek, 7 send, 8 recv_all. */
    ssize_t (*io)(void *socket, int op, char *buf, size_t length);
    ssize_t (*recvfrom)(void *socket, char *buf, size_t length, struct sockaddr *addr, socklen_t *addrlen);
    /* setsockopt(level, name, value); logged: the coroutine socket's, which logs a failure. */
    void (*option)(void *socket, int level, int name, int value, bool logged);
    /* bind(std::string(host, length), port). */
    bool (*bind)(void *socket, const char *host, size_t length, int port);
    /* connect(std::string(host, length), port, flags). */
    bool (*connect)(void *socket, const char *host, size_t length, int port, int flags);
    bool (*listen)(void *socket, int backlog);
    void *(*accept)(void *socket, double timeout);
    bool (*ssl_enabled)(void *socket);
    void (*ssl_encrypt)(void *socket);
    bool (*ssl_handshake)(void *socket);
    void (*ssl_close)(void *socket);
    /* each(arg, pem, length) of the peer certificate, or of its chain. */
    void (*peer_certs)(void *socket, bool chain, void (*each)(void *, const char *, size_t), void *arg);
    /* poll(SW_EVENT_WRITE or SW_EVENT_READ, timeout). */
    bool (*poll)(void *socket, bool write, double timeout);
    /* System::socket_poll of count (fd, events) rows, merged by fd as stream_select
     * merges them; the polled fds and their revents in the poll set's order. */
    bool (*poll_set)(const int *fds, const int *events, size_t count, double timeout, int *ready, int *revents,
                     size_t *size);
    /* set_dtor: hook(arg, socket) runs once as the socket is destroyed. */
    void (*close_hook)(void *socket, void (*hook)(void *arg, void *socket), void *arg);
    bool (*ssl_verify)(void *socket, bool allow_self_signed);
    /* op: 0 readv, 1 readv_all, 2 writev, 3 writev_all; where it stopped into index and offset. */
    ssize_t (*vector)(void *socket, int op, const struct iovec *vectors, int count, int *index, size_t *offset);
    bool (*sendfile)(void *socket, const char *file, int64_t offset, int64_t length);
    ssize_t (*sendto)(void *socket, const char *host, size_t length, int port, const char *data, size_t size);
    /* set_http_proxy (http) or set_socks5_proxy. */
    void (*proxy)(void *socket, bool http, const char *host, size_t host_length, int port, const char *user,
                  size_t user_length, const char *password, size_t password_length);
    /* The SSLContext's owner, which crates/tls configures; NULL while TLS is off. */
    const struct swr_v1_tls_owner *(*ssl_context)(void *socket);
    /* The HTTP proxy's owner (crates/corebase's ProxyState); NULL without one. */
    struct swr_v1_core_proxy *(*http_proxy)(void *socket);
};
extern const struct swr_v1_runtime_socket_ops swr_v1_runtime_socket;
#ifdef __cplusplus
}
#endif
#endif
