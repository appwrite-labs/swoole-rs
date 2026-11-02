/* Swoole\Runtime (Rust, crates/phpruntime): the class and SWOOLE_HOOK_*
 * constants, the hook flags, PHPCoroutine::enable_hook's family transitions,
 * the function-table hooks and the sleep family's replacements (their
 * zif_swoole_* handlers). C++ lends what it alone reaches: the stream
 * transports and wrappers, the PDO drivers, Swoole's coroutine state,
 * callables and logger, and the handlers that stay C++. Methods and functions
 * that can run PHP code go through C++ trampolines, which re-raise a bailout
 * the swr_v1_phpruntime_* bodies below report. */
#ifndef SWOOLE_RS_PHPRUNTIME_H
#define SWOOLE_RS_PHPRUNTIME_H
#include <stdbool.h>
#include <stddef.h>
#include <stdint.h>
#include <sys/socket.h>
#include <sys/time.h>
#include <sys/types.h>
#ifdef __cplusplus
extern "C" {
#endif
struct _zend_function_entry;
struct _zend_class_entry;
struct _zend_execute_data;
struct _zval_struct;
struct _zend_fcall_info_cache;
typedef void (*swr_v1_phpruntime_handler)(struct _zend_execute_data *execute_data, struct _zval_struct *return_value);
struct swr_v1_phpruntime_ops {
    uint32_t features;
    struct _zend_class_entry *exception;
    uint8_t (*in_coroutine)(void);
    int32_t (*coroutine_sleep)(double seconds);
    void (*set_hook_flags)(uint32_t flags);
    uint32_t (*get_hook_flags)(void);
    uint8_t (*enable_library)(void);
    uint8_t (*main_thread)(void);
    uint32_t (*active_threads)(void);
    void (*call_hook)(uint8_t after, uint32_t *flags);
    int32_t (*transport)(uint32_t index, uint8_t hook);
    void (*stream_ops)(uint8_t stdio, uint8_t hook);
    void (*pdo)(uint32_t flag, uint8_t blocking);
    void *(*callable)(struct _zval_struct *name);
    void (*callable_free)(void *callable);
    struct _zend_fcall_info_cache *(*callable_fcc)(void *callable);
    void (*log_warning)(const char *message);
    void (*fatal_error)(int32_t level, const char *message);
    void (*error)(int32_t level, const char *message);
    swr_v1_phpruntime_handler user_func;
    swr_v1_phpruntime_handler stream_select;
    swr_v1_phpruntime_handler stream_socket_pair;
    swr_v1_phpruntime_handler proc_open;
    swr_v1_phpruntime_handler proc_close;
    swr_v1_phpruntime_handler proc_get_status;
    swr_v1_phpruntime_handler proc_terminate;
    swr_v1_phpruntime_handler gethostbyname;
    swr_v1_phpruntime_handler exit;
    swr_v1_phpruntime_handler putenv;
};
void swr_v1_phpruntime_minit(const struct _zend_function_entry *methods,
                             int32_t module_number,
                             const struct swr_v1_phpruntime_ops *ops);
/* enableCoroutine (min 0) and setHookFlags (min 1) after the CLI check: 1 if a
 * bailout was trapped. */
uint8_t swr_v1_phpruntime_set_hook_flags(struct _zend_execute_data *frame, struct _zval_struct *value, uint32_t min);
/* PHPCoroutine::enable_hook: bit 0 applied, bit 1 a trapped bailout. */
uint8_t swr_v1_phpruntime_enable_hook(uint32_t flags);
void swr_v1_phpruntime_unsafe_functions(uint8_t disable);
/* RINIT: 1 on the main thread (C++ then saves the original transports). */
uint8_t swr_v1_phpruntime_rinit(void);
/* RSHUTDOWN: 1 if a bailout was trapped. */
uint8_t swr_v1_phpruntime_rshutdown(void);
swr_v1_phpruntime_handler swr_v1_phpruntime_original_handler(const char *name, size_t length);
/* swoole_user_func_handler: 1 if the call bailed out. */
uint8_t swr_v1_phpruntime_user_func(struct _zend_execute_data *frame, struct _zval_struct *value);

/* The coroutine socket streams: the "socket/coroutine" operations, the
 * transport factory's coroutine branch and php_swoole_create_stream_from_socket.
 * An operation that can run PHP code sets *bailout to 1 when it trapped a
 * bailout, which the C++ caller re-raises. */
struct _php_stream;
struct _php_stream_statbuf;
struct _php_stream_context;
ssize_t swr_v1_phpruntime_socket_write(struct _php_stream *stream, const char *buf, size_t count, uint8_t *bailout);
ssize_t swr_v1_phpruntime_socket_read(struct _php_stream *stream, char *buf, size_t count, uint8_t *bailout);
int swr_v1_phpruntime_socket_close(struct _php_stream *stream, int close_handle);
int swr_v1_phpruntime_socket_flush(struct _php_stream *stream);
int swr_v1_phpruntime_socket_cast(struct _php_stream *stream, int castas, void **ret);
int swr_v1_phpruntime_socket_stat(struct _php_stream *stream, struct _php_stream_statbuf *ssb);
int swr_v1_phpruntime_socket_set_option(
    struct _php_stream *stream, int option, int value, void *ptrparam, uint8_t *bailout);
struct _php_stream *swr_v1_phpruntime_socket_create(const char *proto,
                                                    size_t protolen,
                                                    const char *resourcename,
                                                    const char *persistent_id,
                                                    struct _php_stream_context *context,
                                                    uint8_t *bailout);
struct _php_stream *swr_v1_phpruntime_stream_from_socket(int fd, int domain, int type, int protocol);
/* stream_socket_pair: 1 if a bailout was trapped. */
uint8_t swr_v1_phpruntime_socket_pair(struct _zend_execute_data *frame, struct _zval_struct *value);
/* stream_select: 1 if a bailout was trapped. */
uint8_t swr_v1_phpruntime_stream_select(struct _zend_execute_data *frame, struct _zval_struct *value);
/* php_async_socket_connect_to_host: the connected coroutine socket's fd, or -1. */
struct _zend_string;
int swr_v1_phpruntime_connect_to_host(const char *host,
                                      unsigned short port,
                                      struct timeval *timeout,
                                      struct _zend_string **error_string,
                                      int *error_code,
                                      const char *bindto,
                                      unsigned short bindport,
                                      long sockopts,
                                      uint8_t *bailout);
/* php_async_socket_poll. */
int swr_v1_phpruntime_socket_poll(int fd, int events, int timeout);

/* Defined by ext-src/swoole_runtime.cc for those streams: the coroutine socket
 * (a SocketImpl *) is still C++, so each operation is one call. */
void *swoole_runtime_socket_create(int type, bool ssl);
void *swoole_runtime_socket_from_fd(int fd, int domain, int type, int protocol);
void swoole_runtime_socket_free(void *socket);
int swoole_runtime_socket_fd(void *socket);
int swoole_runtime_socket_domain(void *socket);
int swoole_runtime_socket_type(void *socket);
bool swoole_runtime_socket_inet(void *socket);
/* errCode, with errMsg in *message when message is not null. */
int swoole_runtime_socket_error(void *socket, const char **message);
/* set_err(code). */
void swoole_runtime_socket_set_error(void *socket, int code);
/* errCode = code. */
void swoole_runtime_socket_set_error_code(void *socket, int code);
/* op: 0 send_sync, 1 recv_sync, 2 send_all, 3 send_once, 4 recv, 5 recv_once,
 * 6 peek, 7 send. */
ssize_t swoole_runtime_socket_io(void *socket, int op, char *buf, size_t length);
/* catch_write_error (write) or catch_read_error of code. */
int swoole_runtime_socket_catch_error(void *socket, bool write, int code);
ssize_t swoole_runtime_socket_recvfrom(
    void *socket, char *buf, size_t length, struct sockaddr *addr, socklen_t *addrlen);
bool swoole_runtime_socket_close(void *socket);
/* setsockopt(level, name, value); logged: the coroutine socket's, which logs
 * a failure. */
void swoole_runtime_socket_option(void *socket, int level, int name, int value, bool logged);
void swoole_runtime_socket_nodelay(void *socket, int value);
void swoole_runtime_socket_zero_copy(void *socket);
void swoole_runtime_socket_timeout(void *socket, double timeout, int type);
bool swoole_runtime_socket_bind(void *socket, const char *host, int port);
bool swoole_runtime_socket_connect(void *socket, const char *host, int port);
bool swoole_runtime_socket_listen(void *socket, int backlog);
void *swoole_runtime_socket_accept(void *socket);
bool swoole_runtime_socket_shutdown(void *socket, int how);
bool swoole_runtime_socket_check_liveness(void *socket);
bool swoole_runtime_socket_ssl_enabled(void *socket);
bool swoole_runtime_socket_ssl_available(void *socket);
/* The connection's SSL *, or null. */
void *swoole_runtime_socket_ssl(void *socket);
void swoole_runtime_socket_ssl_encrypt(void *socket);
bool swoole_runtime_socket_ssl_handshake(void *socket);
void swoole_runtime_socket_ssl_close(void *socket);
/* each(arg, pem, length) of the peer certificate, or of its chain. */
void swoole_runtime_socket_peer_certs(void *socket, bool chain, void (*each)(void *, const char *, size_t), void *arg);
/* php_swoole_socket_set_ssl inside zend_try: -1 if it bailed out. */
int swoole_runtime_socket_set_ssl(void *socket, struct _zval_struct *settings);
/* openssl_x509_read(pem) into retval inside zend_try: 1 if it bailed out. */
uint8_t swoole_runtime_x509_read(const char *pem, size_t length, struct _zval_struct *retval);
/* Inside zend_try, 1 if it bailed out: kind 0 php_error_docref(NULL, ..),
 * 1 php_swoole_error, 2 php_swoole_fatal_error, of level and "%s" message. */
uint8_t swoole_runtime_error(int kind, int level, const char *message);
void swoole_runtime_check_reactor(void);
/* The coroutine socket of a hooked fd, or null; *holder keeps it alive until
 * swoole_runtime_fd_socket_release. */
void *swoole_runtime_fd_socket(int fd, void **holder);
void swoole_runtime_fd_socket_release(void *holder);
/* poll(SW_EVENT_WRITE or SW_EVENT_READ, timeout). */
bool swoole_runtime_socket_poll(void *socket, bool write, double timeout);
const char *swoole_runtime_strerror(int code);
/* Coroutine::get_current_safe() inside zend_try: 1 if it bailed out. */
uint8_t swoole_runtime_coroutine_check(void);
/* php_swoole_convert_to_fd(value) into *fd inside zend_try: 1 if it bailed out. */
uint8_t swoole_runtime_convert_to_fd(struct _zval_struct *value, int *fd);
/* System::socket_poll of count (fd, events) rows, merged by fd as stream_select
 * merges them; the polled fds and their revents in the poll set's order. */
bool swoole_runtime_poll(
    const int *fds, const int *events, size_t count, double timeout, int *ready, int *revents, size_t *size);
#ifdef __cplusplus
}
#endif
#endif
