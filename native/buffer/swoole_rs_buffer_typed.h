#pragma once
#include "swoole_rs_buffer.h"
#ifdef __cplusplus
extern "C" {
#endif
typedef void (*swr_v1_buffer_dispose)(void *);
typedef struct swr_v1_buffer_typed_front_result {
    uint32_t kind; /* 0=DATA, 1=sendfile native token, 2=CLOSE */
    uint32_t length;
    uint32_t offset;
    uint8_t *data;
    void *token;
    uint64_t generation;
} swr_v1_buffer_typed_front_result;
typedef struct swr_v1_buffer_typed_detached {
    void *token;
    swr_v1_buffer_dispose dispose;
} swr_v1_buffer_typed_detached;
int32_t swr_v1_buffer_typed_create(uint32_t, swr_v1_buffer **);
int32_t swr_v1_buffer_typed_push(swr_v1_buffer *, uint32_t, void *, swr_v1_buffer_dispose);
int32_t swr_v1_buffer_typed_front(const swr_v1_buffer *, swr_v1_buffer_typed_front_result *);
int32_t swr_v1_buffer_typed_consume(swr_v1_buffer *, uint64_t, uint32_t, uint32_t);
int32_t swr_v1_buffer_typed_pop(swr_v1_buffer *, swr_v1_buffer_typed_detached *);
#ifdef __cplusplus
}
#endif
