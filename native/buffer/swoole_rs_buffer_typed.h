#pragma once
#include "swoole_rs_buffer.h"
#ifdef __cplusplus
extern "C" {
#endif
typedef struct swr_v1_buffer_typed_front_result {
    uint32_t kind; /* 0=DATA, 1=sendfile native token, 2=CLOSE */
    uint32_t length;
    uint32_t offset;
    uint8_t *data;
    void *token;
    uint64_t generation;
} swr_v1_buffer_typed_front_result;
#ifdef __cplusplus
}
#endif
