#include "swoole.h"
#include "swoole_buffer.h"
#include "swoole_rs_buffer.h"
#include <algorithm>
#include <deque>
#include <iostream>
#include <stdexcept>
#include <vector>

swoole::Global SwooleG{};
#ifdef MODEL_FAILURES
extern "C" void swr_model_fail_after(int64_t);
extern "C" uint64_t swr_model_allocations();
extern "C" uint64_t swr_model_failures();
#endif
static uint64_t active_seed, active_step;
static uint32_t active_chunk;
static int64_t active_failure = -1;
static uint64_t append_calls = 0, vector_calls = 0, native_calls = 0, consumes = 0, pops = 0, release_refusals = 0;
static void check(bool ok, const char *message) {
    if (!ok) throw std::runtime_error(std::string(message) + " seed=" + std::to_string(active_seed) +
                                     " step=" + std::to_string(active_step) + " chunk=" + std::to_string(active_chunk) +
                                     " allocation_failure=" + std::to_string(active_failure));
}
struct Random {
    uint64_t state;
    uint64_t next() { state ^= state << 13; state ^= state >> 7; state ^= state << 17; return state; }
};
struct Item { std::vector<uint8_t> bytes; uint64_t *token = nullptr; uint32_t offset = 0; };
using Model = std::deque<Item>;
static uint32_t byte_count(const Model &model) {
    uint32_t n = 0;
    for (const auto &item : model) n += item.bytes.size();
    return n;
}
static void append_model(Model &model, const std::vector<uint8_t> &bytes, uint32_t chunk) {
    const size_t width = chunk ? chunk : INT32_MAX;
    for (size_t pos = 0; pos < bytes.size(); pos += width)
        model.push_back({{bytes.begin() + pos, bytes.begin() + std::min(bytes.size(), pos + width)}, nullptr, 0});
}
static swr_v1_buffer_view inspect(swr_v1_buffer *buffer, const Model &model) {
    uint32_t bytes = UINT32_MAX; size_t count = SIZE_MAX;
    check(swr_v1_buffer_stats(buffer, &bytes, &count) == SWR_BUFFER_OK, "stats status");
    check(bytes == byte_count(model) && count == model.size(), "model accounting/count");
    swr_v1_buffer_view view{};
    const int status = swr_v1_buffer_front(buffer, &view);
    if (model.empty()) {
        check(status == SWR_BUFFER_EMPTY && !view.data && !view.token && !view.length && !view.capacity, "empty front output");
    } else if (model.front().token) {
        check(status == SWR_BUFFER_OK && view.kind == 1 && view.token == model.front().token &&
              !view.data && !view.length && !view.capacity, "native front identity");
    } else {
        const auto &expected = model.front().bytes;
        check(status == SWR_BUFFER_OK && view.kind == 0 && !view.token && view.length == expected.size() &&
              view.capacity == expected.size() && view.data, "data front metadata");
        check(std::memcmp(view.data, expected.data(), expected.size()) == 0, "data front bytes");
    }
    return view;
}
static uint64_t abi_sequence(uint64_t seed, uint32_t chunk, size_t steps, uint64_t *recovered = nullptr) {
    active_seed = seed; active_step = 0; active_chunk = chunk;
    Random random{seed}; Model model;
    // Stable token addresses survive container growth and remain unique until release.
    std::vector<std::unique_ptr<uint64_t>> tokens;
    swr_v1_buffer *buffer = reinterpret_cast<swr_v1_buffer *>(uintptr_t(1));
    int status = swr_v1_buffer_create(chunk, &buffer);
    if (status == SWR_BUFFER_NOMEM) { check(!buffer, "failed create output"); return 1; }
    check(status == SWR_BUFFER_OK && buffer, "create");
    uint64_t failures = 0;
#ifdef MODEL_FAILURES
    bool entries_reserved = false, cache_admitted = false;
#endif
    auto pop = [&]() {
        inspect(buffer, model);
        ++pops;
        void *token = reinterpret_cast<void *>(uintptr_t(1));
        check(swr_v1_buffer_pop(buffer, &token) == (model.empty() ? SWR_BUFFER_EMPTY : SWR_BUFFER_OK), "pop status");
        check(token == (model.empty() ? nullptr : model.front().token), "pop token transfer");
        if (!model.empty()) model.pop_front();
    };
    for (active_step = 0; active_step < steps; ++active_step) {
        const auto before = inspect(buffer, model);
        const unsigned operation = random.next() % 7;
        if (operation < 3 && model.size() < 48) {
            std::vector<uint8_t> bytes(random.next() % 48);
            for (auto &byte : bytes) byte = random.next();
            std::vector<uint8_t> suffix = bytes;
            if (operation == 0) {
                ++append_calls;
                status = swr_v1_buffer_append(buffer, bytes.data(), bytes.size());
            } else {
                ++vector_calls;
                const size_t split = bytes.size() / 2;
                swr_v1_buffer_iovec vectors[] = {{nullptr, 0}, {bytes.data(), split}, {nullptr, 0},
                    {bytes.empty() ? nullptr : bytes.data() + split, bytes.size() - split}, {nullptr, 0}};
                // Includes exact descriptor boundaries and exact-total no-ops.
                const size_t offset = operation == 1 ? split : random.next() % (bytes.size() + 1);
                suffix.erase(suffix.begin(), suffix.begin() + offset);
                status = swr_v1_buffer_appendv(buffer, vectors, 5, offset);
            }
            check(status == SWR_BUFFER_OK || status == SWR_BUFFER_NOMEM, "append status");
#ifdef MODEL_FAILURES
            // Only the best-effort cache admission after payload publication can
            // recover an allocator failure. Required queue/payload storage still
            // reports NOMEM; every ordinary model assertion remains below.
            if (status == SWR_BUFFER_OK && !suffix.empty()) {
                const bool admission = entries_reserved && !cache_admitted;
                if (recovered && swr_model_failures() && failures == 0 && *recovered == 0) {
                    check(admission, "allocation failure recovered outside cache admission");
                    ++*recovered;
                } else if (admission) cache_admitted = true;
                entries_reserved = true;
            }
#endif
            if (status == SWR_BUFFER_OK) append_model(model, suffix, chunk); else ++failures;
            const auto after = inspect(buffer, model);
            if (before.data) check(after.data == before.data, "append invalidated front allocation");
        } else if (operation == 3 && model.size() < 48) {
            ++native_calls;
            tokens.emplace_back(new uint64_t(tokens.size() + 1));
            status = swr_v1_buffer_push_native(buffer, tokens.back().get());
            check(status == SWR_BUFFER_OK || status == SWR_BUFFER_NOMEM, "native push status");
            if (status == SWR_BUFFER_OK) {
                model.push_back({{}, tokens.back().get(), 0});
#ifdef MODEL_FAILURES
                entries_reserved = true;
#endif
            } else ++failures;
            if (before.data) check(inspect(buffer, model).data == before.data, "native push invalidated data view");
        } else if (operation == 4 && !model.empty() && !model.front().token) {
            // ABI offsets belong to the consumer. Mutate a consumed byte through
            // the borrowed range, retain full accounting until this chunk pops.
            ++consumes;
            auto &item = model.front();
            const uint32_t consume = 1 + random.next() % (item.bytes.size() - item.offset);
            item.bytes[item.offset] ^= 0x80;
            before.data[item.offset] ^= 0x80;
            item.offset += consume;
            if (item.offset == item.bytes.size()) pop();
        } else if (operation == 5) {
            const bool native = std::any_of(model.begin(), model.end(), [](const Item &i) { return i.token; });
            if (native) {
                ++release_refusals;
                check(swr_v1_buffer_release(buffer) == SWR_BUFFER_PENDING_NATIVE, "release lost native token");
            }
        } else pop();
        inspect(buffer, model);
    }
    // Validate every remaining entry, not just the front visible during generation.
    while (!model.empty()) pop();
    pop(); inspect(buffer, model);
    check(swr_v1_buffer_release(buffer) == SWR_BUFFER_OK, "release");
    for (size_t i = 0; i < tokens.size(); ++i)
        check(*tokens[i] == i + 1, "opaque native token payload changed");
    return failures;
}

