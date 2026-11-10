/* SPDX-License-Identifier: Apache-2.0 */
/* Swoole's include/swoole_protocol.h, owned here. The structs keep upstream's layout:
 * crates/framing, crates/ports, crates/server and crates/phpclients read them in place.
 * Every function is Rust under its upstream symbol (crates/ports/src/framing_recv.rs,
 * crates/corebase); only what C++ calls is declared. */
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

    static ssize_t default_length_func(const Protocol *protocol, network::Socket *socket, PacketLength *pl);
};
}  // namespace swoole

char *swoole_dec2hex(ulong_t value, int base);
ulong_t swoole_hex2dec(const char *hex, size_t *parsed_bytes);
int swoole_type_size(char type);
