#ifndef SWOOLE_RS_HTTP_H
#define SWOOLE_RS_HTTP_H
#include <stddef.h>
#include <stdint.h>
#ifdef __cplusplus
extern "C" {
#endif
typedef struct { size_t offset, length; } swr_v1_http_span;

int32_t swr_v1_http_multipart_disposition(const uint8_t *, size_t);
int32_t swr_v1_http_multipart_filename(uint8_t *, size_t);
typedef struct { uint32_t phase, last; size_t url_length, filename_length; } swr_v1_http_static_path_state;
int32_t swr_v1_http_static_location(const uint8_t *, size_t, const uint8_t *, size_t, swr_v1_http_static_path_state *);
int32_t swr_v1_http_method(const uint8_t *, size_t);
const char *swr_v1_http_method_string(int32_t);
int32_t swr_v1_http_boundary(const uint8_t *, size_t, size_t, swr_v1_http_span *);
#ifdef __cplusplus
}
#endif
#endif
