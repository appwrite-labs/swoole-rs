#ifndef SWOOLE_RS_HTTPBINDINGS_H
#define SWOOLE_RS_HTTPBINDINGS_H
#include <stddef.h>
#include <stdint.h>
#include <sys/types.h>
#ifndef __cplusplus
#include <stdbool.h>
#endif
#ifdef __cplusplus
extern "C" {
#endif
typedef struct { const uint8_t *data; size_t length; } swr_v1_httpbindings_text;
typedef struct {
    swr_v1_httpbindings_text fields[6];
    int64_t expires; uint32_t flags; double age;
} swr_v1_httpbindings_cookie_input;
typedef struct {
    uint32_t kind; swr_v1_httpbindings_text text; int64_t value;
} swr_v1_httpbindings_action;
enum {
    SWR_HTTPBINDINGS_APPEND = 1, SWR_HTTPBINDINGS_WARN = 2,
    SWR_HTTPBINDINGS_ENCODE = 3, SWR_HTTPBINDINGS_APPEND_ENCODED = 4,
    SWR_HTTPBINDINGS_FREE_ENCODED = 5, SWR_HTTPBINDINGS_FORMAT_DATE = 6,
    SWR_HTTPBINDINGS_APPEND_DATE = 7, SWR_HTTPBINDINGS_FREE_DATE = 8,
    SWR_HTTPBINDINGS_READ_AGE = 9, SWR_HTTPBINDINGS_APPEND_AGE = 10,
    SWR_HTTPBINDINGS_DONE = 11
};
int32_t swr_v1_httpbindings_cookie(const swr_v1_httpbindings_cookie_input *, uint32_t *, swr_v1_httpbindings_action *);
/* Swoole\Http\Cookie (Rust): Rust registers the class with its method table
 * (crates/phpstubs' arginfo); the methods are its zim_swoole_http_cookie_*.
 * `warning` is php_swoole_error(E_WARNING, "%s", message); `unavailable` sets
 * SW_ERROR_HTTP_COOKIE_UNAVAILABLE. */
struct _zend_string;
struct _zval_struct;
void swr_v1_httpbindings_cookie_minit(void (*warning)(const char *message), void (*unavailable)(void));
/* Response::cookie()'s Set-Cookie line from its arguments, or from a Cookie
 * object (reset when it cannot be sent); null after a warning. */
struct _zend_string *swr_v1_httpbindings_cookie_line(uint8_t encode, struct _zend_string *name,
    struct _zend_string *value, int64_t expires, struct _zend_string *path, struct _zend_string *domain,
    uint8_t secure, uint8_t http_only, struct _zend_string *same_site, struct _zend_string *priority,
    uint8_t partitioned);
struct _zend_string *swr_v1_httpbindings_cookie_object_line(const struct _zval_struct *object);
enum {
    SWR_HTTPBINDINGS_HEADER_APPEND = 0, SWR_HTTPBINDINGS_HEADER_COOKIE = 1,
    SWR_HTTPBINDINGS_HEADER_UPGRADE = 2, SWR_HTTPBINDINGS_HEADER_FORM = 3,
    SWR_HTTPBINDINGS_HEADER_MULTIPART = 4, SWR_HTTPBINDINGS_HEADER_COMPRESSION = 5
};
int32_t swr_v1_httpbindings_request_option(swr_v1_httpbindings_text, uint32_t);
int32_t swr_v1_httpbindings_compression(swr_v1_httpbindings_text, uint32_t);
swr_v1_httpbindings_text swr_v1_httpbindings_encoding(int32_t, uint32_t);
int32_t swr_v1_httpbindings_form_source(uint32_t);
int32_t swr_v1_httpbindings_body_prefix(swr_v1_httpbindings_text, uint32_t, size_t *);
int32_t swr_v1_httpbindings_request_header(swr_v1_httpbindings_text, swr_v1_httpbindings_text, uint32_t, uint32_t);
int32_t swr_v1_httpbindings_request_key(swr_v1_httpbindings_text);
int32_t swr_v1_httpbindings_tokens(uint8_t *, size_t, const uint8_t *, size_t, uint32_t);
typedef struct { size_t path_length, query_offset, query_length; } swr_v1_httpbindings_url;
typedef struct { size_t offset, length; } swr_v1_httpbindings_trimmed;
int32_t swr_v1_httpbindings_request_url(swr_v1_httpbindings_text, swr_v1_httpbindings_url *);
int32_t swr_v1_httpbindings_trim_quotes(uint8_t *, size_t, swr_v1_httpbindings_trimmed *);
int32_t swr_v1_httpbindings_codec(int32_t, uint32_t, uint32_t, size_t, uint32_t);
int32_t swr_v1_httpbindings_codec_level(int32_t, int32_t, int32_t, int32_t);
int32_t swr_v1_httpbindings_response_header(swr_v1_httpbindings_text);
int32_t swr_v1_httpbindings_file_range(int64_t, int64_t, int64_t, int64_t *);
int32_t swr_v1_httpbindings_response_key(uint8_t *, size_t);
typedef struct { uint32_t entry, alias, stage; } swr_v1_httpbindings_settings_cursor;
enum {
 SWR_HTTPBINDINGS_SETTING_LOOKUP=1, SWR_HTTPBINDINGS_SETTING_COOKIE,
 SWR_HTTPBINDINGS_SETTING_POST, SWR_HTTPBINDINGS_SETTING_FILES,
 SWR_HTTPBINDINGS_SETTING_COMPRESSION, SWR_HTTPBINDINGS_SETTING_LEVEL,
 SWR_HTTPBINDINGS_SETTING_MINIMUM, SWR_HTTPBINDINGS_SETTING_TYPES,
 SWR_HTTPBINDINGS_SETTING_WEBSOCKET, SWR_HTTPBINDINGS_SETTING_UPLOAD,
 SWR_HTTPBINDINGS_SETTING_DONE
};
int32_t swr_v1_httpbindings_settings(swr_v1_httpbindings_settings_cursor *, uint32_t, uint32_t, swr_v1_httpbindings_action *);
uint32_t swr_v1_httpbindings_server_level(int64_t);
int32_t swr_v1_httpbindings_handler(swr_v1_httpbindings_text, swr_v1_httpbindings_text);
typedef struct {int32_t per_process,system,timeout,bad_ssl,canceled;} swr_v1_httpbindings_accept_errors;
/* Upstream's Coroutine\Http\Server::start accept loop over its host's effects:
 * 1 is RETURN_TRUE, 0 RETURN_FALSE after a failed accept. */
typedef struct {
    void *data;
    bool (*running)(void *);
    int64_t (*accept)(void *, bool *);
    int32_t (*error)(void *, const char **);
    void (*sleep)(void *);
    void (*set_error)(void *);
    void (*fail)(void *, swr_v1_httpbindings_text);
} swr_v1_httpbindings_accept_host;
int32_t swr_v1_httpbindings_accept_loop(const swr_v1_httpbindings_accept_host *, const swr_v1_httpbindings_accept_errors *);

/* PHP Swoole\Http\Request and Swoole\Http\Response (crates/phphttp). MINIT
 * registers each with its Rust method table and returns the class; both
 * objects hold their swoole::http::Context pointer. */
struct _zend_class_entry;
struct _zend_object;
struct _zend_string;
struct _zval_struct;
struct _zend_class_entry *swr_v1_phphttp_request_minit(void);
struct _zend_class_entry *swr_v1_phphttp_response_minit(void);
/* Swoole\Coroutine\Http\Server (crates/phphttp/src/server_coro.rs): MINIT
 * registers it and returns the class; the HTTP/2 session's request handler
 * calls the dispatch. */
struct _zend_class_entry *swr_v1_phphttp_server_coro_minit(struct _zend_class_entry *exception, bool short_name,
    bool compression);
void swr_v1_phphttp_server_coro_http2_request(void *context, void *http1, void *server);
/* What it asks of C++: the settings the contexts copy that stay C++
 * (`settings`, an HttpServerSettings), the coroutine sockets (`sock`), a
 * socket's read buffer (`buffer`, a swoole::String) and the contexts it
 * creates. swr_hsc_call is 1 called, 0 failed, -1 bailed out. */
typedef struct {
    struct _zend_string *remote_addr;
    struct _zend_string *server_addr;
    int remote_port;
    int server_port;
} swr_hsc_peer_t;
/* The listening socket for `host`, and the new server's settings. */
void *swr_hsc_new(const char *host, size_t length, void **settings);
void swr_hsc_delete(void *sock, void *settings);
/* The handler map: `handle()` (1 set, 0 not callable, -1 bailed out) and a
 * request's callable (null for none). */
int swr_hsc_set_handler(void *settings, const char *pattern, size_t length, struct _zval_struct *callable);
void *swr_hsc_handler(void *settings, void *context);
int swr_hsc_call(void *callable, struct _zval_struct *args, bool coroutine);
void swr_hsc_cancel(void *sock);
bool swr_hsc_recv_waiting(void *sock);
int64_t swr_hsc_cid(void);
bool swr_hsc_bind(void *sock, const char *host, size_t length, int64_t port, bool reuse_port);
bool swr_hsc_listen(void *sock, bool ssl);
void swr_hsc_socket_info(void *sock, int *fd, int *port);
int swr_hsc_error(void *sock, const char **message);
void *swr_hsc_connection(struct _zval_struct *connection);
void *swr_hsc_connection_buffer(void *sock);
bool swr_hsc_ssl_handshake(void *sock);
void swr_hsc_peer(void *listener, void *sock, swr_hsc_peer_t *peer);
ssize_t swr_hsc_recv(void *sock, void *buffer);
void swr_hsc_recv_wait(void *sock, bool waiting);
size_t swr_hsc_package_max_length(void *sock);
void swr_hsc_extend(void *buffer, size_t size);
void swr_hsc_send_all(void *sock, const char *data, size_t length);
void *swr_hsc_context(void *sock, struct _zval_struct *connection);
/* HTTP/2 on the connection: the server's SETTINGS frame and frame-length
 * protocol, each frame (*retval as recv_packet; the read buffer's data), and
 * the sessions' request handler (a struct swr_h2s_request). */
void swr_hsc_http2_begin(void *sock);
const char *swr_hsc_recv_packet(void *sock, ssize_t *retval);
void swr_hsc_http2_request(void *request);
void swr_hsc_request_data(void *context, void *buffer, size_t length);
bool swr_hsc_closed(struct _zval_struct *connection);
/* `start`: the `onAccept` callable (0, 1 not callable, -1 bailed out), the
 * protocol settings, a setting by SWR_HTTPBINDINGS_SETTING_* kind (0 applied,
 * 1 refused, 2 no return value, -1 bailed out), and accepting a connection
 * into a new coroutine (its cid). */
struct _zend_execute_data;
struct _zend_array;
int swr_hsc_accept_callable(struct _zval_struct *object, struct _zend_execute_data *frame, void **callable);
void swr_hsc_accept_free(void *callable);
bool swr_hsc_set_protocol(void *sock, struct _zval_struct *settings);
const char *swr_hsc_strerror(int code);
void swr_hsc_accept_errors(swr_v1_httpbindings_accept_errors *errors);
/* *bailed: creating the onAccept coroutine bailed out; Rust re-raises it. */
int64_t swr_hsc_accept(void *sock, void *callable, bool *connected, bool *bailed);
void swr_hsc_sleep(void);
int swr_hsc_setting(
    void *server, uint32_t kind, struct _zend_array *table, struct _zval_struct *value, int64_t *result);
/* The compression types (reset by a null `text`, then one per call) and
 * upload_tmp_dir swr_hsc_setting reads for `server`, the Rust HttpServer. */
void swr_v1_phphttp_server_coro_text(const void *server, uint32_t kind, const char *text, size_t length);
void *swr_v1_phphttp_context(const struct _zend_object *object);
void swr_v1_phphttp_set_context(struct _zend_object *object, void *context);

/* What those methods ask of the context, implemented in ext-src over
 * swoole::http::Context until it moves to Rust. */
void swr_http_host_error(int code, int always, const char *message);
/* `new Context()`, which Context::free releases, and Context::init(Server *). */
void *swr_v1_phphttp_context_new(void);
void swr_v1_phphttp_context_init(void *ctx, void *server);

/* The request parser (crates/phphttp/src/parse.rs): llhttp and multipart
 * callbacks over swoole::http::ContextState, whose layout state.rs mirrors.
 * `context` is a swoole::http::Context. MINIT passes the codecs built in
 * (1 zlib, 2 brotli, 4 zstd, 8 any). */
enum { SWR_PHPHTTP_PARSE_SIZE = 144, SWR_PHPHTTP_STATE_SIZE = 768 };
void swr_v1_phphttp_parse_minit(uint32_t codecs);
size_t swr_v1_phphttp_parse(void *context, const char *data, size_t length);
void swr_v1_phphttp_parse_free(void *context);
bool swr_v1_phphttp_multipart_boundary(void *context, const char *at, size_t length, size_t offset,
    char **boundary, int *boundary_length);
bool swr_v1_phphttp_multipart_init(void *context, const char *boundary, int length);
bool swr_v1_phphttp_multipart_parse(void *context, const char *at, size_t length);

/* brotli/zstd output (swr_http_host_compress_other) as the context's
 * compressed body (crates/phphttp/src/header.rs). */
void swr_v1_phphttp_set_compressed(void *context, const char *data, size_t length);
/* Context::end (crates/phphttp/src/send.rs): true if a bailout was trapped,
 * which the caller re-raises. */
bool swr_v1_phphttp_end(void *context, struct _zend_string *data, struct _zval_struct *return_value);
/* What send.rs asks of the context: the after-response hook (true on a
 * bailout), the HTTP/2 server's end, write and send_file (-1 on a bailout),
 * a default Content-Type, and the WebSocket handshake's compression offer and
 * connection status. */
enum { SWR_PHPHTTP_HTTP2_END = 0, SWR_PHPHTTP_HTTP2_WRITE = 1, SWR_PHPHTTP_HTTP2_SEND_FILE = 2, SWR_PHPHTTP_HTTP2_PING = 3 };
bool swr_http_host_after(void *context);
int swr_http_host_http2(void *context, int op, struct _zend_string *data, int64_t offset, size_t length);
void swr_http_host_file_type(void *context, struct _zend_string *file);
/* What they ask of the context: brotli/zstd (1 done, 0 failed, -1 not built). */
int swr_http_host_compress_other(void *context, int method, const char *data, size_t length);
/* Calls that can run user code (a value's __toString while building the
 * response, exit() in it) trap a Zend bailout in C++: request_create,
 * response_free and redirect return true when one happened, and Rust
 * re-raises it (docs/zend.md). */
bool swr_http_host_redirect(void *context, struct _zval_struct *url, struct _zval_struct *code, struct _zval_struct *return_value);
bool swr_http_host_goaway(void *context, int64_t code, struct _zend_string *debug);
/* Response::create asks what an object is (1 a Swoole\Server, 2 a coroutine
 * socket, whose fd it sets) and for sw_server(). */
int swr_http_host_object_kind(struct _zval_struct *object, int64_t *fd);
void *swr_http_host_server(void);
/* Rust's side of the context: a new context's request and response objects,
 * Context::set_header (re-raising a bailout itself), and the RSHUTDOWN of the
 * cached Date header. */
void swr_v1_phphttp_context_objects(void *ctx, int64_t fd);
bool swr_v1_phphttp_set_header(void *ctx, const char *key, size_t length, struct _zval_struct *value, bool format);
void swr_v1_phphttp_response_rshutdown(void);
/* Context::free (crates/phphttp/src/context.rs) and what it and the response's
 * release ask of C++: a coroutine socket's state, a server connection's and
 * whether a reactor runs. */
void swr_v1_phphttp_context_free(void *ctx);
bool swr_http_host_socket_closed(const struct _zval_struct *socket);
bool swr_http_host_connection_open(void *server, int64_t fd);
bool swr_http_host_reactor(void);
/* The context's transport (context.rs): ctx->send and ctx->close, Context::bind
 * to a server or (`server` null) a coroutine socket object, Context::copy and
 * onBeforeRequest (false: not now). It sends through the server or the
 * coroutine socket, and reads the server's upload_tmp_dir and
 * http_compression_types (a null type asks whether it lists any). */
bool swr_v1_phphttp_send(void *ctx, const char *data, size_t length);
bool swr_v1_phphttp_close(void *ctx);
void swr_v1_phphttp_context_bind(void *ctx, void *server, struct _zval_struct *socket);
void swr_v1_phphttp_context_copy(void *ctx, const void *from);
bool swr_http_host_server_send(void *server, int64_t fd, const char *data, size_t length);
bool swr_http_host_server_sendfile(void *server, int64_t fd, struct _zend_string *file, int64_t offset, size_t length);
bool swr_http_host_server_close(void *server, int64_t fd);
bool swr_http_host_socket_send(void *sock, const char *data, size_t length);
bool swr_http_host_socket_sendfile(void *sock, struct _zend_string *file, int64_t offset, size_t length);
bool swr_http_host_socket_close(void *sock);
void *swr_http_host_socket(struct _zval_struct *socket);
const char *swr_http_host_server_upload_tmp_dir(void *server);
bool swr_http_host_server_compression_type(void *server, const char *type, size_t length);

/* Swoole\WebSocket\Server, Frame and CloseFrame (crates/phphttp/src/websocket.rs).
 * MINIT registers them and the constants, returning the server class and
 * filling `frame`. onMessage and FrameObject's members call the rest; each raises a bailout it
 * trapped once its Rust frames are done. `object` is a swoole::websocket::FrameObject. */
struct _zend_class_entry *swr_v1_phphttp_websocket_minit(int module_number, struct _zend_class_entry *http_server,
    bool serializable, bool zlib, struct _zend_class_entry **frame);
/* swoole_websocket_onMessage: SW_OK/SW_ERR. It calls onMessage through swr_ws_host_call. */
int swr_v1_phphttp_websocket_message(void *server, void *request, int64_t fd, int server_fd, uint16_t ext_flags);
void swr_v1_phphttp_websocket_frame_object(void *object, struct _zval_struct *data, int64_t opcode, int64_t flags,
    int64_t code);
struct _zend_string *swr_v1_phphttp_websocket_pack(const void *object);
/* websocket::recv_frame: `settings` packs in_server, open_ping_frame,
 * open_pong_frame and open_close_frame from bit 0; `slot` holds the message
 * being assembled (null when none; buffer_free releases it), `sock` is a
 * swoole::coroutine::Socket. It asks C++ to receive, send and read the fd. */
void swr_v1_phphttp_websocket_recv(uint32_t settings, void **slot, void *sock, struct _zval_struct *return_value,
                                   double timeout);
void swr_v1_phphttp_websocket_buffer_free(void **slot);
ssize_t swr_ws_host_sock_recv(void *sock, double timeout, char **data, size_t *length);
void swr_ws_host_sock_send(void *sock, const char *data, size_t length);
int swr_ws_host_sock_fd(void *sock);
/* What it asks of the Server (`server` is a swoole::Server, `context` a
 * swoole::http::Context): the checked server of `$this` (true when the check
 * bailed out), a connection's WebSocket state, sending (`wait`: yield on a
 * full buffer as Server::send does in a coroutine), closing, a port's frame
 * settings (close | ping << 1 | pong << 2, -1 without a port) and subprotocol,
 * the received data, and calling the server's callback for an event. */
typedef struct { int status; int server_fd; bool compression; bool closed; } swr_ws_connection;
enum { SWR_WS_ON_BEFORE_HANDSHAKE_RESPONSE = 0, SWR_WS_ON_OPEN = 1, SWR_WS_ON_MESSAGE = 2 };
enum { SWR_WS_CALL_BAILED = -1, SWR_WS_CALL_DONE = 0, SWR_WS_CALL_FAILED = 1, SWR_WS_CALL_NONE = 2 };
bool swr_ws_host_server(struct _zval_struct *object, void **server);
bool swr_ws_host_started(void *server);
bool swr_ws_host_connection(void *server, int64_t fd, bool verify, swr_ws_connection *out);
void swr_ws_host_set_status(void *server, int64_t fd, int64_t status);
bool swr_ws_host_send(void *server, int64_t fd, const char *data, size_t length, bool wait);
bool swr_ws_host_close(void *server, int64_t fd, bool reset);
int swr_ws_host_port_settings(void *server, int64_t fd);
const char *swr_ws_host_protocol(void *server, int server_fd, size_t *length);
void swr_ws_host_recv_data(void *server, void *request, struct _zval_struct *data);
int swr_ws_host_call(void *server, int server_fd, int event, uint32_t argc, struct _zval_struct *args);
struct _zval_struct *swr_ws_host_server_zval(void *server);
bool swr_ws_host_server_compression(void *server);
void swr_ws_host_set_compression(void *server, int64_t fd, bool compression);
void swr_ws_host_co_protocol(void *context);
bool swr_ws_host_set_header(void *context, const char *key, size_t key_length, const char *value, size_t length);

/* Swoole\Http\Server's per-worker state (crates/phphttp/src/server.rs): the
 * addresses cached for keep-alive sessions (`server` a swoole::Server, `conn`
 * its swoole::Connection, `table` the request's `server` array, `loopback` 4
 * or 6 for a loopback peer) and the requests queued past
 * worker_max_concurrency. What it asks of C++: an address, the worker's
 * counters (false without a server worker) and deferring a queued request's
 * handler. */
struct _zend_array;
void swr_v1_phphttp_server_addresses(void *server, void *conn, struct _zend_array *table, int64_t session,
                                     int64_t local_port, int64_t remote_port, int loopback, bool cache);
void swr_v1_phphttp_server_close(int64_t session);
void swr_v1_phphttp_server_rshutdown(void);
typedef struct { uint32_t *concurrency; uint32_t *server_concurrency; uint32_t max_concurrency; bool shutdown; } swr_http_worker;
const char *swr_http_host_address(void *server, void *conn, bool local);
bool swr_http_host_worker(swr_http_worker *worker);
void swr_http_host_defer(void *context);
/* php_swoole_http_server_onReceive. It returns what the trampoline does next:
 * return SW_ERR (CLOSED), hand the data to the TCP, WebSocket or HTTP/2
 * onReceive, or return SW_OK (DONE: Rust parsed the request and ran its
 * handler). It asks C++ for the connection and port facts (null: closed), a
 * new context holding the request data, the before-request hook, the port's
 * onHandshake or onRequest, a notice and zend::function::call (swr_hsc_call).
 * swr_http_host_defer defers swr_v1_phphttp_server_deferred, a queued
 * request's handler. */
enum {
    SWR_PHPHTTP_RECEIVE_CLOSED = 0,
    SWR_PHPHTTP_RECEIVE_TCP = 1,
    SWR_PHPHTTP_RECEIVE_WEBSOCKET = 2,
    SWR_PHPHTTP_RECEIVE_HTTP2 = 3,
    SWR_PHPHTTP_RECEIVE_DONE = 5,
};
int swr_v1_phphttp_server_receive(void *server, void *request, int64_t session, int server_fd);
void swr_v1_phphttp_server_deferred(void *ctx, bool coroutine);
typedef struct {
    double last_recv_time;
    int64_t local_port;
    int64_t remote_port;
    int loopback;
    uint8_t websocket_status;
    bool http;
    bool websocket;
    bool http2;
    bool cache;
    bool coroutine;
} swr_http_receive;
void *swr_http_host_receive(void *server, int64_t session, int server_fd, swr_http_receive *out);
void *swr_http_host_receive_context(void *server, void *conn, int64_t session, void *request,
                                    struct _zval_struct **zdata, swr_http_receive *out);
uint8_t swr_http_host_before_request(void *context, void *conn);
void *swr_http_host_callback(void *server, int server_fd, bool handshake);
void swr_http_host_notice(const char *message);
#ifdef __cplusplus
}
#endif
#endif
