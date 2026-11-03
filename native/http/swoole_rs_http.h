#ifndef SWOOLE_RS_HTTP_H
#define SWOOLE_RS_HTTP_H
#include <stddef.h>
#include <stdint.h>
#ifdef __cplusplus
extern "C" {
#endif
typedef struct { size_t offset, length; } swr_v1_http_span;

/* http_server::Request's native effects (crates/http/src/request.rs, src/protocol/http.cc's
 * request_native(op, a, b, n, m)). */
enum swr_v1_http_native {
    SWR_HTTP_NATIVE_STRING_NEW=0, SWR_HTTP_NATIVE_STRING_FREE=1, SWR_HTTP_NATIVE_STRING_APPEND=2,
    SWR_HTTP_NATIVE_PARSER_NEW=3, SWR_HTTP_NATIVE_PARSER_EXECUTE=4, SWR_HTTP_NATIVE_PARSER_FREE=5,
    SWR_HTTP_NATIVE_TMPFILE=6, SWR_HTTP_NATIVE_FDOPEN=7, SWR_HTTP_NATIVE_FWRITE=8, SWR_HTTP_NATIVE_FFLUSH=9,
    SWR_HTTP_NATIVE_FCLOSE=10, SWR_HTTP_NATIVE_UNLINK=11, SWR_HTTP_NATIVE_NUMBER_ERRORS=12,
    SWR_HTTP_NATIVE_LOG_PARSER_NEW=13, SWR_HTTP_NATIVE_LOG_FOPEN=14, SWR_HTTP_NATIVE_LOG_WRITE=15,
    SWR_HTTP_NATIVE_LOG_PARSE_REASON=16, SWR_HTTP_NATIVE_LOG_PARSE_PARTIAL=17, SWR_HTTP_NATIVE_TRACE_CHUNK=18,
    SWR_HTTP_NATIVE_TRACE_HEADER_FIELD=19, SWR_HTTP_NATIVE_TRACE_HEADER_VALUE=20, SWR_HTTP_NATIVE_TRACE_DATA=21,
    SWR_HTTP_NATIVE_TRACE_HEADERS_COMPLETE=22, SWR_HTTP_NATIVE_TRACE_DATA_END=23,
    SWR_HTTP_NATIVE_TRACE_PART_BEGIN=24, SWR_HTTP_NATIVE_TRACE_BODY_END=25, SWR_HTTP_NATIVE_TRACE_EXECUTE=26
};


typedef struct swr_v2_http_tasks swr_v2_http_tasks;
typedef struct { int64_t offset; size_t length; char part_header[256]; } swr_v2_http_static_task;
swr_v2_http_tasks *swr_v2_http_tasks_new(void);
void swr_v2_http_tasks_drop(swr_v2_http_tasks *);
int32_t swr_v2_http_tasks_begin(swr_v2_http_tasks *);
int32_t swr_v2_http_tasks_push(swr_v2_http_tasks *);
size_t swr_v2_http_tasks_count(const swr_v2_http_tasks *);
int32_t swr_v2_http_tasks_copy(const swr_v2_http_tasks *, size_t, swr_v2_http_static_task *);
int32_t swr_v2_http_tasks_clear(swr_v2_http_tasks *);
int32_t swr_v2_http_tasks_snapshot(const swr_v2_http_tasks *, swr_v2_http_static_task *, size_t, size_t *);

typedef struct swr_v2_http_directory swr_v2_http_directory;
swr_v2_http_directory *swr_v2_http_directory_new(void);
void swr_v2_http_directory_drop(swr_v2_http_directory *);
int32_t swr_v2_http_directory_clone(const swr_v2_http_directory *, swr_v2_http_directory **);
int32_t swr_v2_http_directory_scan_append(swr_v2_http_directory *, const uint8_t *, size_t);
void swr_v2_http_directory_scan_finish(swr_v2_http_directory *);
int32_t swr_v2_http_directory_insert(swr_v2_http_directory *, const uint8_t *, size_t);
size_t swr_v2_http_directory_count(const swr_v2_http_directory *);
int32_t swr_v2_http_directory_contains(const swr_v2_http_directory *, const uint8_t *, size_t);

typedef struct {
    const uint8_t *data; size_t length;
} swr_v1_http_range_text;
typedef struct {
    swr_v1_http_range_text boundary, mime;
    size_t offset, length, filesize; uint32_t part, separator;
} swr_v1_http_range_format_input;
int32_t swr_v1_http_range_format(const swr_v1_http_range_format_input *, uint8_t *, size_t, size_t *);
int32_t swr_v2_http_tasks_prepare(swr_v2_http_tasks *, int64_t, size_t, const swr_v1_http_range_format_input *, size_t *);
int32_t swr_v1_http_multipart_disposition(const uint8_t *, size_t);
int32_t swr_v1_http_multipart_filename(uint8_t *, size_t);
typedef struct { size_t key_offset, key_length, value_offset, value_length; uint32_t has_value; } swr_v1_http_cookie;
int32_t swr_v1_http_cookie_next(uint8_t *, size_t, size_t *, swr_v1_http_cookie *);
// Invocation-local Rust policy state; no resource owner or pointer is retained.
typedef struct { uint32_t phase, source_inside; size_t source_offset, required, cursor; swr_v1_http_cookie cookie; } swr_v2_http_cookie_flow_state;
int32_t swr_v2_http_cookie_flow(swr_v2_http_cookie_flow_state *, const uint8_t *, size_t, uint8_t *, size_t, uint32_t, uint32_t);
typedef struct { const uint8_t *data; size_t length; } swr_v1_http_proxy_text;
int32_t swr_v1_http_rewrite_rule(const uint8_t *, size_t, swr_v1_http_span *, uint32_t *);
typedef struct {
    swr_v1_http_proxy_text target, host, version, authorization;
    int32_t port; uint32_t authenticated;
} swr_v1_http_proxy_request;
typedef struct {
    swr_v1_http_proxy_text status, date, modified, content_type, software, page;
    int64_t content_length, range_offset; size_t range_length, filesize;
    uint32_t variant, keep_alive, range_kind;
} swr_v1_http_static_header;
/* Static response selection scratch; values are copied only at their phase.
 * No native owner/borrow/callback enters Rust. See contract.md for effect order. */
typedef struct { uint32_t phase, head, range_kind; } swr_v1_http_static_selection;
enum swr_v1_http_static_select_action {
    SWR_HTTP_SELECT_SKIP=0, SWR_HTTP_SELECT_STATUS=1, SWR_HTTP_SELECT_NOT_FOUND=2,
    SWR_HTTP_SELECT_DATES=3, SWR_HTTP_SELECT_CHECK_MODIFIED=4, SWR_HTTP_SELECT_NOT_MODIFIED=5,
    SWR_HTTP_SELECT_INDEX=6, SWR_HTTP_SELECT_INSPECT_INDEX=7, SWR_HTTP_SELECT_AUTOINDEX=8,
    SWR_HTTP_SELECT_FILE=9, SWR_HTTP_SELECT_FILE_HEADER=10, SWR_HTTP_SELECT_READY=11
};
int32_t swr_v1_http_static_select(swr_v1_http_static_selection *, size_t, int32_t);
enum swr_v1_http_static_body_action {
    SWR_HTTP_BODY_DONE=0, SWR_HTTP_BODY_ALLOCATE=1, SWR_HTTP_BODY_PART_HEADER=2,
    SWR_HTTP_BODY_FILE=3, SWR_HTTP_BODY_END_PART=4, SWR_HTTP_BODY_FREE=5,
    SWR_HTTP_BODY_KEEP_ALIVE=6, SWR_HTTP_BODY_CLOSE=7
};
typedef struct { size_t count, cursor; uint32_t phase, head; } swr_v1_http_static_body_state;
int32_t swr_v1_http_static_body(swr_v1_http_static_body_state *, size_t, uint32_t);
enum swr_v1_http_date_action {
    SWR_HTTP_DATE_IGNORE=0, SWR_HTTP_DATE_GMT=1, SWR_HTTP_DATE_UTC=2,
    SWR_HTTP_DATE_RFC850=3, SWR_HTTP_DATE_ASCTIME=4, SWR_HTTP_DATE_FILE_GMT=5,
    SWR_HTTP_DATE_PARSED_TIME=6, SWR_HTTP_DATE_FILE_TIME=7, SWR_HTTP_DATE_MATCH=8
};
typedef struct {
    uint32_t phase, kind, valid; size_t length;
    int64_t timezone, file_mtime, value, parsed_time;
} swr_v1_http_conditional_state;
const char *swr_v1_http_date_format(uint32_t);
int32_t swr_v1_http_conditional(swr_v1_http_conditional_state *);
enum swr_v1_http_path_action {
    SWR_HTTP_PATH_SKIP=0, SWR_HTTP_PATH_SERVE=1, SWR_HTTP_PATH_LOCATIONS=2,
    SWR_HTTP_PATH_DIRECTORY=3, SWR_HTTP_PATH_STAT=4, SWR_HTTP_PATH_CANONICAL=5,
    SWR_HTTP_PATH_MIME=6, SWR_HTTP_PATH_ERROR=7
};
enum swr_v1_http_path_fact {
    SWR_HTTP_PATH_HAS_LOCATIONS=1, SWR_HTTP_PATH_STAT_OK=2, SWR_HTTP_PATH_CANONICAL_OK=4,
    SWR_HTTP_PATH_LINK=8, SWR_HTTP_PATH_IS_DIRECTORY=16, SWR_HTTP_PATH_FILE=32,
    SWR_HTTP_PATH_INDEX=64, SWR_HTTP_PATH_AUTOINDEX=128, SWR_HTTP_PATH_MIME_OK=256
};
typedef struct {
    swr_v1_http_proxy_text path, entry, css, powered;
    uint32_t part;
} swr_v1_http_index_page_input;
int32_t swr_v1_http_index_page(const swr_v1_http_index_page_input *, uint8_t *, size_t, size_t *);
int32_t swr_v2_http_directory_page(const swr_v2_http_directory *, size_t, const swr_v1_http_index_page_input *, uint8_t *, size_t, size_t *);

typedef struct { uint32_t phase, last; size_t url_length, filename_length; } swr_v1_http_static_path_state;
int32_t swr_v1_http_static_index_path(uint8_t *, size_t, size_t, const uint8_t *, size_t);
int32_t swr_v1_http_static_location(const uint8_t *, size_t, const uint8_t *, size_t, swr_v1_http_static_path_state *);
int32_t swr_v1_http_static_path(const uint8_t *, size_t, const uint8_t *, size_t, uint8_t *, size_t, swr_v1_http_static_path_state *, uint32_t);
int32_t swr_v1_http_transport_route(uint32_t, uint32_t);
int32_t swr_v1_http_proxy_auth_enabled(size_t, size_t);
int32_t swr_v1_http_unknown_status(int32_t, uint8_t *, size_t, size_t *);
const uint8_t *swr_v1_http_static_boundary(size_t, size_t *);
const uint8_t *swr_v1_http_static_content_prefix(size_t);
int32_t swr_v1_http_static_content_type(const swr_v1_http_proxy_text *, uint8_t *, size_t, size_t *);
int32_t swr_v1_http_static_catch(uint32_t, int32_t *);
int32_t swr_v1_http_static_headers(const swr_v1_http_static_header *, uint8_t *, size_t, size_t *);
int32_t swr_v2_http_proxy_authorization(const swr_v1_http_proxy_text *, uint8_t *, size_t *);
int32_t swr_v1_http_proxy_credentials(const swr_v1_http_proxy_text *, uint8_t *, size_t *);
int32_t swr_v1_http_proxy_pack(const swr_v1_http_proxy_request *, uint8_t *, size_t, size_t *);
int32_t swr_v1_http_proxy_handshake(const uint8_t *, size_t);
const char *swr_v1_http_status(int32_t);
const char *swr_v2_http_status_message(int32_t);
int32_t swr_v1_http_method(const uint8_t *, size_t);
const char *swr_v1_http_method_string(int32_t);
int32_t swr_v1_http_url_capacity(const uint8_t *, size_t, size_t *);
int32_t swr_v1_http_url_encode(const uint8_t *, size_t, uint8_t *, size_t, size_t *);
int32_t swr_v1_http_url_decode(uint8_t *, size_t, size_t *);
int32_t swr_v1_http_boundary(const uint8_t *, size_t, size_t, swr_v1_http_span *);
#ifdef __cplusplus
}
#endif
#endif
