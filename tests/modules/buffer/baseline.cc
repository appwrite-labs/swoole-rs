// Link against the actual pinned buffer.cc and string.cc, never replacement code.
#include "swoole.h"
#include "swoole_buffer.h"
#include <algorithm>
#include <iostream>
#include <stdexcept>
#include <vector>

void original_buffer_append_iov();
swoole::Global SwooleG{};
static uint32_t random_state = 1;
int swoole_rand(int minimum, int maximum) {
    random_state ^= random_state << 13;
    random_state ^= random_state >> 17;
    random_state ^= random_state << 5;
    return minimum + int(random_state % uint32_t(maximum - minimum + 1));
}

static void require(bool condition, const char *message) {
    if (!condition) throw std::runtime_error(message);
}

static unsigned callbacks = 0;
static std::vector<unsigned> callback_order;
static void destroy_native(swoole::BufferChunk *chunk) {
    ++callbacks;
    callback_order.push_back(chunk->value.u32);
}
static unsigned data_callbacks = 0;
static void destroy_data(swoole::BufferChunk *) { ++data_callbacks; }

static void print_bytes(const std::vector<char> &bytes) {
    std::cout << '[';
    for (size_t index = 0; index < bytes.size(); ++index) {
        if (index) std::cout << ',';
        std::cout << unsigned(static_cast<unsigned char>(bytes[index]));
    }
    std::cout << ']';
}

