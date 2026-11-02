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

/* PHP Coroutine\Socket (crates/phpclients). MINIT passes upstream's method
 * table, built from the stub's arginfo; Rust registers the class, its exception,
 * properties and constants, and the methods are its zim_swoole_socket_coro_*
 * exports (getOption, setOption and import stay C++). */
struct _zend_function_entry;
struct _zend_class_entry;
struct _zend_object;
struct _zval_struct;
struct _zend_string;
void swr_v1_phpclients_socket_minit(const struct _zend_function_entry *methods,
                                    struct _zend_class_entry *exception,
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
void swoole_socket_coro_init(struct _zend_object *object);
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
bool swoole_socket_coro_set_protocol(void *socket, const struct _zval_struct *settings);

/* PHP Coroutine\Client (crates/phpclients): MINIT and, from
 * ext-src/swoole_client_coro.cc, what its methods need beyond the socket calls. */
void swr_v1_phpclients_client_minit(bool shortname,
                                    struct _zend_class_entry **client_class);
void swoole_rs_check_reactor(void);
void swoole_rs_set_last_error(int code);
void *swoole_client_coro_create_socket(struct _zend_object *object, int64_t type);
bool swoole_client_coro_apply(void *socket, const struct _zval_struct *settings);
void swoole_client_coro_merge_setting(struct _zend_object *object, const struct _zval_struct *settings);
struct _zval_struct *swoole_client_coro_setting(struct _zend_object *object);
int64_t swoole_client_coro_type_property(struct _zend_object *object);
bool swoole_client_coro_connect(void *socket, const char *host, int64_t port, double timeout, int64_t flags);
void swoole_client_coro_rdwr_timeout(void *socket, double timeout);
bool swoole_client_coro_close(void *socket);
bool swoole_client_coro_is_tcp(void *socket);
int swoole_client_coro_type(void *socket);
bool swoole_client_coro_packet_mode(void *socket);
bool swoole_client_coro_is_connected(void *socket);
bool swoole_client_coro_has_ssl(void *socket);
int swoole_client_coro_enable_ssl(struct _zend_object *object, void *socket);
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
void swoole_http_client_coro_apply(void *socket, const struct _zval_struct *settings);
void *swoole_http_client_coro_resolve_new(void);
void swoole_http_client_coro_resolve_reset(void *context, bool with_port);
void swoole_http_client_coro_resolve_free(void *context);
void swoole_http_client_coro_configure(void *socket, double timeout, void *resolve, void *conn);
bool swoole_http_client_coro_connect(void *socket, const char *host, size_t length, int port);
bool swoole_http_client_coro_close(void *socket);
void swoole_http_client_coro_close_wait(void *socket);
bool swoole_http_client_coro_closing(void *socket);
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

#ifdef __cplusplus
}
#endif
#endif
