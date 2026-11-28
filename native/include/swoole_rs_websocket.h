#ifndef SWOOLE_RS_WEBSOCKET_H
#define SWOOLE_RS_WEBSOCKET_H
#include <stddef.h>
#include <stdint.h>
#ifdef __cplusplus
extern "C" {
#endif
/* Borrowed ranges: docs/abi-v1.md. */
int32_t swr_v1_ws_mask(uint8_t *data, size_t length, const uint8_t *key);
#ifdef __cplusplus
}
#endif
#endif