int main() {
    // String's standard allocator is normally installed by Swoole startup.
    SwooleG.std_allocator = {std::malloc, std::calloc, std::realloc, std::free};
    SwooleG.pagesize = 4096;
    try {
        for (unsigned seed = 1; seed <= 64; ++seed) {
            random_state = seed;
            original_buffer_append_iov();
        }
        for (uint32_t chunk_size : {0u, 1u, 4u, 8u}) {
            const std::vector<char> data{0, char(255), 1, 2, 0, 4, 5, char(128), 7, 8, 9};
            swoole::Buffer buffer(chunk_size);
            buffer.append(data.data(), uint32_t(data.size()));
            const auto effective_chunk_size = chunk_size ? chunk_size : uint32_t(INT_MAX);
            require(buffer.count() == (data.size() + effective_chunk_size - 1) / effective_chunk_size, "Contiguous append chunk count differs");
            auto *front = buffer.front();
            buffer.append(data.data(), 1);
            require(buffer.front() == front, "Append invalidated existing front");
            std::vector<char> copied;
            uint32_t remaining = uint32_t(data.size() + 1);
            while (!buffer.empty()) {
                auto *chunk = buffer.front();
                require(chunk->size == chunk->length, "Contiguous append underfilled chunk");
                copied.insert(copied.end(), chunk->value.str, chunk->value.str + chunk->length);
                remaining -= chunk->length;
                chunk->destroy = destroy_data;
                const auto old_callbacks = data_callbacks;
                buffer.pop();
                require(data_callbacks == old_callbacks + 1, "Data callback not called exactly once");
                require(buffer.length() == remaining, "Contiguous pop accounting differs");
            }
            auto expected = data;
            expected.push_back(data.front());
            require(copied == expected, "Contiguous binary bytes differ");
        }
        std::cout << "{\"original_assertion_runs\":64,\"cases\":[";
        bool comma = false;
        unsigned mismatches = 0;
        unsigned copied_mismatches = 0;
        for (uint32_t chunk_size : {0u, 1u, 4u, 8u}) {
            for (size_t offset : {size_t(0), size_t(1), size_t(4), size_t(5), size_t(6), size_t(10)}) {
                std::vector<char> bytes{0, char(255), 2, 3, 4, 0, 6, 7, char(128), 9, 10};
                iovec vectors[] = {{bytes.data(), 5}, {bytes.data() + 5, 6}};
                swoole::Buffer buffer(chunk_size);
                buffer.append(vectors, 2, offset);
                const uint32_t initial_length = buffer.length();
                const auto initial_count = buffer.count();
                std::vector<char> copied;
                uint32_t expected = uint32_t(bytes.size() - offset);
                bool mismatch = false;
                if (comma) std::cout << ',';
                comma = true;
                std::cout << "{\"chunk_size\":" << chunk_size << ",\"offset\":" << offset
                          << ",\"expected_copied_bytes\":" << expected << ",\"actual_initial_length\":" << initial_length
                          << ",\"initial_count\":" << initial_count << ",\"pops\":[";
                bool pop_comma = false;
                while (!buffer.empty()) {
                    auto *chunk = buffer.front();
                    require(chunk->type == swoole::BufferChunk::TYPE_DATA, "Incorrect data chunk type");
                    require(chunk->length <= chunk->size, "Copied length exceeds allocation");
                    require(chunk->offset == 0, "Initial offset not zero");
                    const uint32_t capacity = chunk->size, length = chunk->length;
                    copied.insert(copied.end(), chunk->value.str, chunk->value.str + length);
                    // Partial send changes native offset but not Buffer::length().
                    const auto before = buffer.length();
                    chunk->offset = std::min(1u, length);
                    require(buffer.front() == chunk && buffer.length() == before, "Front identity or partial-send accounting changed");
                    expected -= length;
                    buffer.pop();
                    mismatch |= buffer.length() != expected;
                    if (pop_comma) std::cout << ',';
                    pop_comma = true;
                    std::cout << "{\"capacity\":" << capacity << ",\"length\":" << length
                              << ",\"expected_remaining\":" << expected << ",\"actual_remaining\":" << buffer.length() << '}';
                }
                const bool copied_equal = std::equal(copied.begin(), copied.end(), bytes.begin() + offset, bytes.end());
                copied_mismatches += !copied_equal;
                mismatches += mismatch;
                std::cout << "],\"copied_bytes_equal\":" << (copied_equal ? "true" : "false")
                          << ",\"accounting_mismatch\":" << (mismatch ? "true" : "false") << ",\"actual_bytes\":";
                print_bytes(copied);
                std::cout << ",\"expected_bytes\":";
                print_bytes(std::vector<char>(bytes.begin() + offset, bytes.end()));
                std::cout << '}';
            }
        }
        // Minimal deterministic underflow: six input bytes, three skipped.
        char data[] = {0, 1, 2, 3, 4, 5};
        iovec vector{data, 6};
        swoole::Buffer underflow(4);
        underflow.append(&vector, 1, 3);
        require(underflow.length() == 3 && underflow.front()->length == 3, "Minimal reproducer copied length changed");
        require(underflow.front()->size == 4, "Pinned baseline allocation changed");
        underflow.pop();
        require(underflow.empty() && underflow.length() == UINT32_MAX, "Expected pinned baseline underflow not reproduced");
        {
            swoole::Buffer buffer(4);
            for (unsigned id = 1; id <= 3; ++id) {
                auto *chunk = buffer.alloc(id == 2 ? swoole::BufferChunk::TYPE_CLOSE : swoole::BufferChunk::TYPE_SENDFILE, 0);
                chunk->value.u32 = id;
                chunk->destroy = destroy_native;
            }
            buffer.pop();
            require(callbacks == 1 && callback_order.front() == 1, "Pop did not dispose native token exactly once");
        }
        require(callbacks == 3 && callback_order == std::vector<unsigned>({1, 2, 3}), "Teardown destructor ordering differs");
        std::cout << "],\"accounting_mismatch_cases\":" << mismatches
                  << ",\"copied_byte_mismatch_cases\":" << copied_mismatches
                  << ",\"contiguous_controls\":4,\"data_destructor_calls\":" << data_callbacks
                  << ",\"minimal_underflow\":" << underflow.length() << ",\"native_destructor_calls\":" << callbacks << "}\n";
        return 0;
    } catch (const std::exception &error) {
        std::cerr << error.what() << '\n';
        return 1;
    }
}
