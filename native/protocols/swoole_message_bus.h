/* SPDX-License-Identifier: Apache-2.0 */
/* Swoole's include/swoole_message_bus.h, owned here: MessageBus as a view of the Rust
 * bus (crates/protocols/src/bus.rs), with only the members C++ still calls (native/,
 * and core-tests' server/message_bus.cpp in the protocols gate). Upstream's file is
 * deleted, so `#include "swoole_message_bus.h"` finds this copy. */

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

class MessageBus {
    // First: crates/process reads it.
    swr_bus *bus_;
    const Allocator *allocator_;
    std::function<uint64_t(void)> id_generator_;

    // A native frame: the allocator may be Zend's, which can bail out.
    ssize_t receive(network::Socket *sock, bool dgram) {
        swr_bus_read state{};
        for (;;) {
            const ssize_t retval = swr_v1_bus_read(bus_, sock->get_fd(), dgram, &sock->kernel_nobufs, &state);
            if (state.event != SWR_BUS_ALLOCATE) {
                return retval;
            }
            auto allocator = state.data ? static_cast<const Allocator *>(state.allocator) : allocator_;
            void *data = state.data ? allocator->realloc(state.data, state.size) : allocator->malloc(state.size);
            if (data == nullptr) {
                throw std::bad_alloc();
            }
            swr_v1_bus_adopt(bus_, data, state.size, allocator);
        }
    }

  public:
    MessageBus() : bus_(swr_v1_bus_create(SW_BUFFER_SIZE_STD)), allocator_(sw_std_allocator()) {}
    MessageBus(const MessageBus &) = delete;
    MessageBus &operator=(const MessageBus &) = delete;
    ~MessageBus() {
        swr_v1_bus_free(bus_);
    }
    bool empty() const {
        return swr_v1_bus_count(bus_) == 0;
    }
    void clear() {
        swr_v1_bus_clear(bus_);
    }
    void set_allocator(const Allocator *allocator) {
        allocator_ = allocator;
    }
    void set_id_generator(const std::function<uint64_t(void)> &id_generator) {
        id_generator_ = id_generator;
        swr_v1_bus_set_id_source(
            bus_, [](void *fn) { return (*static_cast<std::function<uint64_t(void)> *>(fn))(); }, &id_generator_);
    }
    void count_ids(sw_atomic_long_t *counter) {
        swr_v1_bus_count_ids(bus_, const_cast<int64_t *>(counter));
    }
    void set_buffer_size(size_t buffer_size) {
        swr_v1_bus_set_buffer_size(bus_, buffer_size);
    }
    void set_always_chunked_transfer() {
        swr_v1_bus_set_always_chunked(bus_);
    }
    size_t get_buffer_size() const {
        return swr_v1_bus_buffer_size(bus_);
    }
    size_t get_memory_size() const {
        return swr_v1_bus_memory_size(bus_);
    }
    bool alloc_buffer() {
        void *buffer = allocator_->malloc(get_buffer_size());
        swr_v1_bus_set_buffer(bus_, buffer);
        return buffer != nullptr;
    }
    bool write(network::Socket *sock, SendData *packet) const {
        return swr_v1_bus_write(bus_, sock, &packet->info, packet->data);
    }
    // >0: a complete packet, 0: wait for more chunks, -1: an error.
    ssize_t read(network::Socket *sock) {
        return receive(sock, false);
    }
    ssize_t read_with_buffer(network::Socket *sock) {
        return receive(sock, true);
    }
    PacketPtr get_packet() const {
        const auto packet = swr_v1_bus_packet(bus_);
        return {packet.length, packet.data};
    }
    PipeBuffer *get_buffer() const {
        return static_cast<PipeBuffer *>(swr_v1_bus_buffer(bus_));
    }
    void pop() {
        swr_v1_bus_pop(bus_);
    }
    // The calling thread's own copy of a pipe's socket.
    network::Socket *get_pipe_socket(const network::Socket *sock) const {
        return static_cast<network::Socket *>(swr_v1_bus_pipe_socket(bus_, sock->get_fd()));
    }
};
}  // namespace swoole
