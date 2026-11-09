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
typedef struct { size_t key_offset, key_length, value_offset, value_length; uint32_t has_value; } swr_v1_http_cookie;
int32_t swr_v1_http_cookie_next(uint8_t *, size_t, size_t *, swr_v1_http_cookie *);
typedef struct { uint32_t phase, last; size_t url_length, filename_length; } swr_v1_http_static_path_state;
int32_t swr_v1_http_static_location(const uint8_t *, size_t, const uint8_t *, size_t, swr_v1_http_static_path_state *);
struct swr_v1_core_proxy;
int32_t swr_v1_http_proxy_owned_authorization(const struct swr_v1_core_proxy *, uint8_t *, size_t *);
/* HttpProxy's create, pack and handshake (crates/http-ffi/src/owned_proxy.rs). */
void swr_v1_http_proxy_configure(struct swr_v1_core_proxy *, const char *host, size_t, int32_t port,
                                 const char *user, size_t, const char *password, size_t);
size_t swr_v1_http_proxy_connect(const struct swr_v1_core_proxy *, const char *host, size_t, char *output, size_t capacity);
bool swr_v1_http_proxy_accept(const char *, size_t);
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
