/* SPDX-License-Identifier: Apache-2.0 */
#ifndef SWOOLE_RS_CLIENTS_RECORD_H
#define SWOOLE_RS_CLIENTS_RECORD_H
#include "swoole_socket.h"
#include "swoole_protocol.h"
#include <type_traits>

/* ClientRecord in crates/clients/src/client.rs. Original public fields keep
 * their C++ types: Protocol methods, Address copies and enum assignments work
 * on the same storage Rust initializes and reads. Native owns nontrivial fields. */
struct swr_v1_clients_record {
    bool http2;
    bool open_length_check;
    bool open_eof_check;
    swoole::Protocol protocol;
    swoole::network::Address server_addr;
    swoole::network::Address remote_addr;
    swoole::network::Socket *socket;
    void *object;
    swoole::String *buffer;
};
static_assert(std::is_trivially_copyable<swr_v1_clients_record>::value, "Client record is POD");
static_assert(sizeof(swoole::Protocol) == 64 && offsetof(swoole::Protocol, package_max_length) == 16 &&
              offsetof(swoole::Protocol, private_data_1) == 24, "Protocol ABI");
static_assert(offsetof(swr_v1_clients_record, protocol) == 8 &&
              offsetof(swr_v1_clients_record, server_addr) == 72 &&
              offsetof(swr_v1_clients_record, remote_addr) == 72 + sizeof(swoole::network::Address) &&
              offsetof(swr_v1_clients_record, socket) == 72 + 2 * sizeof(swoole::network::Address) &&
              sizeof(swr_v1_clients_record) == 96 + 2 * sizeof(swoole::network::Address), "Client record ABI");
#endif