#ifdef MODEL_FAILURES
static void cache_admission_failures() {
    for (int64_t ordinal : {0, 1}) {
        active_seed = 0; active_step = 0; active_chunk = 8; active_failure = ordinal;
        swr_model_fail_after(-1);
        swr_v1_buffer *buffer = nullptr;
        check(swr_v1_buffer_create(8, &buffer) == SWR_BUFFER_OK, "cache control create");
        Model model; const std::vector<uint8_t> bytes = {'a', 'b', 'c'};
        check(swr_v1_buffer_append(buffer, bytes.data(), bytes.size()) == SWR_BUFFER_OK, "cache control first append");
        append_model(model, bytes, 8);
        const auto before = inspect(buffer, model);
        swr_model_fail_after(ordinal);
        const int status = swr_v1_buffer_append(buffer, bytes.data(), bytes.size());
        check(swr_model_failures() == 1, "cache control allocation failure not delivered once");
        // The first allocation is required payload storage, the next is optional
        // cache admission. Failure of the latter cannot undo a published append.
        check(status == (ordinal == 0 ? SWR_BUFFER_NOMEM : SWR_BUFFER_OK), "required/optional allocation status");
        if (status == SWR_BUFFER_OK) append_model(model, bytes, 8);
        check(inspect(buffer, model).data == before.data, "cache control invalidated front allocation");
        while (!model.empty()) {
            inspect(buffer, model);
            void *token = reinterpret_cast<void *>(uintptr_t(1));
            check(swr_v1_buffer_pop(buffer, &token) == SWR_BUFFER_OK && !token, "cache control pop");
            model.pop_front();
        }
        inspect(buffer, model);
        check(swr_v1_buffer_release(buffer) == SWR_BUFFER_OK, "cache control release");
    }
    swr_model_fail_after(-1); active_failure = -1;
}
#endif

