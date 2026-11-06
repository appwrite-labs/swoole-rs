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

/* PHP Swoole\Http\Request and Swoole\Http\Response (crates/phphttp). MINIT
 * registers each with its Rust method table and returns the class; both
 * objects hold their swoole::http::Context pointer. */
struct _zend_class_entry;
struct _zend_object;
struct _zend_string;
struct _zval_struct;
struct _zend_class_entry *swr_v1_phphttp_request_minit(void);
void *swr_v1_phphttp_context(const struct _zend_object *object);
void swr_v1_phphttp_set_context(struct _zend_object *object, void *context);

/* `new Context()`, which Context::free releases, and Context::init(Server *). */
void *swr_v1_phphttp_context_new(void);
void swr_v1_phphttp_context_init(void *ctx, void *server);

/* The request parser (crates/phphttp/src/parse.rs): llhttp and multipart
 * callbacks over swoole::http::ContextState, whose layout state.rs mirrors.
 * `context` is a swoole::http::Context. */
enum { SWR_PHPHTTP_PARSE_SIZE = 144, SWR_PHPHTTP_STATE_SIZE = 768 };
size_t swr_v1_phphttp_parse(void *context, const char *data, size_t length);
void swr_v1_phphttp_parse_free(void *context);
bool swr_v1_phphttp_multipart_boundary(void *context, const char *at, size_t length, size_t offset,
    char **boundary, int *boundary_length);
bool swr_v1_phphttp_multipart_init(void *context, const char *boundary, int length);
bool swr_v1_phphttp_multipart_parse(void *context, const char *at, size_t length);

/* Context::free (crates/phphttp/src/context.rs). */
void swr_v1_phphttp_context_free(void *ctx);
/* The context's transport (context.rs): ctx->send and ctx->close, Context::bind
 * to a server or (`server` null) a coroutine socket object, Context::copy and
 * onBeforeRequest (false: not now). It sends through the server or the
 * coroutine socket. */
bool swr_v1_phphttp_send(void *ctx, const char *data, size_t length);
bool swr_v1_phphttp_close(void *ctx);
void swr_v1_phphttp_context_bind(void *ctx, void *server, struct _zval_struct *socket);
void swr_v1_phphttp_context_copy(void *ctx, const void *from);

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
 * swoole::coroutine::Socket. */
void swr_v1_phphttp_websocket_recv(uint32_t settings, void **slot, void *sock, struct _zval_struct *return_value,
                                   double timeout);
void swr_v1_phphttp_websocket_buffer_free(void **slot);
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

#ifdef __cplusplus
}
#endif
#endif
