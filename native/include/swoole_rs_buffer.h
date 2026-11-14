#ifndef SWOOLE_RS_BUFFER_H
#define SWOOLE_RS_BUFFER_H
#include <stddef.h>
#include <stdint.h>
#ifdef __cplusplus
extern "C" {
#endif
typedef struct swr_v1_buffer swr_v1_buffer;
enum {
    SWR_BUFFER_OK = 0, SWR_BUFFER_EMPTY = 1,
    SWR_BUFFER_INVALID = -1, SWR_BUFFER_OVERFLOW = -2,
    SWR_BUFFER_NOMEM = -3, SWR_BUFFER_PENDING_NATIVE = -4
};
int32_t swr_v1_buffer_release(swr_v1_buffer *buffer);
int32_t swr_v1_buffer_append(swr_v1_buffer *buffer, const uint8_t *data, size_t length);
int32_t swr_v1_buffer_stats(const swr_v1_buffer *buffer, uint32_t *length, size_t *count);
#ifdef __cplusplus
}
#endif
#endif
