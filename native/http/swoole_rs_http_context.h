#ifndef SWOOLE_RS_HTTP_CONTEXT_H
#define SWOOLE_RS_HTTP_CONTEXT_H
#include "swoole_rs_http.h"
#ifdef __cplusplus
extern "C" {
#endif
typedef struct swr_v2_http_context swr_v2_http_context;
const int64_t *swr_v3_http_context_mtime(const swr_v2_http_context *);
typedef struct {
    size_t filesize;
    int64_t mtime;
    size_t content_length, directory_count, task_count;
    int32_t status;
    uint32_t kind, file_ready, last;
} swr_v2_http_observation;
typedef struct {
    uint32_t event, observed, has_locations, has_index, autoindex;
    swr_v1_http_proxy_text value;
} swr_v2_http_serve_observation;
/* Immutable synchronous filesystem primitives; null selects direct Rust libc.
 * Callbacks retain no arguments and must not throw or perform a Zend bailout. */
typedef struct {
    bool (*stat)(const char *, swr_v2_http_observation *);
    char *(*canonical)(const char *, char *);
    void *(*open)(const char *);
    const char *(*read)(void *);
    void (*close)(void *);
} swr_v3_http_static_io;
int32_t swr_v3_http_context_stat(swr_v2_http_context *, const swr_v3_http_static_io *);
int32_t swr_v3_http_context_directory(swr_v2_http_context *, const swr_v3_http_static_io *);
int32_t swr_v3_http_context_serve(swr_v2_http_context *, const swr_v1_http_proxy_text *,
    const swr_v2_http_serve_observation *, const swr_v3_http_static_io *);
/* Events begin0/config, locations-finished1, stat2, canonical3, MIME4. Native
 * effects run after return; canonical bytes are copied during this call only. */
int32_t swr_v2_http_context_serve(swr_v2_http_context *, const swr_v1_http_proxy_text *,
    const swr_v2_http_serve_observation *);
enum swr_v2_http_context_text {
    SWR_HTTP_CONTEXT_URL=0, SWR_HTTP_CONTEXT_ORIGINAL=1, SWR_HTTP_CONTEXT_DIRECTORY=2,
    SWR_HTTP_CONTEXT_INDEX=3, SWR_HTTP_CONTEXT_FILENAME=4, SWR_HTTP_CONTEXT_BOUNDARY=5,
    SWR_HTTP_CONTEXT_CONTENT=6, SWR_HTTP_CONTEXT_END=7
};
swr_v2_http_context *swr_v2_http_context_new(const uint8_t *, size_t, size_t);
void swr_v2_http_context_drop(swr_v2_http_context *);
int32_t swr_v2_http_context_text(const swr_v2_http_context *, uint32_t, uint8_t *, size_t, size_t *);
int32_t swr_v2_http_context_observe(const swr_v2_http_context *, swr_v2_http_observation *);
int32_t swr_v2_http_context_observed(swr_v2_http_context *, uint32_t, const swr_v2_http_observation *);
/* Admit the positive RNG byte count requested by static_boundary into empty
 * owned storage. Prefix and full boundary construction stay in Rust. */
int32_t swr_v2_http_context_boundary(swr_v2_http_context *, const uint8_t *, size_t);
int32_t swr_v2_http_context_format(swr_v2_http_context *, uint32_t);
typedef struct {
    swr_v1_http_proxy_text mime;
    uint32_t boundary_ready, modified;
} swr_v2_http_range_observation;
int32_t swr_v2_http_context_range_begin(swr_v2_http_context *, size_t, uint32_t);
int32_t swr_v2_http_context_range_step(swr_v2_http_context *, const uint8_t *, size_t);
int32_t swr_v2_http_context_range_resume(swr_v2_http_context *, const swr_v2_http_range_observation *);
int32_t swr_v2_http_context_path(swr_v2_http_context *, uint32_t, const uint8_t *, size_t, uint32_t);
typedef struct { size_t required, length; } swr_v2_http_page_buffer;
/* Actions skip0/reserve1(required)/extend2(required)/written3(length). Buffer
 * storage is observed fresh after Native reserve/extend; no pointer is retained. */
int32_t swr_v2_http_context_page_emit(const swr_v2_http_context *, size_t,
    const swr_v1_http_index_page_input *, uint8_t *, size_t, size_t, swr_v2_http_page_buffer *);
/* Private selected page protocol: begin!=0 requests header1; resume requests
 * entry2(index), footer3, done0. Native buffer effects run after each call. */
int32_t swr_v2_http_context_page_step(swr_v2_http_context *, uint32_t, size_t *);
/* Events begin0/opened1/open-failed2/entry3/end4/closed5.
 * Actions reject0/accept1/open2/read3/close4. Entry bytes never escape the call. */
int32_t swr_v2_http_context_scan(swr_v2_http_context *, uint32_t, const uint8_t *, size_t);
/* Events begin0(configured count), scanned1(fresh count), candidate2(name),
 * autoindex3(enabled), filename4(success), scan-failed6.
 * Actions reject0/accept1/scan2/candidate3(cursor)/filename4/autoindex5. */
int32_t swr_v2_http_context_index(swr_v2_http_context *, uint32_t, size_t, const uint8_t *, size_t, size_t *);
int32_t swr_v2_http_context_page(const swr_v2_http_context *, size_t, const swr_v1_http_index_page_input *, uint8_t *, size_t, size_t *);
int32_t swr_v2_http_context_tasks(const swr_v2_http_context *, swr_v2_http_static_task *, size_t, size_t *);
#ifdef __cplusplus
}
#endif
#endif
