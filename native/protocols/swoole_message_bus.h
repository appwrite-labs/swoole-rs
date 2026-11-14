/* SPDX-License-Identifier: Apache-2.0 */
/* Swoole's include/swoole_message_bus.h, owned here: MessageBus as the layout of the
 * Rust bus's owner (crates/bus/src/lib.rs). Upstream's file is deleted, so
 * `#include "swoole_message_bus.h"` finds this copy. */

#pragma once

#include "swoole_string.h"
#include "swoole_socket.h"
#include "swoole_protocol.h"
#include "swoole_rs_protocols.h"

namespace swoole {
struct PipeBuffer {
    DataHead info;
    char data[0];
};

struct PacketPtr {
    size_t length;
    char *data;
};

struct DgramPacket {
    SocketType socket_type;
    network::Address socket_addr;
    uint32_t length;
    char data[0];
};

// The layout crates/server and crates/process read: the Rust bus, then the
// allocator of its buffer and packets. Rust makes, runs and frees it.
class MessageBus {
  public:
    swr_bus *bus_;
    const Allocator *allocator_;
};
}  // namespace swoole
