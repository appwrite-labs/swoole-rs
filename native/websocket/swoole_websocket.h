/* SPDX-License-Identifier: Apache-2.0 */
// The WebSocket frame codec's C++ face: what the compiled core-tests still use.
// crates/websocket defines the functions under their upstream symbols and fills Frame.
#pragma once

#include "swoole_protocol.h"

#define SW_WEBSOCKET_GUID "258EAFA5-E914-47DA-95CA-C5AB0DC85B11"
#define SW_WEBSOCKET_HEADER_LEN 2
#define SW_WEBSOCKET_MASK_LEN 4
#define SW_WEBSOCKET_FRAME_HEADER_SIZE (SW_WEBSOCKET_HEADER_LEN + SW_WEBSOCKET_MASK_LEN + sizeof(uint64_t))

namespace swoole {
namespace websocket {

enum Status {
    STATUS_HANDSHAKE = 2,
    STATUS_ACTIVE = 3,
};

enum Flag {
    FLAG_FIN = 1 << 0,
    FLAG_RSV1 = 1 << 2,
    FLAG_MASK = 1 << 5,
    FLAG_ENCODE_HEADER_ONLY = 1 << 6,
};

enum Opcode {
    OPCODE_TEXT = 0x1,
    OPCODE_CLOSE = 0x8,
    OPCODE_PING = 0x9,
    OPCODE_PONG = 0xa,
};

enum CloseReason {
    CLOSE_POLICY_ERROR = 1008,
};

// fin:1 rsv1:1 rsv2:1 rsv3:1 opcode:4, then mask:1 length:7: crates/websocket writes it.
struct Header {
    uchar OPCODE : 4;
    uchar RSV3 : 1;
    uchar RSV2 : 1;
    uchar RSV1 : 1;
    uchar FIN : 1;
    uchar LENGTH : 7;
    uchar MASK : 1;
};

struct Frame {
    Header header;
    char mask_key[SW_WEBSOCKET_MASK_LEN];
    uint16_t header_length;
    size_t payload_length;
    char *payload;
};

bool encode(String *buffer, const char *data, size_t length, uint8_t opcode, uint8_t flags);
bool decode(Frame *frame, char *data, size_t length);
void mask(char *data, size_t len, const char *mask_key);
bool pack_close_frame(String *buffer, int code, const char *reason, size_t length, uint8_t flags);
void print_frame(const Frame *frame);
ssize_t get_package_length(const Protocol *protocol, network::Socket *conn, PacketLength *pl);

static inline void parse_ext_flags(uint16_t ext_flags, uchar *opcode, uchar *flags) {
    *opcode = (uchar) (ext_flags >> 8);
    *flags = (uchar) (ext_flags & 0xFF);
}

}  // namespace websocket
}  // namespace swoole
