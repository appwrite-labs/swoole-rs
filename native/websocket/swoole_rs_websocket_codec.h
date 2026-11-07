#ifndef SWOOLE_RS_WEBSOCKET_CODEC_H
#define SWOOLE_RS_WEBSOCKET_CODEC_H
#include <stdbool.h>
#include <stddef.h>
#include <stdint.h>
#ifdef __cplusplus
extern "C" {
#endif
/* Module-local extension of ABI v1. See tests/modules/websocket/codec.md.
 * All pointers are borrowed for the call. No allocation or callbacks. */
typedef struct {
    size_t payload_length;
    size_t header_length;
    uint8_t opcode;
    uint8_t flags;
    uint8_t length_code;
    uint8_t mask_key[4];
} swr_ws_decoded;
/* Positive expected frame length; 0 incomplete header; -1 invalid/overflow;
 * SWR_WS_INVALID_CONTROL for a complete compressed control frame (left masked).
 * Incomplete bodies return expected size without changing payload bytes.
 * Output is initialized on valid arguments, including incomplete frames. */
#define SWR_WS_INVALID_CONTROL (-2)
int64_t swr_v1_ws_decode(uint8_t *data, size_t length, swr_ws_decoded *output);
/* Close, ping and pong frames must be final and at most 125 bytes; encode enforces it. */
bool swr_v1_ws_control_invalid(uint8_t opcode, uint8_t flags, size_t length);
/* Actual bytes written, including header/key only when FLAG_ENCODE_HEADER_ONLY.
 * 0 rejects overflow. No input/output overlap is permitted for encoding. */
size_t swr_v1_ws_encoded_size(size_t length, uint8_t flags);
size_t swr_v1_ws_encode(const uint8_t *data, size_t length, uint8_t opcode, uint8_t flags,
                       uint8_t *output, size_t capacity);
size_t swr_v1_ws_encode_close(int32_t code, const uint8_t *reason, size_t length, uint8_t flags,
                             uint8_t *output, size_t capacity);
enum swr_ws_action {
    SWR_WS_REJECT = 0, SWR_WS_DISPATCH = 1, SWR_WS_START = 2,
    SWR_WS_APPEND = 3, SWR_WS_FINISH = 4, SWR_WS_CONTROL = 5,
    SWR_WS_CLIENT_CLOSE = 6, SWR_WS_SERVER_CLOSE = 7, SWR_WS_UNKNOWN = 8
};
enum swr_ws_error {
    SWR_WS_NO_ERROR = 0, SWR_WS_ORPHAN = 1, SWR_WS_TOO_BIG = 2,
    SWR_WS_OVERLAP = 3, SWR_WS_CONTROL_TOO_BIG = 4, SWR_WS_CLOSE_TOO_BIG = 5
};
typedef struct {
    size_t fragment_length;
    size_t package_max_length;
    uint16_t fragment_flags;
    uint8_t has_fragment;
    uint8_t closing;
} swr_ws_context;
typedef struct {
    size_t offset;
    size_t length;
    uint32_t action;
    uint32_t error;
    uint16_t ext_flags;
} swr_ws_decision;
/* Pure pinned dispatch policy; caller executes allocations, dispatch and I/O.
 * Frame must describe a complete decoded frame within wire_length. */
int32_t swr_v1_ws_decide(const swr_ws_decoded *frame, size_t wire_length,
                        const swr_ws_context *context, swr_ws_decision *output);
/* PHP-facing pure policy; Zend/zlib/socket operations remain native.
 * Pack: low byte normalized flags, bit 8 requests compression.
 * Message: settings bits close=1,ping=2,pong=4; return deliver=0,drop=1,pong=2. */
uint16_t swr_v1_ws_pack_policy(uint8_t opcode, uint8_t flags, size_t length);
uint8_t swr_v1_ws_message_policy(uint8_t opcode, uint8_t settings);
#ifdef __cplusplus
}
#endif
#endif
