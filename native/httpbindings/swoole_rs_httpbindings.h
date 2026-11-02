#ifndef SWOOLE_RS_HTTPBINDINGS_H
#define SWOOLE_RS_HTTPBINDINGS_H
#include <stddef.h>
#include <stdint.h>
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
/* Swoole\Http\Cookie (Rust): MINIT passes upstream's method table, built from
 * the stub's arginfo; the methods are Rust's zim_swoole_http_cookie_* exports.
 * `warning` is php_swoole_error(E_WARNING, "%s", message); `unavailable` sets
 * SW_ERROR_HTTP_COOKIE_UNAVAILABLE. */
struct _zend_function_entry;
struct _zend_string;
struct _zval_struct;
void swr_v1_httpbindings_cookie_minit(const struct _zend_function_entry *methods,
                                      void (*warning)(const char *message), void (*unavailable)(void));
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
enum {
 SWR_HTTPBINDINGS_WRITE_HTTP2=1, SWR_HTTPBINDINGS_WRITE_BUFFER,
 SWR_HTTPBINDINGS_WRITE_HEADER, SWR_HTTPBINDINGS_WRITE_READ,
 SWR_HTTPBINDINGS_WRITE_EMPTY, SWR_HTTPBINDINGS_WRITE_CHUNK,
 SWR_HTTPBINDINGS_WRITE_HEADER_FAILED
};
int32_t swr_v1_httpbindings_write(uint32_t *, uint32_t);
enum {
 SWR_HTTPBINDINGS_FILE_HTTP2=40, SWR_HTTPBINDINGS_FILE_READ_HEADERS,
 SWR_HTTPBINDINGS_FILE_MIME, SWR_HTTPBINDINGS_FILE_HEADER,
 SWR_HTTPBINDINGS_FILE_SEND, SWR_HTTPBINDINGS_FILE_HEADER_FAILED,
 SWR_HTTPBINDINGS_FILE_CLOSE_FAILED, SWR_HTTPBINDINGS_FILE_MARK_END,
 SWR_HTTPBINDINGS_FILE_CLOSE_DONE, SWR_HTTPBINDINGS_FILE_DONE
};
int32_t swr_v1_httpbindings_file(uint32_t *, uint32_t);
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
int32_t swr_v1_httpbindings_accept(uint32_t,int64_t,int32_t,const swr_v1_httpbindings_accept_errors *);

/* PHP Swoole\Http\Request and Swoole\Http\Response (crates/phphttp). MINIT
 * passes upstream's method tables and gets the registered class back; both
 * objects hold their swoole::http::Context pointer. */
struct _zend_class_entry;
struct _zend_function_entry;
struct _zend_object;
struct _zend_string;
struct _zval_struct;
struct _zend_class_entry *swr_v1_phphttp_request_minit(const struct _zend_function_entry *methods);
struct _zend_class_entry *swr_v1_phphttp_response_minit(const struct _zend_function_entry *methods);
void *swr_v1_phphttp_context(const struct _zend_object *object);
void swr_v1_phphttp_set_context(struct _zend_object *object, void *context);

/* What those methods ask of the context, implemented in ext-src over
 * swoole::http::Context until it moves to Rust. */
enum { SWR_PHPHTTP_DETACHED = 2, SWR_PHPHTTP_SEND_TRAILER = 256 };
enum { SWR_PHPHTTP_HEADER = 0, SWR_PHPHTTP_COOKIE = 1, SWR_PHPHTTP_TRAILER = 2 };
void swr_http_host_error(int code, int always, const char *message);
uint32_t swr_http_host_flags(const void *context);
int64_t swr_http_host_fd(const void *context);
void swr_http_host_mark(void *context, uint32_t flag);
void swr_http_host_request_free(void *context);
bool swr_http_host_request_create(struct _zval_struct *return_value, struct _zval_struct *options);

/* The request parser (crates/phphttp/src/parse.rs): llhttp and multipart
 * callbacks over swoole::http::ContextState, whose layout state.rs mirrors.
 * `context` is a swoole::http::Context. MINIT passes the codecs built in
 * (1 zlib, 2 brotli, 4 zstd, 8 any). */
