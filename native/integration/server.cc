// SPDX-License-Identifier: Apache-2.0
// swoole::Server's constructor and destructor: building and destroying its C++
// members and deleting its ListenPorts, which Rust can't do. The rest is
// crates/server's swr_v1_server_open and swr_v1_server_close.
#include "swoole_server.h"

namespace swoole {
Server::Server(Mode mode)
    : detail::ServerFields(SW_CPU_NUM, SwooleG.max_sockets, get_timezone(), &SwooleG.std_allocator) {
    if (!swr_v1_server_open(this, mode, sizeof(ServerGS))) {
        throw std::bad_alloc();
    }
    message_bus.count_ids(&gs->pipe_packet_msg_id);
    tables_->message_bus = &message_bus;
}

Server::~Server() {
    swr_v1_server_close(this, [](void *port) { delete static_cast<ListenPort *>(port); });
}
}  // namespace swoole
