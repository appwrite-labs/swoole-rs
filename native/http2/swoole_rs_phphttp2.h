#ifndef SWOOLE_RS_PHPHTTP2_H
#define SWOOLE_RS_PHPHTTP2_H

#include <stdbool.h>
#include <stddef.h>
#include <stdint.h>
#include <sys/types.h>

#ifdef __cplusplus
extern "C" {
#endif

struct _zval_struct;
struct _zend_string;

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
/* An async server's finished request (crates/phphttp/src/server.rs): the
 * static handler or the port's onRequest. It asks for the connection (null:
 * closed) and what it reads of it and the server. */
void swr_v1_phphttp_server_http2_request(struct swr_h2s_request *request);
typedef struct {
    double last_recv_time;
    int64_t local_port;
    int64_t remote_port;
    int loopback;
    int server_fd;
    bool cache;
    bool static_handler;
    bool unavailable;
    bool coroutine;
} swr_h2s_connection_facts;
void *swr_h2s_connection(void *server, int64_t fd, swr_h2s_connection_facts *out);
bool swr_v1_phphttp2_session_exists(int64_t fd);
void swr_v1_phphttp2_session_new(
    int64_t fd, void *default_ctx, uint32_t max_body_size, bool is_coro, swr_h2s_handler handle, void *data);
void swr_v1_phphttp2_session_free(int64_t fd);

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
/* File responses (crates/phphttp2/src/files.rs): the static handler's answer
 * to a request (true when it served it) and Response::sendfile's. What they
 * ask of the StaticHandler (`handler`): a step or fact (SWR_H2S_STATIC_*), a
 * text (SWR_H2S_TEXT_*, a new string), a date check, the ranges and a task;
 * and of a file: opening it read-only (null if not), one read at an offset
 * (on the thread pool with `async`; -1 on failure), its whole content
 * (System::read_file in a coroutine; null if unreadable, `owner` freed by
 * swr_h2s_file_content_free) and its mime type. */
bool swr_v1_phphttp2_server_static(void *server, void *ctx);
bool swr_v1_phphttp2_server_send_file(void *ctx, struct _zend_string *file, int64_t offset, size_t length);
enum {
    SWR_H2S_STATIC_SERVE,
    SWR_H2S_STATIC_STATUS,
    SWR_H2S_STATIC_INDEX,
    SWR_H2S_STATIC_AUTOINDEX,
    SWR_H2S_STATIC_FILESIZE,
    SWR_H2S_STATIC_CONTENT_LENGTH,
    SWR_H2S_STATIC_TASKS,
};
enum {
    SWR_H2S_TEXT_INDEX_PAGE,
    SWR_H2S_TEXT_LAST_MODIFIED,
    SWR_H2S_TEXT_CONTENT_TYPE,
    SWR_H2S_TEXT_FILENAME,
    SWR_H2S_TEXT_END_PART,
    SWR_H2S_TEXT_PART_HEADER,
    SWR_H2S_TEXT_PAGE_404,
};
void *swr_h2s_static_new(void *server, const char *url, size_t length);
void swr_h2s_static_free(void *handler);
int64_t swr_h2s_static(void *handler, int op);
struct _zend_string *swr_h2s_static_text(void *handler, int which, size_t task);
bool swr_h2s_static_modified(void *handler, const char *date, size_t length);
void swr_h2s_static_range(void *handler, const char *range, const char *if_range);
void swr_h2s_static_task(void *handler, size_t task, int64_t *offset, size_t *length);
void *swr_h2s_file_open(const char *path);
ssize_t swr_h2s_file_read(void *file, int64_t offset, char *buf, size_t length, bool async);
void swr_h2s_file_close(void *file);
const char *swr_h2s_file_content(const char *path, size_t *length, void **owner);
void swr_h2s_file_content_free(void *owner);
const char *swr_h2s_mime_type(const char *path, size_t length);

/* What it asks of C++, defined by ext-src/swoole_http2_server.cc. */
/* A stream's new context: a copy of `default_ctx`'s settings, its request's streamId set. */
void *swr_h2s_stream_context(void *default_ctx, int64_t fd, uint32_t stream_id);
void swr_h2s_wait(void **coroutine);
void swr_h2s_wake(void *coroutine);
void swr_h2s_query(struct _zval_struct *array, const char *query, size_t length);
void swr_h2s_path(void *ctx, const char *path, size_t length);
void swr_h2s_cookie(struct _zval_struct *array, const char *cookie, size_t length);
bool swr_h2s_multipart(void *ctx, const char *value, size_t length);
void swr_h2s_set_header(void *ctx, const char *name, size_t name_length, const char *value, size_t length);
bool swr_h2s_in_coroutine(void);
void swr_h2s_log(int level, int error, const char *func, const char *message);
void swr_h2s_error(int code, int always, const char *message);

#ifdef __cplusplus
}
#endif
#endif
