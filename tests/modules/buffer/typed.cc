// Ordinary DATA cursor and inert CLOSE tokens only. No callback/lifetime probes.
#include "swoole.h"
#include "swoole_buffer.h"
#include <cstring>
#include <iostream>
#include <stdexcept>
static_assert(sizeof(swoole::Buffer::TypedFront) == 40 && alignof(swoole::Buffer::TypedFront) == 8, "Typed front C/Rust layout");
static_assert(offsetof(swoole::Buffer::TypedFront, data) == 16 && offsetof(swoole::Buffer::TypedFront, token) == 24 && offsetof(swoole::Buffer::TypedFront, generation) == 32, "Typed front C/Rust offsets");
static_assert(sizeof(swr_v1_buffer_typed_detached) == 16, "Typed detach C/Rust layout");
static unsigned checks;
static void require(bool condition) { ++checks; if (!condition) throw std::runtime_error("typed queue assertion"); }
int main() {
    for (uint32_t chunk : {1u, 3u, 16u, 0u}) {
        swoole::Buffer queue(chunk);
        const char bytes[] = {'a', 0, char(255), 'b', 'c', 'd', 'e'};
        queue.append(bytes, sizeof(bytes));
        queue.typed_close();
        size_t copied = 0;
        while (queue.typed_front().kind == 0) {
            const auto front = queue.typed_front();
            require(front.offset == 0);
            require(std::memcmp(front.data, bytes + copied, front.length) == 0);
            queue.typed_consume(front, 1);
            auto next = queue.typed_front();
            require(next.offset == 1 && next.generation == front.generation);
            require(queue.length() == sizeof(bytes) - copied);
            queue.typed_consume(next, next.length - next.offset);
            copied += next.length;
            queue.pop();
            require(queue.length() == sizeof(bytes) - copied);
        }
        require(copied == sizeof(bytes));
        require(queue.typed_front().kind == 2);
        queue.pop();
        require(queue.empty() && queue.length() == 0);
    }
    std::cout << "{\"schemas\":4,\"checks\":" << checks << ",\"buffer_size\":" << sizeof(swoole::Buffer) << "}\n";
}
