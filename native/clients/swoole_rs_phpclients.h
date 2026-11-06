#ifndef SWOOLE_RS_PHPCLIENTS_H
#define SWOOLE_RS_PHPCLIENTS_H

#include <stdbool.h>
#include <stddef.h>
#include <stdint.h>
#include <sys/types.h>

#ifdef __cplusplus
extern "C" {
#endif

struct _zend_class_entry;
struct _zend_object;
struct _zval_struct;
struct _zend_string;
/* php_swoole_error(level, "%s", message), or with fatal php_swoole_fatal_error
 * (crates/phpclients). */
void swoole_rs_php_error(int level, bool fatal, const char *message);

/* PHP Coroutine\Http\Client (crates/phpclients): MINIT, the methods that can
 * call PHP (the C++ trampolines re-raise a bailout they return), the socket's
 * close hook and the Set-Cookie parser the HTTP/2 client shares. */
struct _zend_execute_data;
void swr_v1_phpclients_http_client_minit(struct _zend_class_entry *exception,
                                         bool shortname,
                                         int module,
                                         uint32_t features);
bool swr_v1_phpclients_http_client_execute(struct _zend_execute_data *execute_data, struct _zval_struct *return_value);
bool swr_v1_phpclients_http_client_get(struct _zend_execute_data *execute_data, struct _zval_struct *return_value);
bool swr_v1_phpclients_http_client_post(struct _zend_execute_data *execute_data, struct _zval_struct *return_value);
bool swr_v1_phpclients_http_client_download(struct _zend_execute_data *execute_data, struct _zval_struct *return_value);
bool swr_v1_phpclients_http_client_upgrade(struct _zend_execute_data *execute_data, struct _zval_struct *return_value);
bool swr_v1_phpclients_http_client_recv(struct _zend_execute_data *execute_data, struct _zval_struct *return_value);
void swr_v1_phpclients_http_client_socket_dtor(void *conn, void *written);
void swr_v1_phpclients_parse_set_cookies(const char *at,
                                         size_t length,
                                         struct _zval_struct *cookies,
                                         struct _zval_struct *set_cookie_headers);

/* Defined by ext-src/swoole_http_client_coro.cc for those methods. */
struct _zend_object *swoole_http_client_coro_create_socket(int64_t type, int *error);
bool swoole_http_client_coro_enable_ssl(void *socket);
/* After the settings: an HTTP proxy without SSL skips its CONNECT handshake. */
void swoole_http_client_coro_apply(void *socket);
void swoole_http_client_coro_resolve_reset(void *context, bool with_port);
void swoole_http_client_coro_configure(void *socket, double timeout, void *resolve, void *conn);
bool swoole_http_client_coro_connect(void *socket, const char *host, size_t length, int port);
bool swoole_http_client_coro_close(void *socket);
void swoole_http_client_coro_check_bound(void *socket, int event);
void swoole_http_client_coro_set_err(void *socket, int code);
int swoole_http_client_coro_last_error(void);
int swoole_http_client_coro_proxy(void *socket);
struct _zend_string *swoole_http_client_coro_proxy_auth(void *socket);
bool swoole_http_client_coro_send(void *socket, const char *data, size_t length);
bool swoole_http_client_coro_send_raw(void *socket, const char *data, size_t length);
const char *swoole_http_client_coro_written(void *socket, void *saved, size_t *length);
void swoole_http_client_coro_buffer_free(void *buffer);
char *swoole_http_client_coro_read_buffer(void *socket, size_t *length, size_t *size);
void swoole_http_client_coro_read_buffer_set(void *socket, size_t length);
void swoole_http_client_coro_read_buffer_keep(void *socket, size_t length, size_t offset);
ssize_t swoole_http_client_coro_recv(void *socket, char *data, size_t length);
void *swoole_http_client_coro_timeout(void *socket, double timeout);
bool swoole_http_client_coro_timed_out(void *controller);
void swoole_http_client_coro_timeout_end(void *controller);
void *swoole_http_client_coro_ws_new(void);
void swoole_http_client_coro_ws_free(void *ws);
void swoole_http_client_coro_ws_apply(void *ws, const struct _zval_struct *settings);
bool swoole_http_client_coro_ws_compression(void *ws);
int swoole_http_client_coro_ws_push(
    void *socket, void *ws, struct _zval_struct *data, int64_t opcode, int flags, int64_t code, bool compress);
void swoole_http_client_coro_ws_recv(void *socket, void *ws, struct _zval_struct *return_value, double timeout);
void swoole_http_client_coro_ws_protocol(void *socket);
void *swoole_http_client_coro_file_open(const char *name, int64_t offset);
bool swoole_http_client_coro_file_write(void *file, const char *data, size_t length);
void swoole_http_client_coro_file_free(void *file);
void *swoole_http_client_coro_callable(struct _zval_struct *fn);
void swoole_http_client_coro_callable_free(void *callable);
int swoole_http_client_coro_call(void *callable, struct _zend_object *client, const char *data, size_t length);
struct _zend_string *swoole_http_client_coro_peer_cert(void *socket);
const char *swoole_http_client_coro_mime(const char *path);
void swoole_http_client_coro_sys_error(const char *message);
void swoole_http_client_coro_inflate_warning(int method);

/* PHP Swoole\Client and Swoole\Async\Client (crates/phpclients): MINIT
 * (php_swoole_client_async_minit is Rust's), and the async events
 * network::Client raises, which re-raise a PHP callback's bailout as their
 * last step. For SWOOLE_CLIENT_ON_ERROR, `length` is
 * swoole_get_last_error(). */
enum {
    SWOOLE_CLIENT_ON_CONNECT = 1,
    SWOOLE_CLIENT_ON_RECEIVE = 2,
    SWOOLE_CLIENT_ON_CLOSE = 3,
    SWOOLE_CLIENT_ON_ERROR = 4,
    SWOOLE_CLIENT_ON_BUFFER_FULL = 5,
    SWOOLE_CLIENT_ON_BUFFER_EMPTY = 6,
};
void swr_v1_phpclients_net_minit(struct _zend_class_entry *exception,
                                 bool sockets,
                                 int module,
                                 struct _zend_class_entry **client_class);
void swr_v1_phpclients_async_event(struct _zend_object *object, int kind, const char *data, size_t length);

/* Defined by ext-src/swoole_client.cc: network::Client, one call per operation. */
enum { SWOOLE_CLIENT_ERROR = 0, SWOOLE_CLIENT_ERROR_FATAL = 1, SWOOLE_CLIENT_ERROR_SYS = 2, SWOOLE_CLIENT_ERROR_CORE = 3 };
/* php_swoole_error, _fatal_error, _sys_error or _core_error(level, "%s", message). */
void swoole_client_php_error(int kind, int level, const char *message);
/* new Client(type, async); null, with errno in `error`, when it has no socket. */
void *swoole_client_create(int type, bool async, int *error);
/* The client's Rust state (crates/clients' ClientState). */
void *swoole_client_state(void *cli);
/* php_swoole_convert_to_socket(fd) into `zsocket`; false without ext/sockets. */
bool swoole_client_export_socket(void *cli, struct _zval_struct *zsocket);
/* Install the async callbacks in `events` (1 << SWOOLE_CLIENT_ON_*), raising them on `object`. */
void swoole_client_async_listen(void *cli, struct _zend_object *object, uint32_t events);
bool swoole_client_has_on_error(void *cli);

/* The client's SSLContext owner, which crates/tls configures; NULL while TLS is off. */
const struct swr_v1_tls_owner *swoole_client_ssl_context(void *cli);
/* set_http_proxy (http) or set_socks5_proxy. */
void swoole_client_proxy(void *cli,
                         bool http,
                         const char *host,
                         size_t host_length,
                         int64_t port,
                         const char *user,
                         size_t user_length,
                         const char *password,
                         size_t password_length);

/* One operation that can run PHP code, run by swoole_client_run as a
 * bailout::trap body. */
enum {
    SWOOLE_CLIENT_CONNECT = 0,     /* data: host; a: port; timeout; b: sock_flag */
    SWOOLE_CLIENT_SEND = 1,        /* data, length; a: flags */
    SWOOLE_CLIENT_SENDTO = 2,      /* data, length: host; a: port; buffer, b: data */
    SWOOLE_CLIENT_SENDFILE = 3,    /* data: file; a: offset; b: length */
    SWOOLE_CLIENT_RECV = 4,        /* buffer, length; a: flags */
    SWOOLE_CLIENT_PACKAGE_LENGTH = 5, /* buffer, length: get_package_length */
    SWOOLE_CLIENT_SHUTDOWN = 6,    /* a: how */
    SWOOLE_CLIENT_CLOSE = 7,
    SWOOLE_CLIENT_SLEEP = 8,
    SWOOLE_CLIENT_WAKEUP = 9,
    SWOOLE_CLIENT_ENABLE_SSL = 11, /* enable_ssl_encrypt */
    SWOOLE_CLIENT_SSL_HANDSHAKE = 12,
    SWOOLE_CLIENT_SSL_VERIFY = 13, /* a: allow_self_signed */
    SWOOLE_CLIENT_SSL_WAIT = 14,   /* swoole_event_set(socket, SW_EVENT_WRITE) */
    SWOOLE_CLIENT_DESTROY = 15,    /* delete the timer, the length callable and the client */
    SWOOLE_CLIENT_DESTRUCT = 16,   /* cli: the object; $this->close() as Swoole\Client::close */
    SWOOLE_CLIENT_CALLABLE = 17,   /* settings: sw_callable_create, into buffer */
    SWOOLE_CLIENT_CALLABLE_FREE = 18, /* cli: the callable */
    SWOOLE_CLIENT_INVOKE = 19,     /* cli: the callable; settings, a: argv, argc; result: success */
    SWOOLE_CLIENT_CONVERT_FD = 20, /* settings: php_swoole_convert_to_fd */
};
struct swoole_client_call {
    void *cli;
    int op;
    const char *data;
    size_t length;
    char *buffer;
    int64_t a;
    int64_t b;
    double timeout;
    const struct _zval_struct *settings;
    int64_t result;
};
void swoole_client_run(void *call);

/* Swoole\NameResolver\Context and the swoole_name_resolver_* functions
 * (crates/phpclients/src/resolver.rs). */
void swr_v1_phpclients_name_resolver_minit(void);
/* The PHP resolver's lookup of name; writes a non-empty result through assign
 * (out is a std::string), then raises a bailout its PHP code caused. */
void swr_v1_phpclients_name_resolver_lookup(struct _zval_struct *resolver,
                                            const char *name,
                                            size_t length,
                                            void *context,
                                            void *out,
                                            void (*assign)(void *out, const char *text, size_t length));
/* Defined by ext-src/swoole_name_resolver.cc. context is a NameResolver::Context *. */
void swoole_name_resolver_php_add(struct _zval_struct *resolver);
struct _zval_struct *swoole_name_resolver_php_remove(bool (*same)(struct _zval_struct *kept,
                                                                  struct _zval_struct *resolver),
                                                     struct _zval_struct *resolver);
struct swoole_name_resolver_lookup_call {
    const char *name;
    size_t length;
    void *context;
    void *out;
    void (*assign)(void *out, const char *text, size_t length);
};
/* swoole_name_resolver_lookup over a swoole_name_resolver_lookup_call; a
 * bailout::trap body. */
void swoole_name_resolver_lookup_run(void *call);
void *swoole_name_resolver_context_new(void);
void swoole_name_resolver_context_free(void *context);
void swoole_name_resolver_context_set(void *context, int64_t type, bool with_port);
struct _zval_struct *swoole_name_resolver_context_get(void *context, bool *with_port);
void swoole_name_resolver_context_settle(void *context, struct _zval_struct *cluster, bool is_final);
void swoole_name_resolver_context_release(void *context);

#ifdef __cplusplus
}
#endif
#endif
