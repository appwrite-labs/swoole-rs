/* SPDX-License-Identifier: Apache-2.0 */
/* Native packet records and protocol callback signatures. The coroutine socket
 * embeds Protocol and checks its layout against Rust in swoole_rs_co_socket.h. */
#pragma once

#include "swoole_socket.h"

#include <netdb.h>

namespace swoole {
struct SendData {
    DataHead info;
    const char *data;
};

struct RecvData {
    DataHead info;
    const char *data;

    SessionId session_id() const {
        return info.fd;
    }
};

struct PacketLength {
    const char *buf;
    uint32_t buf_size;
    uint32_t header_len;
};

struct Protocol {
    bool split_by_eof;
    char package_eof[SW_DATA_EOF_MAXLEN];
    uint8_t package_eof_len;
    char package_length_type;
    uint8_t package_length_size;
    uint16_t package_length_offset;
    uint16_t package_body_offset;
    uint32_t package_max_length;
    void *private_data_1;
    void *private_data_2;
    int (*onPackage)(const Protocol *, network::Socket *, const RecvData *);
    ssize_t (*get_package_length)(const Protocol *, network::Socket *, PacketLength *pl);
    uint8_t (*get_package_length_size)(network::Socket *);
};
}  // namespace swoole
