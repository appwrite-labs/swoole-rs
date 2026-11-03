#ifndef SWOOLE_RS_PHPCLIENTS_H
#define SWOOLE_RS_PHPCLIENTS_H

#include <stdbool.h>
#include <stddef.h>
#include <stdint.h>
#include <sys/types.h>
#include <sys/uio.h>

#ifdef __cplusplus
extern "C" {
#endif

/* PHP Coroutine\Socket (crates/phpclients). Rust registers the class with its
 * method table, its exception,
 * properties and constants, and the methods are its zim_swoole_socket_coro_*
 * exports. */
struct _zend_class_entry;
struct _zend_object;
struct _zval_struct;
struct _zend_string;
void swr_v1_phpclients_socket_minit(struct _zend_class_entry *exception,
                                    bool shortname,
                                    bool sockets,
                                    int module,
                                    uint32_t ai,
                                    struct _zend_class_entry **socket_class);

/* Defined by ext-src/swoole_socket_coro.cc for those methods: the coroutine
 * socket (a SocketImpl *) is still C++, so each operation is one call. */
void swoole_rs_php_error(int level, bool fatal, const char *message);
void swoole_socket_coro_unconstructed(void);
void *swoole_socket_coro_create(int64_t domain, int64_t type, int64_t protocol, int *error);
/* socket_coro_init's socket half: zero copy and the zend string allocator, then
 * its fd, domain, type and protocol into fields. */
void swoole_socket_coro_adopt(void *socket, int64_t *fields);
/* A Coroutine\Socket object over socket, as socket_coro_init leaves it;
 * reference: the object does not own the socket. */
struct _zend_object *swr_v1_phpclients_socket_object(void *socket, bool reference);
void swoole_socket_coro_release(struct _zend_object *object);
void swoole_socket_coro_close(struct _zend_object *object);
bool swoole_socket_coro_is_closed(void *socket);
int swoole_socket_coro_error(void *socket, const char **message);
void swoole_socket_coro_set_error(void *socket, int code, const char *message);
bool swoole_socket_coro_bind(void *socket, const char *address, size_t length, int64_t port);
bool swoole_socket_coro_listen(void *socket, int64_t backlog);
void *swoole_socket_coro_accept(void *socket, double timeout);
bool swoole_socket_coro_port_required(void *socket);
bool swoole_socket_coro_connect(void *socket, const char *host, size_t length, int64_t port, double timeout);
bool swoole_socket_coro_check_liveness(void *socket);
int64_t swoole_socket_coro_bound_cid(void *socket, int64_t event);
/* op: 0 peek, 1 recv, 2 recv_all, 3 recv_line, 4 recv_with_buffer, 5 send, 6 send_all. */
ssize_t swoole_socket_coro_transfer(void *socket, int op, char *buffer, size_t length, double timeout);
ssize_t swoole_socket_coro_recv_packet(void *socket, double timeout, bool clear_empty);
char *swoole_socket_coro_pop_packet(void *socket);
/* op: 0 readv, 1 readv_all, 2 writev, 3 writev_all. */
ssize_t swoole_socket_coro_vector(
    void *socket, int op, struct iovec *vectors, int count, double timeout, int *index, size_t *offset);
bool swoole_socket_coro_sendfile(void *socket, const char *file, int64_t offset, int64_t length);
ssize_t swoole_socket_coro_recvfrom(void *socket, char *buffer, size_t length, double timeout);
const char *swoole_socket_coro_address(void *socket, int *port);
ssize_t swoole_socket_coro_sendto(
    void *socket, const char *address, size_t length, int64_t port, const char *data, size_t data_length);
bool swoole_socket_coro_shutdown(void *socket, int64_t how);
bool swoole_socket_coro_getsockname(void *socket);
bool swoole_socket_coro_getpeername(void *socket, char *address, size_t size, int *port);
bool swoole_socket_coro_cancel(void *socket, int event);
bool swoole_socket_coro_ssl_handshake(void *socket);
/* The settings arrays (php_swoole_socket_set, _set_protocol and _set_ssl), for
 * C++ callers: whether they all applied. A deferred bailout is raised last. */
enum { SWR_PHPCLIENTS_SETTINGS_ALL = 0, SWR_PHPCLIENTS_SETTINGS_PROTOCOL = 1, SWR_PHPCLIENTS_SETTINGS_SSL = 2 };
bool swr_v1_phpclients_socket_settings(void *socket, const struct _zval_struct *settings, int which);
/* What the options and settings need from the C++ socket. flag: 0 open_length_check,
 * 1 open_eof_check, 2 http2. preset: 0 mqtt::set_protocol, 1 the default length function. */
void swoole_socket_coro_set_flag(void *socket, int flag, bool value);
void *swoole_socket_coro_protocol(void *socket);
void swoole_socket_coro_protocol_preset(void *socket, int kind);
/* package_length_func, run inside zend_try: a callable replaces any earlier one. */
typedef struct {
    void *socket;
    struct _zval_struct *value;
} swoole_socket_coro_callable;
void swoole_socket_coro_length_callable(void *call);
bool swoole_socket_coro_set_option(void *socket, int level, int name, int value);
void swoole_socket_coro_set_timeout(void *socket, double timeout, int type);
double swoole_socket_coro_get_timeout(void *socket, int type);
void swoole_socket_coro_proxy(void *socket, bool http, const char *host, size_t host_length, int64_t port,
                              const char *user, size_t user_length, const char *password, size_t password_length);
/* SSL: ENABLE is enable_ssl_encrypt(), ENABLED ssl_is_enable(), the file options
 * report whether the file exists, HAS_* whether one is set; the rest set text or
 * value and return true, or false when the build lacks them (ssl_supports). */
enum {
    SWR_PHPCLIENTS_SSL_ENABLE,
    SWR_PHPCLIENTS_SSL_ENABLED,
    SWR_PHPCLIENTS_SSL_PROTOCOLS,
    SWR_PHPCLIENTS_SSL_DISABLE_COMPRESS,
    SWR_PHPCLIENTS_SSL_CERT_FILE,
    SWR_PHPCLIENTS_SSL_KEY_FILE,
    SWR_PHPCLIENTS_SSL_HAS_CERT_FILE,
    SWR_PHPCLIENTS_SSL_HAS_KEY_FILE,
    SWR_PHPCLIENTS_SSL_PASSPHRASE,
    SWR_PHPCLIENTS_SSL_HOST_NAME,
    SWR_PHPCLIENTS_SSL_VERIFY_PEER,
    SWR_PHPCLIENTS_SSL_ALLOW_SELF_SIGNED,
    SWR_PHPCLIENTS_SSL_CAFILE,
    SWR_PHPCLIENTS_SSL_CAPATH,
    SWR_PHPCLIENTS_SSL_VERIFY_DEPTH,
    SWR_PHPCLIENTS_SSL_CIPHERS,
    SWR_PHPCLIENTS_SSL_ECDH_CURVE,
    SWR_PHPCLIENTS_SSL_GREASE,
};
bool swoole_socket_coro_ssl_option(void *socket, int option, const char *text, size_t length, int64_t value);
bool swoole_socket_coro_ssl_supports(int option);
/* ext/sockets' helpers, run inside zend_try. op: 0 getsockopt_ipv6_rfc3542,
 * 1 setsockopt_ip_mcast, 2 setsockopt_ipv6_mcast, 3 setsockopt_ipv6_rfc3542,
 * 4 add4_to_if_index(address) into index. */
typedef struct {
    void *socket;
    int op, level, name;
    struct _zval_struct *value;
    const void *address;
    unsigned index;
    int result;
} swoole_socket_coro_sockopt_call;
void swoole_socket_coro_sockopt(void *call);
/* getsockopt(SOL_TCP, TCP_INFO) into array: 1; 0 when name is not TCP_INFO (or
 * the build has none); -1 when it failed. */
int swoole_socket_coro_tcp_info(void *socket, int name, struct _zval_struct *array);
/* php_swoole_create_socket_from_fd(fd, type), run inside zend_try; with a
 * stream (import), the object holds it and nonblock is set. Null when none
 * could be made. */
typedef struct {
    int fd, type, nonblock;
    struct _zval_struct *stream;
    struct _zend_object *object;
} swoole_socket_coro_import_call;
void swoole_socket_coro_import(void *call);

/* PHP Coroutine\Client (crates/phpclients): MINIT and, from
 * ext-src/swoole_client_coro.cc, what its methods need beyond the socket calls. */
void swr_v1_phpclients_client_minit(bool shortname,
                                    struct _zend_class_entry **client_class);
void swoole_rs_check_reactor(void);
void swoole_rs_set_last_error(int code);
void *swoole_client_coro_create_socket(struct _zend_object *object, int64_t type);
void swoole_client_coro_merge_setting(struct _zend_object *object, const struct _zval_struct *settings);
struct _zval_struct *swoole_client_coro_setting(struct _zend_object *object);
int64_t swoole_client_coro_type_property(struct _zend_object *object);
bool swoole_client_coro_connect(void *socket, const char *host, int64_t port, double timeout, int64_t flags);
void swoole_client_coro_rdwr_timeout(void *socket, double timeout);
bool swoole_client_coro_close(void *socket);
/* The coroutine socket's network::Socket record (get_socket()), or null. */
void *swoole_client_coro_socket_record(void *socket);
int swoole_client_coro_type(void *socket);
bool swoole_client_coro_packet_mode(void *socket);
bool swoole_client_coro_is_connected(void *socket);
bool swoole_client_coro_has_ssl(void *socket);
struct _zend_string *swoole_client_coro_peer_cert(void *socket);
bool swoole_client_coro_verify_peer(void *socket, bool allow_self_signed);

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
void *swoole_http_client_coro_resolve_new(void);
void swoole_http_client_coro_resolve_reset(void *context, bool with_port);
void swoole_http_client_coro_resolve_free(void *context);
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

/* PHP Swoole\Client and Swoole\Async\Client (crates/phpclients): MINIT, and
 * the async events network::Client raises, which re-raise a PHP callback's
 * bailout as their last step. For SWOOLE_CLIENT_ON_ERROR, `length` is
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
void swr_v1_phpclients_async_minit(struct _zend_class_entry *client_class,
                                   struct _zend_class_entry **async_class);
void swr_v1_phpclients_async_event(struct _zend_object *object, int kind, const char *data, size_t length);

/* Defined by ext-src/swoole_client.cc: network::Client, one call per operation. */
enum { SWOOLE_CLIENT_ERROR = 0, SWOOLE_CLIENT_ERROR_FATAL = 1, SWOOLE_CLIENT_ERROR_SYS = 2, SWOOLE_CLIENT_ERROR_CORE = 3 };
/* php_swoole_error, _fatal_error, _sys_error or _core_error(level, "%s", message). */
void swoole_client_php_error(int kind, int level, const char *message);
void swoole_client_package_too_big(int length);
int swoole_client_last_error(void);
/* new Client(type, async); null, with errno in `error`, when it has no socket. */
void *swoole_client_create(int type, bool async, int *error);
int swoole_client_fd(void *cli);
/* The client's Rust state (crates/clients' ClientState). */
void *swoole_client_state(void *cli);
bool swoole_client_has_socket(void *cli);
int swoole_client_socket_type(void *cli);
bool swoole_client_has_ssl(void *cli);
bool swoole_client_check_liveness(void *cli);
/* getsockopt(SO_ERROR): whether it succeeded. */
bool swoole_client_socket_error(void *cli, int *error);
/* socket->catch_error(error): SW_CLOSE and friends. */
int swoole_client_catch_error(void *cli, int error);
struct swoole_client_framing {
    bool eof_check;
    bool length_check;
    uint8_t eof_length;
    char eof[8];
    uint32_t max_length;
    uint16_t length_offset;
    uint8_t length_size;
};
void swoole_client_framing(void *cli, struct swoole_client_framing *framing);
/* getsockname: the address, with the port in `port`; null on failure (errno). */
const char *swoole_client_sockname(void *cli, int *port);
bool swoole_client_peername(void *cli, char *address, size_t size, int *port);
struct _zend_string *swoole_client_peer_cert(void *cli);
/* php_swoole_convert_to_socket(fd) into `zsocket`; false without ext/sockets. */
bool swoole_client_export_socket(void *cli, struct _zval_struct *zsocket);
void swoole_client_merge_setting(struct _zend_object *object, const struct _zval_struct *settings);
/* Install the async callbacks in `events` (1 << SWOOLE_CLIENT_ON_*), raising them on `object`. */
void swoole_client_async_listen(void *cli, struct _zend_object *object, uint32_t events);
bool swoole_client_has_on_error(void *cli);
void swoole_client_ssl_wait_stream(void *cli);

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
    SWOOLE_CLIENT_SETTINGS = 10,   /* settings: php_swoole_client_check_setting */
    SWOOLE_CLIENT_ENABLE_SSL = 11, /* settings (or null): the SSL options, then enable_ssl_encrypt */
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

#ifdef __cplusplus
}
#endif
#endif
