#ifndef SWOOLE_RS_PHPHTTP2_H
#define SWOOLE_RS_PHPHTTP2_H

#include <stdbool.h>
#include <stddef.h>
#include <stdint.h>
#include <sys/types.h>

#ifdef __cplusplus
extern "C" {
#endif

/* PHP Coroutine\Http2\Client, Http2\Request and Http2\Response (crates/phphttp2).
 * MINIT passes upstream's method table; Rust registers the classes, their
 * properties and the HTTP/2 constants, and the methods are its
 * zim_swoole_http2_client_coro_* exports. */
struct _zend_function_entry;
struct _zend_class_entry;
struct _zend_object;
struct _zval_struct;
struct _zend_string;
void swr_v1_phphttp2_client_minit(const struct _zend_function_entry *methods,
                                  struct _zend_class_entry *exception,
                                  bool shortname,
                                  int module);
/* The socket's close hook. */
void swr_v1_phphttp2_client_socket_dtor(void *conn);

/* A header for the HPACK helpers: nghttp2_nv's layout. */
struct swoole_http2_client_coro_nv {
    const uint8_t *name;
    const uint8_t *value;
    size_t namelen;
    size_t valuelen;
    uint8_t flags;
};

/* Defined by ext-src/swoole_http2_client_coro.cc: the coroutine socket and
 * nghttp2's HPACK codec stay C++, one call per operation. */
struct _zend_object *swoole_http2_client_coro_create_socket(const char *host,
                                                            size_t length,
                                                            struct _zend_string **converted,
                                                            void **socket,
                                                            int *error);
void swoole_http2_client_coro_setup(void *socket, void *conn);
bool swoole_http2_client_coro_enable_ssl(void *socket);
void swoole_http2_client_coro_protocol(void *socket, const struct _zval_struct *settings);
void swoole_http2_client_coro_apply(void *socket, const struct _zval_struct *settings);
bool swoole_http2_client_coro_connect(void *socket, const char *host, size_t length, int port);
bool swoole_http2_client_coro_close(void *socket);
bool swoole_http2_client_coro_is_connected(void *socket);
bool swoole_http2_client_coro_write_bound(void *socket);
ssize_t swoole_http2_client_coro_send_all(void *socket, const char *data, size_t length);
int swoole_http2_client_coro_error(void *socket, const char **message);
void swoole_http2_client_coro_set_error(void *socket, int code, const char *message);
ssize_t swoole_http2_client_coro_recv_packet(void *socket, double timeout);
const char *swoole_http2_client_coro_read_buffer(void *socket);
void *swoole_http2_client_coro_inflater(int *error);
void *swoole_http2_client_coro_deflater(uint32_t table_size, int *error);
void swoole_http2_client_coro_hpack_free(void *inflater, void *deflater);
int swoole_http2_client_coro_deflate_table(void *deflater, uint32_t size);
size_t swoole_http2_client_coro_deflate_bound(void *deflater,
                                              const struct swoole_http2_client_coro_nv *headers,
                                              size_t count);
ssize_t swoole_http2_client_coro_deflate(void *deflater,
                                         uint8_t *out,
                                         size_t capacity,
                                         const struct swoole_http2_client_coro_nv *headers,
                                         size_t count);
ssize_t swoole_http2_client_coro_inflate(void *inflater,
                                         const uint8_t *in,
                                         size_t length,
                                         int *flags,
                                         struct swoole_http2_client_coro_nv *header);
void swoole_http2_client_coro_inflate_end(void *inflater);
const char *swoole_http2_client_coro_strerror(int code);
void swoole_http2_client_coro_set_cookies(const char *at,
                                          size_t length,
                                          struct _zval_struct *cookies,
                                          struct _zval_struct *set_cookie_headers);
void swoole_http2_client_coro_unconstructed(void);
void swoole_http2_client_coro_last_error(int code);
const char *swoole_http2_client_coro_swoole_strerror(int code);
void swoole_http2_client_coro_warning(const char *message);
/* Upstream's logs: 0 unknown stream (notice), 1 decompress failed, 2 inflateInit2 failed. */
void swoole_http2_client_coro_log(int what, uint32_t stream_id);

#ifdef __cplusplus
}
#endif
#endif
