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
 * Rust registers the classes with their method table, their
 * properties and the HTTP/2 constants, and the methods are its
 * zim_swoole_http2_client_coro_* exports. */
struct _zend_class_entry;
struct _zend_object;
struct _zval_struct;
struct _zend_string;
void swr_v1_phphttp2_client_minit(struct _zend_class_entry *exception,
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


/* The HTTP/2 server's sessions (crates/phphttp2/src/session.rs), one per
 * connection `fd`, and their streams, each owning its request's context.
 * `handle` runs onRequest for a finished request (Rust traps a bailout
 * around it); `data` is passed along. */
struct swr_h2s_request {
    void *ctx;
    void *default_ctx;
    void *data;
};
typedef void (*swr_h2s_handler)(struct swr_h2s_request *request);
bool swr_v1_phphttp2_session_exists(int64_t fd);
void swr_v1_phphttp2_session_new(
    int64_t fd, void *default_ctx, uint32_t max_body_size, bool is_coro, swr_h2s_handler handle, void *data);
void swr_v1_phphttp2_session_free(int64_t fd);
/* Holds a context's stream (and so the context) across coroutine switches; null if none. */
const void *swr_v1_phphttp2_stream_hold(void *ctx);
void swr_v1_phphttp2_stream_release(const void *stream);

/* The HTTP/2 server (crates/phphttp2/src/server.rs): frames, the response and
 * the request header block. `ctx` is a swoole::http::Context. A bailout in
 * user code (a header's __toString, onRequest) is trapped and re-raised
 * before return. */
int swr_v1_phphttp2_session_parse(int64_t fd, const char *buf);
/* The same for a Rust caller, which raises a pending bailout itself. */
int swr_v1_phphttp2_session_receive(int64_t fd, const char *buf);
bool swr_v1_phphttp2_server_respond(void *ctx, const char *data, size_t length);
bool swr_v1_phphttp2_server_write(void *ctx, const char *data, size_t length);
bool swr_v1_phphttp2_server_ping(void *ctx);
bool swr_v1_phphttp2_server_goaway(void *ctx, int64_t code, const char *data, size_t length);
void swr_v1_phphttp2_server_reset(void *ctx, uint32_t stream_id, uint32_t code);
/* A file response's header: -1 if it was not sent, else whether a trailer follows. */
int swr_v1_phphttp2_server_begin_file(void *ctx, const char *content_type, size_t type_length, size_t length);
bool swr_v1_phphttp2_server_send_body(void *ctx, const char *data, size_t length, bool end_stream);
bool swr_v1_phphttp2_server_finish_file(void *ctx, bool trailer, bool error);

/* What it asks of C++, defined by ext-src/swoole_http2_server.cc. */
enum swr_h2s_field {
    SWR_H2S_END,
    SWR_H2S_SEND_HEADER,
    SWR_H2S_SEND_CHUNKED,
    SWR_H2S_STATUS,
    SWR_H2S_PARSE_BODY,
    SWR_H2S_FORM,
    SWR_H2S_MULTIPART,
    SWR_H2S_CO_SOCKET,
    SWR_H2S_ACCEPT_COMPRESSION,
    SWR_H2S_ENABLE_COMPRESSION,
    SWR_H2S_CONTENT_COMPRESSED,
    SWR_H2S_COMPRESSION_TYPES,
    SWR_H2S_STREAM_ID,
};
enum swr_h2s_value { SWR_H2S_SERVER, SWR_H2S_HEADER, SWR_H2S_RESPONSE, SWR_H2S_GET, SWR_H2S_POST, SWR_H2S_COOKIE };
/* A stream's new context: a copy of `default_ctx`'s settings, its request's streamId set. */
void *swr_h2s_stream_context(void *default_ctx, int64_t fd, uint32_t stream_id);
void swr_h2s_wait(void **coroutine);
void swr_h2s_wake(void *coroutine);
bool swr_h2s_send(void *ctx, const char *data, size_t length);
bool swr_h2s_close(void *ctx);
uint32_t swr_h2s_get(void *ctx, int field);
void swr_h2s_set(void *ctx, int field, uint32_t value);
struct _zval_struct *swr_h2s_zval(void *ctx, int which);
const char *swr_h2s_data(void *ctx, bool create, size_t *length);
void swr_h2s_data_append(void *ctx, const char *data, size_t length);
bool swr_h2s_body(void *ctx, bool multipart);
void swr_h2s_query(struct _zval_struct *array, const char *query, size_t length);
void swr_h2s_path(void *ctx, const char *path, size_t length);
void swr_h2s_cookie(struct _zval_struct *array, const char *cookie, size_t length);
bool swr_h2s_multipart(void *ctx, const char *value, size_t length);
void swr_h2s_set_header(void *ctx, const char *name, size_t name_length, const char *value, size_t length);
struct _zend_string *swr_h2s_date(void);
void swr_h2s_compression_method(void *ctx, const char *value, size_t length);
bool swr_h2s_compressible(void *ctx, const char *type, size_t length);
const char *swr_h2s_compress(void *ctx, const char *data, size_t length, size_t *compressed);
const char *swr_h2s_compressed(void *ctx, size_t *length);
bool swr_h2s_in_coroutine(void);
void swr_h2s_log(int level, int error, const char *func, const char *message);
void swr_h2s_error(int code, int always, const char *message);

#ifdef __cplusplus
}
#endif
#endif