enum { SWR_PHPHTTP_PARSE_SIZE = 144, SWR_PHPHTTP_STATE_SIZE = 680 };
void swr_v1_phphttp_parse_minit(uint32_t codecs);
size_t swr_v1_phphttp_parse(void *context, const char *data, size_t length);
void swr_v1_phphttp_parse_free(void *context);
bool swr_v1_phphttp_multipart_boundary(void *context, const char *at, size_t length, size_t offset,
    char **boundary, int *boundary_length);
bool swr_v1_phphttp_multipart_init(void *context, const char *boundary, int length);
bool swr_v1_phphttp_multipart_parse(void *context, const char *at, size_t length);
void swr_v1_phphttp_compression_method(void *context, const char *at, size_t length);
/* What the parser asks of Swoole. `swr_http_host_log` is swoole_error_log
 * (error > 0), swoole_warning (0) or swoole_sys_warning (-errno) from `func`. */
void swr_http_host_log(int level, int error, const char *func, const char *message);
void swr_http_host_treat_data(int arg, char *str, struct _zval_struct *array);
int swr_http_host_websocket_connection(void *context);
const char *swr_http_host_upload_tmp_dir(const void *context);
int swr_http_host_tmpfile(char *path);
const char *swr_http_host_http2_data(void *context, size_t *length);

/* The response header, trailer and body compression (crates/phphttp/src/header.rs).
 * The header and trailer are Rust's thread-local bytes until the next call. */
const char *swr_v1_phphttp_build_header(void *context, const char *body, size_t length, size_t *header_length);
const char *swr_v1_phphttp_build_trailer(const void *context, size_t *length);
bool swr_v1_phphttp_compress(void *context, const char *data, size_t length);
/* What they ask of the context: brotli/zstd (1 done, 0 failed, -1 not built),
 * the zlib_buffer, whether a content type is compressible (a null type asks
 * whether the server lists types) and the cached Date. */
int swr_http_host_compress_other(void *context, int method, const char *data, size_t length);
char *swr_http_host_compression_buffer(void *context, size_t size);
void swr_http_host_compressed(void *context, size_t length);
size_t swr_http_host_compressed_length(const void *context);
bool swr_http_host_compression_type(const void *context, const char *type, size_t length);
struct _zend_string *swr_http_host_date(void);
/* Calls that can run user code (a value's __toString while building the
 * response, exit() in it) trap a Zend bailout in C++: request_create,
 * response_free, write, end, redirect and push return true, send_file and
 * upgrade -1, when one happened, and Rust re-raises it (docs/zend.md). */
bool swr_http_host_response_free(void *context);
void swr_http_host_response_create(struct _zval_struct *object, int64_t fd, struct _zval_struct *return_value);
struct _zval_struct *swr_http_host_response_property(void *context, int which);
bool swr_http_host_write(void *context, struct _zend_string *data, struct _zval_struct *return_value);
bool swr_http_host_end(void *context, struct _zend_string *data, struct _zval_struct *return_value);
int swr_http_host_send_file(void *context, struct _zend_string *file, int64_t offset, int64_t length);
bool swr_http_host_redirect(void *context, struct _zval_struct *url, struct _zval_struct *code, struct _zval_struct *return_value);
void swr_http_host_status(void *context, int64_t status, const char *reason, size_t reason_length);
bool swr_http_host_set_header(void *context, const char *key, size_t key_length, struct _zval_struct *value, bool format);
void swr_http_host_disconnect(void *context, int64_t code, struct _zend_string *reason, struct _zval_struct *return_value);
bool swr_http_host_push(void *context, struct _zval_struct *data, int64_t opcode, struct _zval_struct *flags,
    struct _zval_struct *return_value);
void swr_http_host_ping(void *context, struct _zend_string *data, struct _zval_struct *return_value);
bool swr_http_host_goaway(void *context, int64_t code, struct _zend_string *debug);
int swr_http_host_upgrade(void *context);
bool swr_http_host_close(void *context);
void swr_http_host_recv(void *context, double timeout, struct _zval_struct *return_value);
#ifdef __cplusplus
}
#endif
#endif
