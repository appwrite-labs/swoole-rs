// Runs upstream's core-tests/src/memory/buffer.cpp (through test_core.h) against the
// production Buffer: the hybrid build's buffer.cc over the Rust queue.
#include "swoole.h"
#include <cstdlib>
#include <iostream>
#include <stdexcept>

void original_buffer_append_iov();
swoole::Global SwooleG{};

int main() {
    // String's standard allocator is normally installed by Swoole startup.
    SwooleG.std_allocator = {std::malloc, std::calloc, std::realloc, std::free};
    SwooleG.pagesize = 4096;
    try {
        for (unsigned run = 0; run < 64; ++run) original_buffer_append_iov();
    } catch (const std::exception &error) {
        std::cerr << error.what() << '\n';
        return 1;
    }
    std::cout << "{\"original_assertion_runs\":64}\n";
}