static std::deque<Item> expected_callbacks;
static uint64_t callbacks = 0;
static bool callback_error = false;
static void disposed(swoole::BufferChunk *chunk) {
    // Never throw/reenter the tested Buffer from a native destructor callback.
    if (expected_callbacks.empty()) { callback_error = true; return; }
    const auto &expected = expected_callbacks.front();
    if (expected.token) callback_error |= chunk->type != swoole::BufferChunk::TYPE_CLOSE || chunk->value.u64 != *expected.token;
    else callback_error |= chunk->type != swoole::BufferChunk::TYPE_DATA || chunk->length != expected.bytes.size() ||
                           std::memcmp(chunk->value.str, expected.bytes.data(), expected.bytes.size()) != 0;
    expected_callbacks.pop_front(); ++callbacks;
}
static void facade_sequence(uint64_t seed) {
    active_seed = seed; active_step = 0; active_chunk = 7;
    Model model; Random random{seed};
    std::vector<std::unique_ptr<uint64_t>> tokens;
    auto buffer = std::unique_ptr<swoole::Buffer>(new swoole::Buffer(7));
    for (active_step = 0; active_step < 64; ++active_step) {
        const auto op = random.next() % 4;
        if (op == 0 || model.empty()) {
            std::vector<uint8_t> bytes(1 + random.next() % 33);
            for (auto &byte : bytes) byte = random.next();
            buffer->append(reinterpret_cast<const char *>(bytes.data()), bytes.size());
            append_model(model, bytes, 7);
        } else if (op == 1) {
            tokens.emplace_back(new uint64_t(tokens.size() + 1));
            auto *chunk = buffer->alloc(swoole::BufferChunk::TYPE_CLOSE, 0);
            chunk->value.u64 = *tokens.back(); chunk->destroy = disposed;
            model.push_back({{}, tokens.back().get(), 0});
        } else {
            auto *front = buffer->front(); auto &item = model.front();
            if (!item.token) {
                check(front->length == item.bytes.size() && front->offset == item.offset, "facade partial offset");
                check(std::memcmp(front->value.str, item.bytes.data(), item.bytes.size()) == 0, "facade bytes");
                const uint32_t n = 1 + random.next() % (item.bytes.size() - item.offset);
                front->offset += n; item.offset += n;
                if (item.offset != item.bytes.size()) {
                    check(buffer->length() == byte_count(model) && buffer->count() == model.size(), "partial consume accounting");
                    continue;
                }
                front->destroy = disposed;
            }
            expected_callbacks.push_back(item);
            buffer->pop(); model.pop_front();
            check(expected_callbacks.empty() && !callback_error, "pop callback order/live bytes");
        }
        check(buffer->length() == byte_count(model) && buffer->count() == model.size(), "facade accounting/count");
    }
    // Only the first Rust DATA view can have a callback installed; later DATA
    // entries are discarded unobserved. All native tokens retain callbacks.
    if (!model.empty() && !model.front().token) {
        buffer->front()->destroy = disposed;
        expected_callbacks.push_back(model.front());
    }
    for (const auto &item : model) if (item.token) expected_callbacks.push_back(item);
    buffer.reset();
    check(expected_callbacks.empty() && !callback_error, "teardown callback order/live bytes");
}
int main() {
    try {
        const uint64_t seeds[] = {1, 7, 42, 0x12345678, 0xdeadbeef, 0x9e3779b97f4a7c15ULL};
        uint64_t replay_runs = 0, injected = 0, recoveries = 0;
        for (uint32_t chunk : {0u, 1u, 4u, 16u}) for (uint64_t seed : seeds) {
#ifdef MODEL_FAILURES
            swr_model_fail_after(-1);
#endif
            check(abi_sequence(seed, chunk, 64) == 0, "baseline allocation failure");
#ifdef MODEL_FAILURES
            const uint64_t allocations = swr_model_allocations();
            for (uint64_t failure = 0; failure < allocations; ++failure) {
                active_failure = failure;
                swr_model_fail_after(failure);
                uint64_t recovered = 0;
                const uint64_t surfaced = abi_sequence(seed, chunk, 64, &recovered);
                check(surfaced + recovered == 1 && swr_model_failures() == 1, "allocation replay did not hit once");
                recoveries += recovered;
                ++replay_runs; ++injected;
            }
            swr_model_fail_after(-1);
            active_failure = -1;
#endif
        }
#ifdef MODEL_FAILURES
        check(recoveries != 0, "cache admission allocation recovery unvisited");
        cache_admission_failures();
#endif
        for (uint64_t seed : seeds) facade_sequence(seed);
        check(append_calls && vector_calls && native_calls && consumes && pops && release_refusals, "unvisited operation category");
        std::cout << "{\"seeds\":[1,7,42,305419896,3735928559,11400714819323198485],"
                  << "\"chunk_sizes\":[0,1,4,16],\"steps_per_sequence\":64,\"baseline_sequences\":24,"
                  << "\"allocation_replay_sequences\":" << replay_runs << ",\"injected_failures\":" << injected
                  << ",\"recovered_cache_failures\":" << recoveries
                  << ",\"facade_sequences\":6,\"callbacks\":" << callbacks
                  << ",\"operations\":{\"append\":" << append_calls << ",\"appendv\":" << vector_calls
                  << ",\"native_push\":" << native_calls << ",\"partial_consume\":" << consumes
                  << ",\"pop\":" << pops << ",\"pending_native_release\":" << release_refusals
                  << "},\"passed\":true}\n";
    } catch (const std::exception &error) { std::cerr << error.what() << '\n'; return 1; }
}
