#ifndef SWOOLE_RS_HTTP_CONTEXT_H
#define SWOOLE_RS_HTTP_CONTEXT_H
#include "swoole_rs_http.h"
#ifdef __cplusplus
extern "C" {
#endif
/* The static handler's context (crates/http/src/static_context.rs), for
 * tests/modules/http/rewrite_gate.cc; the server drives it in Rust. */
typedef struct swr_v2_http_context swr_v2_http_context;
enum swr_v2_http_context_text {
    SWR_HTTP_CONTEXT_URL=0, SWR_HTTP_CONTEXT_ORIGINAL=1, SWR_HTTP_CONTEXT_DIRECTORY=2,
    SWR_HTTP_CONTEXT_INDEX=3, SWR_HTTP_CONTEXT_FILENAME=4, SWR_HTTP_CONTEXT_BOUNDARY=5,
    SWR_HTTP_CONTEXT_CONTENT=6, SWR_HTTP_CONTEXT_END=7
};
swr_v2_http_context *swr_v2_http_context_new(const uint8_t *, size_t, size_t);
void swr_v2_http_context_drop(swr_v2_http_context *);
int32_t swr_v2_http_context_text(const swr_v2_http_context *, uint32_t, uint8_t *, size_t, size_t *);
#ifdef __cplusplus
}
#endif
#endif
