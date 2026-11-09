#ifndef SWOOLE_RS_HTTP_H
#define SWOOLE_RS_HTTP_H
#include <stddef.h>
#include <stdint.h>
#ifdef __cplusplus
extern "C" {
#endif
typedef struct { size_t offset, length; } swr_v1_http_span;

typedef struct { int64_t offset; size_t length; char part_header[256]; } swr_v2_http_static_task;

typedef struct {
    const uint8_t *data; size_t length;
} swr_v1_http_range_text;
typedef struct {
    swr_v1_http_range_text boundary, mime;
    size_t offset, length, filesize; uint32_t part, separator;
} swr_v1_http_range_format_input;
int32_t swr_v1_http_range_format(const swr_v1_http_range_format_input *, uint8_t *, size_t, size_t *);
int32_t swr_v1_http_multipart_disposition(const uint8_t *, size_t);
int32_t swr_v1_http_multipart_filename(uint8_t *, size_t);
typedef struct { size_t key_offset, key_length, value_offset, value_length; uint32_t has_value; } swr_v1_http_cookie;
int32_t swr_v1_http_cookie_next(uint8_t *, size_t, size_t *, swr_v1_http_cookie *);
typedef struct { const uint8_t *data; size_t length; } swr_v1_http_proxy_text;
typedef struct {
    swr_v1_http_proxy_text target, host, version, authorization;
    int32_t port; uint32_t authenticated;
} swr_v1_http_proxy_request;
typedef struct {
    swr_v1_http_proxy_text status, date, modified, content_type, software, page;
    int64_t content_length, range_offset; size_t range_length, filesize;
    uint32_t variant, keep_alive, range_kind;
} swr_v1_http_static_header;
typedef struct {
    size_t tm_size, tm_align;
    bool (*parse)(const char *, const char *, void *);
    int64_t (*to_time)(void *);
    void *(*to_gmt)(int64_t);
} swr_v3_http_calendar;
int32_t swr_v3_http_conditional(const char *, size_t, uint32_t, const int64_t *, const long *, const swr_v3_http_calendar *);
size_t swr_v3_http_date(const int64_t *, uint8_t *, size_t);
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

typedef struct { uint32_t phase, last; size_t url_length, filename_length; } swr_v1_http_static_path_state;
int32_t swr_v1_http_static_location(const uint8_t *, size_t, const uint8_t *, size_t, swr_v1_http_static_path_state *);
int32_t swr_v1_http_static_path(const uint8_t *, size_t, const uint8_t *, size_t, uint8_t *, size_t, swr_v1_http_static_path_state *, uint32_t);
const uint8_t *swr_v1_http_static_boundary(size_t, size_t *);
int32_t swr_v1_http_static_headers(const swr_v1_http_static_header *, uint8_t *, size_t, size_t *);
struct swr_v1_core_proxy;
int32_t swr_v1_http_proxy_owned_authorization(const struct swr_v1_core_proxy *, uint8_t *, size_t *);
int32_t swr_v1_http_proxy_owned_pack(const struct swr_v1_core_proxy *, swr_v1_http_proxy_text host, swr_v1_http_proxy_text version, swr_v1_http_proxy_text authorization, uint8_t *, size_t, size_t *);
int32_t swr_v2_http_proxy_authorization(const swr_v1_http_proxy_text *, uint8_t *, size_t *);
int32_t swr_v1_http_proxy_credentials(const swr_v1_http_proxy_text *, uint8_t *, size_t *);
int32_t swr_v1_http_proxy_pack(const swr_v1_http_proxy_request *, uint8_t *, size_t, size_t *);
int32_t swr_v1_http_proxy_handshake(const uint8_t *, size_t);
/* HttpProxy's create, pack and handshake (crates/http/src/owned_proxy.rs). */
void swr_v1_http_proxy_configure(struct swr_v1_core_proxy *, const char *host, size_t, int32_t port,
                                 const char *user, size_t, const char *password, size_t);
size_t swr_v1_http_proxy_connect(const struct swr_v1_core_proxy *, const char *host, size_t, char *output, size_t capacity);
bool swr_v1_http_proxy_accept(const char *, size_t);
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
