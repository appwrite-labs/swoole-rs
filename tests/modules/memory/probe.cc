#include "swoole_memory.h"
#include "swoole_heap.h"
#include "swoole_channel.h"
#include "swoole_table.h"
#include "table_compat.h"
#include <algorithm>
#include <cassert>
#include <cstring>
#include <map>
#include <deque>
#include <vector>
#include <thread>
#include <mutex>
#include <condition_variable>
#include <sys/wait.h>
#include <sys/mman.h>
#include <unistd.h>

#if defined(SWOOLE_RS_COREBASE) && defined(__linux__)
#include <dlfcn.h>
#include <link.h>
// SwooleG's Rust record is the extension's process state, filled by its swoole_init.
// This probe links its own copy of the record from the archive, and PHP loads it with
// RTLD_DEEPBIND, so the probe's copy shadows the extension's and starts empty.
// Initialise it as swoole_init does and share the extension's memory pool, as the
// pinned probe shares SwooleG's.
static void *extension_memory_pool() {
    void *pool = nullptr;
    dl_iterate_phdr(
        [](dl_phdr_info *object, size_t, void *out) {
            void *handle = object->dlpi_name[0] ? dlopen(object->dlpi_name, RTLD_LAZY | RTLD_NOLOAD) : nullptr;
            auto get = handle ? reinterpret_cast<void *(*) ()>(dlsym(handle, "swr_v1_core_memory_pool")) : nullptr;
            if (handle) {
                dlclose(handle);
            }
            if (!get || get == &swr_v1_core_memory_pool) {
                return 0;
            }
            *static_cast<void **>(out) = get();
            return 1;
        },
        &pool);
    return pool;
}

__attribute__((constructor)) static void adopt_process_state() {
    swr_v1_core_init();
    swr_v1_core_set_memory_pool(extension_memory_pool());
}
#endif

static uint64_t checksum = 1469598103934665603ULL;
static void observe(uint64_t value) { checksum = (checksum ^ value) * 1099511628211ULL; }
static uint64_t random_step(uint64_t &state) { state ^= state << 13; state ^= state >> 7; state ^= state << 17; return state; }
static void child_ok(pid_t child) { int status = 0; assert(child > 0); assert(waitpid(child, &status, 0) == child); assert(WIFEXITED(status) && WEXITSTATUS(status) == 0); }

static void fixed_pools() {
    for (bool shared : {false, true}) {
        swoole::FixedPool pool(97, 31, shared);
        assert(pool.get_slice_size() == 32 && pool.get_number_of_total_slice() == 97);
        std::vector<void *> nodes;
        for (unsigned i = 0; i < 97; ++i) { auto *p = pool.alloc(0); assert(p); memset(p, i, 32); nodes.push_back(p); }
        assert(pool.alloc(0) == nullptr && pool.get_number_of_spare_slice() == 0);
        for (unsigned i = 0; i < 97; i += 2) { assert(*static_cast<unsigned char *>(nodes[i]) == i); pool.free(nodes[i]); }
        for (int i = 96; i >= 0; i -= 2) { auto *p = pool.alloc(0); assert(p == nodes[i]); observe(i); }
        if (shared) {
            auto child = fork();
            if (child == 0) { *static_cast<unsigned char *>(nodes[0]) = 123; pool.free(nodes[1]); _exit(0); }
            child_ok(child);
            assert(*static_cast<unsigned char *>(nodes[0]) == 123);
            assert(pool.get_number_of_spare_slice() == 1);
            assert(pool.alloc(0) == nodes[1]);
        }
    }
    // Real table rows can have odd widths: preserve the packed arena layout.
    for (uint32_t width : {1, 7, 31}) {
        const size_t stride = swoole::FixedPool::sizeof_struct_slice() + width;
        const size_t total = swoole::FixedPool::sizeof_struct_impl() + 17 * stride;
        std::vector<uint64_t> arena((total + 7) / 8);
        swoole::FixedPool pool(width, arena.data(), total, false);
        std::vector<void *> pointers;
        for (unsigned i = 0; i < 17; ++i) { auto *p = pool.alloc(0); assert(p); memset(p, i, width); pointers.push_back(p); }
        for (auto *p : pointers) pool.free(p);
        assert(pool.get_number_of_spare_slice() == 17);
        for (int i = 16; i >= 0; --i) assert(pool.alloc(0) == pointers[i]);
        observe(width);
    }
}

static void heaps() {
    for (auto type : {swoole::Heap::MIN_HEAP, swoole::Heap::MAX_HEAP}) {
        swoole::Heap heap(2, type);
        std::map<uintptr_t, swoole::HeapNode *> model;
        uint64_t seed = 17;
        for (uintptr_t id = 1; id < 10000; ++id) {
            auto value = random_step(seed);
            if (value % 4 == 0) { auto *node = heap.push(value % 100, reinterpret_cast<void *>(id)); assert(node); model[id] = node; }
            else if (value % 4 == 1 && !model.empty()) {
                auto it = model.begin(); std::advance(it, value % model.size());
                heap.remove(it->second); model.erase(it);
            } else if (value % 4 == 2 && !model.empty()) {
                auto it = model.begin(); std::advance(it, value % model.size());
                heap.change_priority(value % 100, it->second);
            } else {
                auto *top = heap.top();
                if (!model.empty()) {
                    assert(top);
                    uint64_t priority = top->priority;
                    for (const auto &kv : model) assert(type == swoole::Heap::MIN_HEAP ? priority <= kv.second->priority : priority >= kv.second->priority);
                    auto token = reinterpret_cast<uintptr_t>(top->data);
                    assert(heap.pop() == reinterpret_cast<void *>(token)); model.erase(token); observe(token); observe(priority);
                } else assert(heap.pop() == nullptr);
            }
            assert(heap.count() == model.size());
        }
        while (heap.count()) { auto *top = heap.top(); observe(top->priority); observe(reinterpret_cast<uintptr_t>(heap.pop())); }
    }
    swoole::Heap equal(3, swoole::Heap::MIN_HEAP);
    for (uintptr_t id = 1; id <= 3; ++id) equal.push(1, reinterpret_cast<void *>(id));
    for (uintptr_t id : {1, 3, 2}) assert(equal.pop() == reinterpret_cast<void *>(id));
}

static void mapping_growth() {
    auto *data = static_cast<unsigned char *>(sw_shm_malloc(1200)); assert(data);
    for (unsigned i = 0; i < 1200; ++i) data[i] = i % 251;
    auto *grown = static_cast<unsigned char *>(sw_shm_realloc(data, 4096)); assert(grown);
    for (unsigned i = 0; i < 1200; ++i) assert(grown[i] == i % 251);
    for (unsigned i = 1200; i < 4096; ++i) assert(grown[i] == 0);
    assert(sw_shm_protect(grown, PROT_READ) == 0);
    for (unsigned i = 0; i < 1200; ++i) assert(grown[i] == i % 251);
    assert(sw_shm_protect(grown, PROT_READ | PROT_WRITE) == 0);
    grown[0] = 0x5a; assert(grown[0] == 0x5a);
    sw_shm_free(grown); observe(4096);
    auto *zero = static_cast<unsigned char *>(sw_shm_calloc(17, 31)); assert(zero);
    for (unsigned i = 0; i < 17 * 31; ++i) assert(zero[i] == 0);
    sw_shm_free(zero); observe(17 * 31);
}

static void ring_pools() {
    for (bool shared : {false, true}) {
        swoole::RingBuffer pool(65536, shared);
        struct Entry { unsigned char *data; uint32_t size; uint8_t pattern; };
        std::vector<Entry> active;
        uint64_t seed = 51;
        uintptr_t base = 0;
        for (uint32_t step = 0; step < 30000; ++step) {
            auto value = random_step(seed);
            if (value % 3 != 0) {
                uint32_t size = value % 1024 + 1;
                auto *data = static_cast<unsigned char *>(pool.alloc(size));
                if (data) {
                    if (!base) base = reinterpret_cast<uintptr_t>(data);
                    observe(reinterpret_cast<uintptr_t>(data) - base);
                    memset(data, step % 251, size); active.push_back({data, size, uint8_t(step % 251)});
                } else observe(0xffff);
            } else if (!active.empty()) {
                size_t index = value % active.size(); auto entry = active[index];
                for (uint32_t i = 0; i < entry.size; ++i) assert(entry.data[i] == entry.pattern);
                pool.free(entry.data); active.erase(active.begin() + index);
            }
        }
        for (auto entry : active) { for (uint32_t i = 0; i < entry.size; ++i) assert(entry.data[i] == entry.pattern); pool.free(entry.data); }
        if (shared) {
            auto *data = static_cast<unsigned char *>(pool.alloc(32));
            while (!data) data = static_cast<unsigned char *>(pool.alloc(32));
            auto child = fork();
            if (child == 0) { data[0] = 231; pool.free(data); _exit(0); }
            child_ok(child); assert(data[0] == 231);
            assert(pool.alloc(32));
        }
    }
    swoole::RingBuffer pool(65536, true);
    struct Entry { void *data; uint32_t size, id; };
    struct Queue { std::mutex mutex; std::condition_variable changed; std::deque<Entry> entries; } queues[4];
    std::thread readers[4];
    for (unsigned slot = 0; slot < 4; ++slot) readers[slot] = std::thread([&, slot] {
        for (unsigned i = 0; i < 1250; ++i) {
            Entry entry;
            { std::unique_lock<std::mutex> lock(queues[slot].mutex); queues[slot].changed.wait(lock, [&]{return !queues[slot].entries.empty();}); entry=queues[slot].entries.front(); queues[slot].entries.pop_front(); }
            auto *bytes = static_cast<unsigned char *>(entry.data);
            for (unsigned n = 0; n < entry.size; ++n) assert(bytes[n] == entry.id % 251);
            pool.free(entry.data);
        }
    });
    for (uint32_t id = 0; id < 5000; ++id) {
        uint32_t size = id % 257 + 1; void *data;
        while (!(data = pool.alloc(size))) std::this_thread::yield();
        memset(data, id % 251, size);
        auto &queue = queues[id % 4];
        { std::lock_guard<std::mutex> lock(queue.mutex); queue.entries.push_back({data,size,id}); }
        queue.changed.notify_one();
    }
    for (auto &reader : readers) reader.join();
}

static void global_pools() {
    for (bool shared : {false, true}) {
        swoole::GlobalMemory pool(4096, shared);
        const size_t page_size = pool.get_memory_size();
        std::vector<std::pair<unsigned char *, uint32_t>> blocks;
        for (uint32_t i = 0; i < 2000; ++i) {
            const uint32_t size = (i * 13) % 257;
            auto *data = static_cast<unsigned char *>(pool.alloc(size)); assert(data);
            for (uint32_t n = 0; n < size; ++n) assert(data[n] == 0);
            memset(data, i % 251, size); pool.free(data); blocks.emplace_back(data, size);
            observe(pool.capacity()); observe(pool.get_memory_size());
        }
        for (uint32_t i = 0; i < blocks.size(); ++i)
            for (uint32_t n = 0; n < blocks[i].second; ++n) assert(blocks[i].first[n] == i % 251);
        if (shared) {
            auto *inherited = blocks[1].first;
            size_t before_size = pool.get_memory_size(), before_capacity = pool.capacity();
            auto child = fork();
            if (child == 0) {
                inherited[0] = 233;
                auto *fresh = static_cast<unsigned char *>(pool.alloc(16)); assert(fresh);
                assert(pool.get_memory_size() == page_size && pool.capacity() == page_size - 24);
                assert(inherited[0] == 233); _exit(0);
            }
            child_ok(child); assert(inherited[0] == 233);
            assert(pool.get_memory_size() == before_size && pool.capacity() == before_capacity);
        }
    }
}

static void channels() {
    // Public full() reads only scalar state, including defined unsigned wrap.
    for (int num : {0, 1, -1, INT32_MIN, INT32_MAX}) {
        for (size_t bytes : {size_t(0), size_t(4), SIZE_MAX - 3, SIZE_MAX}) {
            for (size_t size : {size_t(0), size_t(4), size_t(128), SIZE_MAX}) {
                for (int tags = 0; tags < 4; ++tags) {
                    swoole::Channel scalar{};
                    scalar.head = tags & 1; scalar.tail = (tags >> 1) & 1;
                    scalar.head_tag = tags & 1; scalar.tail_tag = (tags >> 1) & 1;
                    scalar.num = num; scalar.bytes = bytes; scalar.size = size;
                    const bool expected = (scalar.head == scalar.tail && scalar.tail_tag != scalar.head_tag) ||
                                          bytes + sizeof(int) * num == size;
                    assert(scalar.full() == expected);
                }
            }
        }
    }

    // All defined lock/notification/mapping combinations use legitimate records.
    for (int flags = 0; flags <= 14; flags += 2) {
        auto *lifecycle = swoole::Channel::make(128, 32, flags); assert(lifecycle);
        assert(lifecycle->in("life", 4) == SW_OK);
        char bytes[32]{};
        assert(lifecycle->out(bytes, sizeof(bytes)) == 4 && memcmp(bytes, "life", 4) == 0);
        if (flags & swoole::SW_CHAN_NOTIFY) assert(lifecycle->notify() == SW_OK && lifecycle->wait() == SW_OK);
        lifecycle->destroy();
    }

    // Opaque lock/pipe effects remain native; initialization publishes them
    // only after Rust has finished with the fresh actual storage.
    auto *notification = swoole::Channel::make(128, 32, swoole::SW_CHAN_LOCK | swoole::SW_CHAN_NOTIFY);
    assert(notification && notification->notify() == SW_OK && notification->wait() == SW_OK);
    notification->destroy();
    auto *readonly = swoole::Channel::make(128, 32, swoole::SW_CHAN_LOCK | swoole::SW_CHAN_SHM);
    assert(readonly && readonly->push("const-peek", 10) == SW_OK);
    assert(sw_shm_protect(readonly, PROT_READ) == 0);
    char peeked[32]{};
    const swoole::Channel *constant = readonly;
    assert(constant->peek(peeked, sizeof(peeked)) == 10 && memcmp(peeked, "const-peek", 10) == 0);
    assert(sw_shm_protect(readonly, PROT_READ | PROT_WRITE) == 0);
    assert(readonly->pop(peeked, sizeof(peeked)) == 10);
    readonly->destroy();

    for (int flags : {int(swoole::SW_CHAN_LOCK), int(swoole::SW_CHAN_LOCK | swoole::SW_CHAN_SHM)}) {
        auto *channel = swoole::Channel::make(128, 24, flags); assert(channel);
        std::deque<std::vector<char>> model;
        uint64_t seed = 42;
        for (int i = 0; i < 20000; ++i) {
            auto value = random_step(seed);
            if (value % 4 != 0) {
                std::vector<char> input(value % 25, value);
                if (channel->push(input.data(), input.size()) == SW_OK) model.push_back(input);
            } else {
                char output[24]{};
                int length = channel->peek(output, sizeof(output));
                if (model.empty()) assert(length == SW_ERR);
                else {
                    assert(length == int(model.front().size())); assert(memcmp(output, model.front().data(), length) == 0);
                    assert(channel->pop(output, sizeof(output)) == length); observe(length); model.pop_front();
                }
            }
            size_t bytes = 0; for (auto &item : model) bytes += item.size();
            assert(channel->count() == int(model.size()) && channel->get_bytes() == bytes);
        }
        channel->destroy();
    }
    auto *channel = swoole::Channel::make(4096, 32, swoole::SW_CHAN_LOCK | swoole::SW_CHAN_SHM);
    auto child = fork();
    if (child == 0) { assert(channel->push("fork-visible", 12) == SW_OK); _exit(0); }
    child_ok(child); char output[32]{};
    assert(channel->pop(output, sizeof(output)) == 12 && memcmp(output, "fork-visible", 12) == 0);
    channel->destroy();
    channel = swoole::Channel::make(4096, 32, swoole::SW_CHAN_LOCK);
    std::thread writer([&] { for (uint64_t i = 0; i < 5000; ++i) while (channel->push(&i, sizeof(i)) != SW_OK) std::this_thread::yield(); });
    for (uint64_t i = 0; i < 5000; ++i) { uint64_t data; while (channel->pop(&data, sizeof(data)) < 0) std::this_thread::yield(); assert(data == i); }
    writer.join(); channel->destroy();

    // Exercise every short-copy threshold and the general-copy fallback through
    // the real shared native facade. Unequal records repeatedly spill/wrap while
    // an independent process produces data; zero-length is a real queue item.
    channel = swoole::Channel::make(8192, 1024, swoole::SW_CHAN_LOCK | swoole::SW_CHAN_SHM);
    assert(channel);
    child = fork();
    if (child == 0) {
        unsigned char input[1024];
        for (unsigned length = 0; length <= sizeof(input); ++length) {
            for (unsigned i = 0; i < length; ++i) input[i] = (i * 37 + length) & 255;
            while (channel->push(input, length) != SW_OK) std::this_thread::yield();
        }
        _exit(0);
    }
    assert(child > 0);
    for (unsigned expected = 0; expected <= 1024; ++expected) {
        unsigned char peeked[1026], consumed[1026];
        memset(peeked, 0xcd, sizeof(peeked)); memset(consumed, 0xab, sizeof(consumed));
        int length;
        while ((length = channel->peek(peeked + 1, 1024)) < 0) std::this_thread::yield();
        assert(length == int(expected));
        assert(channel->pop(consumed + 1, 1024) == length);
        assert(peeked[0] == 0xcd && consumed[0] == 0xab);
        for (unsigned i = 0; i < expected; ++i) {
            auto byte = static_cast<unsigned char>((i * 37 + expected) & 255);
            assert(peeked[i + 1] == byte && consumed[i + 1] == byte);
            observe(byte);
        }
        for (unsigned i = expected + 1; i < sizeof(peeked); ++i) {
            assert(peeked[i] == 0xcd && consumed[i] == 0xab);
        }
        observe(expected);
    }
    child_ok(child);
    assert(channel->count() == 0 && channel->get_bytes() == 0);
    channel->destroy();
}

static uint64_t collision_hash(const char *, size_t) { return 0; }
static void tables() {
    for (bool force_collisions : {false, true}) {
        auto *table = swoole::Table::make(65, 0.2f); assert(table && table->get_size() == 128);
        assert(table->add_column("integer", swoole::TableColumn::TYPE_INT, 0));
        assert(table->add_column("floating", swoole::TableColumn::TYPE_FLOAT, 0));
        assert(table->add_column("binary", swoole::TableColumn::TYPE_STRING, 12));
        auto *integer = table->get_column("integer");
        auto *floating = table->get_column("floating");
        auto *binary = table->get_column("binary");
        assert(integer && floating && binary && binary->size == 16);
        assert(table->add_column("integer", swoole::TableColumn::TYPE_INT, 0));
        std::string binary_name("field\0name", 10);
        assert(table->add_column(binary_name, swoole::TableColumn::TYPE_STRING, 4));
        assert(table->get_column("integer") == integer && table->get_column_size() == 4 && column_list_size(table) == 5);
        assert(table->get_column(binary_name) && !table->get_column("field"));
        if (force_collisions) table->set_hash_func(collision_hash);
        assert(table->create()); assert(!table->create());
        observe(table->get_memory_size()); observe(table->get_total_slice_num());
        std::map<std::string, long> model;
        uint64_t seed = 713;
        for (int step = 0; step < 10000; ++step) {
            auto random = random_step(seed);
            std::string key = "k" + std::to_string(random % 20);
            if (step % 3 == 0) {
                assert(table->del(key.data(), key.size()) == bool(model.erase(key)));
            } else {
                bool fresh = model.find(key) == model.end();
                swoole::TableRow *lock = nullptr;
                int flags = -1;
                auto *row = table->set(key.data(), key.size(), &lock, &flags); assert(row && lock);
                assert(bool(flags & swoole::SW_TABLE_FLAG_NEW_ROW) == fresh);
                long value = long(random % 999999);
                double number = double(value) / 8;
                char payload[12]; for (int i = 0; i < 12; ++i) payload[i] = char(value + i);
                row->set_value(integer, &value, 0); row->set_value(floating, &number, 0);
                row->set_value(binary, payload, sizeof(payload));
                long returned = 0; double returned_double = 0; char *string = nullptr; uint32_t length = 0;
                row->get_value(integer, &returned); row->get_value(floating, &returned_double); row->get_value(binary, &string, &length);
                assert(returned == value && returned_double == number && length == 12 && memcmp(string, payload, 12) == 0);
                observe(flags); lock->unlock(); model[key] = value;
            }
            assert(table->count() == model.size());
            if (step % 101 == 0) {
                std::map<std::string, long> seen; table->rewind();
                for (table->forward(); table->current()->active; table->forward()) {
                    auto *row = table->current(); long value; row->get_value(integer, &value);
                    assert(seen.emplace(std::string(row->key, row->key_len), value).second);
                }
                assert(seen == model);
            }
        }
        for (auto &entry : model) {
            swoole::TableRow *lock = nullptr;
            auto *row = table->get(entry.first.data(), entry.first.size(), &lock); assert(row);
            long value = 0; row->get_value(integer, &value); assert(value == entry.second); observe(value); lock->unlock();
        }
        // Root promotion, failure locking, and collision counter state are observable.
        const auto stats = table_stats(table);
        observe(stats.conflict_count); observe(stats.conflict_max_level);
        observe(stats.insert_count); observe(stats.delete_count); observe(stats.update_count);
        for (auto &entry : model) assert(table->del(entry.first.data(), entry.first.size()));
        assert(table->count() == 0 && table->get_available_slice_num() == table->get_total_slice_num());
        const char key[] = {'x', 0, 'y'};
        auto child = fork();
        if (child == 0) {
            swoole::TableRow *lock = nullptr; auto *row = table->set(key, sizeof(key), &lock, nullptr); assert(row);
            long value = 519; row->set_value(integer, &value, 0); lock->unlock(); _exit(0);
        }
        child_ok(child); swoole::TableRow *lock = nullptr;
        auto *row = table->get(key, sizeof(key), &lock); assert(row); long value = 0; row->get_value(integer, &value);
        assert(value == 519 && table->count() == 1); lock->unlock();
        assert(table->del(key, sizeof(key)));
        if (force_collisions) {
            // Exhaustion preserves out_flags and still returns the root lock.
            size_t capacity = table->get_total_slice_num() + 1;
            for (size_t i = 0; i < capacity; ++i) {
                std::string key = "full" + std::to_string(i);
                row = table->set(key.data(), key.size(), &lock, nullptr); assert(row); lock->unlock();
            }
            int sentinel = 717;
            row = table->set("exhausted", 9, &lock, &sentinel); assert(!row && lock && sentinel == 717); lock->unlock();
            assert(table->count() == capacity && table->get_available_slice_num() == 0);
            for (size_t i = 0; i < capacity; ++i) { std::string key = "full" + std::to_string(i); assert(table->del(key.data(), key.size())); }
        }
        std::string long_key(100, 'q');
        row = table->set(long_key.data(), long_key.size(), &lock, nullptr); assert(row && row->key_len == 63);
        table->clear_row(row);
        long cleared = 91; row->get_value(integer, &cleared); assert(cleared == 0);
        char *empty = nullptr; uint32_t length = 1; row->get_value(binary, &empty, &length); assert(length == 0);
        lock->unlock(); assert(table->exists(long_key)); assert(table->del(long_key.data(), long_key.size()));
        // Contended read-modify-write uses the owning native root lock.
        row = table->set("shared", 6, &lock, nullptr); assert(row); long zero = 0; row->set_value(integer, &zero, 0); lock->unlock();
        std::vector<std::thread> workers;
        for (int worker = 0; worker < 4; ++worker) workers.emplace_back([&] {
            for (int i = 0; i < 1000; ++i) {
                swoole::TableRow *held = nullptr; auto *entry = table->get("shared", 6, &held); assert(entry);
                long value = 0; entry->get_value(integer, &value); value++; entry->set_value(integer, &value, 0); held->unlock();
            }
        });
        for (auto &worker : workers) worker.join();
        row = table->get("shared", 6, &lock); assert(row); long total = 0; row->get_value(integer, &total); assert(total == 4000); observe(total); lock->unlock();
        assert(table->del("shared", 6)); table->destroy();
    }
}

template <typename Function>
static std::string capture_diagnostic(Function function) {
    fflush(stdout);
    FILE *file = tmpfile(); assert(file);
    const int saved = dup(STDOUT_FILENO); assert(saved >= 0);
    assert(dup2(fileno(file), STDOUT_FILENO) >= 0);
    function();
    fflush(stdout);
    assert(dup2(saved, STDOUT_FILENO) >= 0); close(saved);
    rewind(file);
    std::string text;
    char buffer[256]; size_t length;
    while ((length = fread(buffer, 1, sizeof(buffer), file))) text.append(buffer, length);
    assert(!ferror(file)); fclose(file);
    return text;
}

static void collection_diagnostics() {
    for (bool shared : {false, true}) {
        swoole::FixedPool pool(5, 8, shared);
        auto *allocated = pool.alloc(8); assert(allocated);
        for (int limit : {-2, -1, 0, 1, 3, 99}) {
            const auto text = capture_diagnostic([&] { pool.debug(limit); });
            const std::string header = "===============================debug=================================\n";
            assert(text.compare(0, header.size(), header) == 0);
            const int expected = limit < 0 ? 1 : std::min(5, limit + 2);
            size_t offset = header.size();
            for (int line = 0; line < expected; ++line) {
                const auto prefix = "#" + std::to_string(line) + "\tslice[";
                assert(text.compare(offset, prefix.size(), prefix) == 0);
                offset = text.find('\n', offset); assert(offset != std::string::npos); ++offset;
            }
            assert(offset == text.size());
        }
        pool.free(allocated);
    }
    for (auto type : {swoole::Heap::MIN_HEAP, swoole::Heap::MAX_HEAP}) {
        swoole::Heap heap(4, type);
        assert(capture_diagnostic([&] { heap.print(); }).empty());
        assert(heap.peek() == nullptr);
        int tokens[4] = {0, 1, 2, 3};
        const uint64_t priorities[4] = {9, 3, 5, 3};
        swoole::HeapNode *nodes[4];
        for (int i = 0; i < 4; ++i) { nodes[i] = heap.push(priorities[i], &tokens[i]); assert(nodes[i]); }
        std::sort(nodes, nodes + 4, [](const swoole::HeapNode *a, const swoole::HeapNode *b) { return a->position < b->position; });
        std::string expected;
        for (const auto *node : nodes) {
            char line[160]; const int length = snprintf(line, sizeof(line), "#%u\tpriority=%ld, data=%p\n", node->position, (long) node->priority, node->data);
            assert(length > 0 && static_cast<size_t>(length) < sizeof(line)); expected.append(line, length);
        }
        assert(capture_diagnostic([&] { heap.print(); }) == expected);
        assert(heap.peek() == heap.top()->data);
    }
}

static size_t lookup_callback_calls;
static const char *lookup_callback_key;
static uint64_t lookup_callback(const char *key, size_t length) {
    assert(key == lookup_callback_key && length == 63);
    ++lookup_callback_calls;
    return 63;
}
static void lookup_callback_boundary() {
    auto *table = swoole::Table::make(64, 0.5); assert(table);
    assert(table->add_column("integer", swoole::TableColumn::TYPE_INT, 0));
    assert(table->create());
    const std::string key(100, 'l');
    lookup_callback_key = key.data(); lookup_callback_calls = 0;
    table->set_hash_func(lookup_callback);
    swoole::TableRow *lock = nullptr;
    auto *row = table->set(key.data(), key.size(), &lock, nullptr); assert(row && row->key_len == 63);
    assert(row == table->get_by_index(63)); lock->unlock();
    row = table->get(key.data(), key.size(), &lock); assert(row && row->key_len == 63); lock->unlock();
    assert(table->exists(key));
    assert(table->del(key.data(), key.size()));
    assert(!table->del(key.data(), key.size()));
    assert(!table->exists(key));
    assert(lookup_callback_calls == 6 && table->count() == 0);
    table->destroy();
}

static void clear_batch_boundaries() {
    for (unsigned count : {1u, 7u, 8u, 9u, 17u}) {
        auto *table = swoole::Table::make(64, 0.5); assert(table);
        for (unsigned i = 0; i < count; ++i) {
            assert(table->add_column("field" + std::to_string(i), i % 2 ? swoole::TableColumn::TYPE_STRING : swoole::TableColumn::TYPE_INT, 4));
        }
        assert(table->create());
        swoole::TableRow *lock = nullptr;
        auto *row = table->set("batch", 5, &lock, nullptr); assert(row && lock);
        unsigned char header[sizeof(swoole::TableRow)]; memcpy(header, row, sizeof(header));
        memset(row->data, 0x5a, count * 8);
        table->clear_row(row);
        assert(memcmp(header, row, sizeof(header)) == 0);
        for (unsigned i = 0; i < count; ++i) {
            const unsigned zeroes = i % 2 ? 4 : 8;
            for (unsigned j = 0; j < 8; ++j) {
                assert(static_cast<unsigned char>(row->data[i * 8 + j]) == (j < zeroes ? 0 : 0x5a));
            }
        }
        lock->unlock(); assert(table->del("batch", 5)); table->destroy();
    }
}

static void value_write_boundaries() {
    auto *table = swoole::Table::make(64, 0.5); assert(table);
    assert(table->add_column("value", swoole::TableColumn::TYPE_STRING, 4));
    assert(table->create()); auto *column = table->get_column("value"); assert(column);
    swoole::TableRow *lock = nullptr;
    auto *row = table->set("value", 5, &lock, nullptr); assert(row && lock);
    const unsigned char payload[16] = {0, 1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15};
    for (size_t length : {size_t(0), size_t(1), size_t(4), size_t(5), size_t(16)}) {
        memset(row->data, 0x5a, 8);
        row->set_value(column, payload, length);
        char *actual = nullptr; uint32_t stored = 99;
        row->get_value(column, &actual, &stored);
        assert(stored == std::min(size_t(4), length));
        assert(memcmp(actual, payload, stored) == 0);
        for (unsigned i = stored; i < 4; ++i) assert(static_cast<unsigned char>(actual[i]) == 0x5a);
    }
    row->set_value(column, nullptr, 0);
    char *actual = nullptr; uint32_t stored = 99;
    row->get_value(column, &actual, &stored); assert(stored == 0);
    lock->unlock(); assert(table->del("value", 5)); table->destroy();
}

using TestColumn = const swoole::TableColumn *;
static bool column_valid(TestColumn c) { return c != nullptr; }
static size_t column_index(TestColumn c) { return c->index; }
static uint32_t column_width(TestColumn c) { return c->size; }
static uint32_t column_kind(TestColumn c) { return c->type; }
static void column_get(swoole::TableRow *row, TestColumn c, long *value) { row->get_value(c, value); }
static void column_set(swoole::TableRow *row, TestColumn c, const long *value) { row->set_value(c, value, 0); }

static void versioned_columns() {
    {
        auto *table = swoole::Table::make(64, 0.5); assert(table);
        const std::string binary_name("field\0tail", 10);
        assert(table->add_column("integer", swoole::TableColumn::TYPE_INT, 0));
        assert(table->add_column("floating", swoole::TableColumn::TYPE_FLOAT, 0));
        assert(table->add_column(binary_name, swoole::TableColumn::TYPE_STRING, 12));
        const auto integer = table->get_column("integer");
        const auto floating = table->get_column("floating");
        const auto binary = table->get_column(binary_name);
        assert(column_valid(integer) && column_valid(floating) && column_valid(binary));
        assert(column_index(integer) == 0 && column_index(floating) == 8 && column_index(binary) == 16);
        assert(column_width(binary) == 16 && column_name(binary) == binary_name);
        assert(!table->get_column("field"));
        assert(table->create());
        swoole::TableRow *lock = nullptr;
        auto *row = table->set("mixed", 5, &lock, nullptr); assert(row && lock);
        table->clear_row(row);
        long ivalue = 817, iread = 0;
        double dvalue = 12.5, dread = 0;
        const std::string value("hello\0world!", 12);
        char *bytes = nullptr; swoole::TableStringLength length = 0;
        row->set_value(integer, &ivalue, 0); row->get_value(integer, &iread);
        row->set_value(floating, &dvalue, 0); row->get_value(floating, &dread);
        row->set_value(binary, value.data(), value.size()); row->get_value(binary, &bytes, &length);
        assert(iread == ivalue && dread == dvalue && length == value.size());
        assert(std::string(bytes, length) == value);
        table->clear_row(row);
        row->get_value(integer, &iread); row->get_value(floating, &dread); row->get_value(binary, &bytes, &length);
        assert(iread == 0 && dread == 0 && length == 0 && std::string(bytes, value.size()) == value);
        lock->unlock(); table->destroy();
    }
    for (unsigned count : {1u, 3u, 17u, 65u}) {
        auto *table = swoole::Table::make(64, 0.5); assert(table);
        for (unsigned i = 0; i < count; ++i) {
            assert(table->add_column("column" + std::to_string(i), swoole::TableColumn::TYPE_INT, 0));
        }
        assert(table->add_column("column0", swoole::TableColumn::TYPE_INT, 0));
        const auto first = table->get_column("column0"); assert(column_valid(first) && column_index(first) == 0);
        assert(table->get_column_size() == count);
        assert(column_list_size(table) == count + 1);
        for (unsigned i = 0; i <= count; ++i) {
            const auto *column = column_list_at(table, i); assert(column->index == i * 8);
            assert(column_name(column) == "column" + std::to_string(i == count ? 0 : i));
        }
        assert(table->create());
#ifdef SWOOLE_RS_MEMORY
        // Rust refuses a column once rows exist; upstream would corrupt them.
        assert(!table->add_column("after-create", swoole::TableColumn::TYPE_INT, 0));
#endif
        swoole::TableRow *lock = nullptr;
        auto *row = table->set("owned", 5, &lock, nullptr); assert(row && lock);
        memset(row->data, 0x5a, (count + 1) * 8);
        table->clear_row(row);
        for (unsigned i = 0; i < (count + 1) * 8; ++i) assert(row->data[i] == 0);
        long value = 713; column_set(row, first, &value);
        value = 0; column_get(row, first, &value); assert(value == 713);
        lock->unlock();
        assert(table->count() == 1 && table_stats(table).insert_count == 1);
        std::vector<std::thread> readers;
        for (unsigned worker = 0; worker < 4; ++worker) readers.emplace_back([&] {
            for (unsigned step = 0; step < 128; ++step) {
                assert(table->get_column("column0") == first);
                assert(column_list_size(table) == count + 1);
                for (unsigned i = 0; i <= count; ++i) {
                    const auto *column = column_list_at(table, i); assert(column && column->index == i * 8);
                    assert(column_width(column) == 8 && column_kind(column) == swoole::TableColumn::TYPE_INT);
                }
                swoole::TableRow *held = nullptr;
                auto *entry = table->get("owned", 5, &held); assert(entry && held);
                long current = 0; column_get(entry, first, &current); ++current;
                column_set(entry, first, &current); held->unlock();
            }
        });
        for (auto &reader : readers) reader.join();
        row = table->get("owned", 5, &lock); assert(row && lock);
        value = 0; column_get(row, first, &value); assert(value == 713 + 512); lock->unlock();
        // Columns are process-private, copied on fork; only the locked row is
        // shared. The child never changes the schema or destroys the table.
        const auto child = fork();
        if (child == 0) {
            const auto inherited = table->get_column("column0");
            assert(column_valid(inherited) && column_index(inherited) == 0);
            assert(column_name(inherited) == "column0");
            assert(column_list_size(table) == count + 1);
            unsigned seen = 0;
            for (size_t i = 0; i < column_list_size(table); ++i) {
                const auto *column = column_list_at(table, i);
                assert(column && column->index == seen * 8);
                assert(column_name(column) == "column" + std::to_string(seen == count ? 0 : seen));
                ++seen;
            }
            assert(seen == count + 1);
            swoole::TableRow *held = nullptr;
            auto *entry = table->get("owned", 5, &held); assert(entry && held);
            long changed = 0; column_get(entry, inherited, &changed);
            changed += 17; column_set(entry, inherited, &changed); held->unlock();
            _exit(0);
        }
        child_ok(child);
        row = table->get("owned", 5, &lock); assert(row && lock);
        value = 0; column_get(row, first, &value); assert(value == 713 + 512 + 17); lock->unlock();
        table->rewind(); table->forward(); assert(table->current()->active);
        table->forward(); assert(!table->current()->active);
        assert(table->exists("owned", 5) && table->del("owned", 5));
        assert(table->count() == 0 && table_stats(table).delete_count == 1);
        table->destroy();
    }
    auto *uncreated = swoole::Table::make(64, 0.5); assert(uncreated);
    assert(uncreated->add_column("uncreated", swoole::TableColumn::TYPE_INT, 0));
    uncreated->destroy();
}

extern "C" __attribute__((visibility("default"))) int swr_memory_probe() {
    try {
        fixed_pools(); heaps(); channels(); global_pools(); ring_pools(); mapping_growth(); tables(); collection_diagnostics(); lookup_callback_boundary(); clear_batch_boundaries(); value_write_boundaries(); versioned_columns();
        printf("{\"passed\":true,\"checksum\":%llu,\"fixed_widths\":5,\"heap_steps\":19998,\"channel_steps\":40000,\"channel_variable_fork_records\":1025,\"thread_messages\":5000,\"fork_cases\":7,\"global_allocations\":4000,\"ring_steps\":60000,\"ring_thread_messages\":5000,\"table_steps\":20000,\"table_contended_updates\":8000,\"fixed_diagnostic_cases\":12,\"heap_diagnostic_cases\":4,\"native_hash_callback_cases\":6,\"native_clear_batch_schemas\":5,\"native_value_boundary_cases\":6,\"versioned_column_schemas\":4,\"uncreated_column_teardown\":1,\"versioned_column_reader_updates\":2048,\"versioned_column_fork_cases\":4}\n", (unsigned long long) checksum);
        return 0;
    } catch (const std::exception &error) { fprintf(stderr, "%s\n", error.what()); return 99; }
}


// Separate Rust-only witness for shrinking realloc, which is not compared.
extern "C" __attribute__((visibility("default"))) int swr_memory_mapping_probe() {
    for (size_t original : {size_t(32768), size_t(65536), size_t(262144)}) {
        auto *data = static_cast<unsigned char *>(sw_shm_malloc(original)); assert(data);
        memset(data, 97, original);
        auto *shrunk = static_cast<unsigned char *>(sw_shm_realloc(data, 1)); assert(shrunk && shrunk[0] == 97);
        sw_shm_free(shrunk);
    }
    assert(sw_shm_malloc(SIZE_MAX) == nullptr);
    assert(sw_shm_calloc(SIZE_MAX, 2) == nullptr);
    printf("{\"passed\":true,\"shrink_cases\":3,\"overflow_rejected\":true,\"scope\":\"Rust facade only; unsafe upstream shrinking not run\"}\n");
    return 0;
}


#ifdef SWOOLE_RS_MEMORY
// Rust-only safety witness: the retained original odd-row metadata receipt
// faults on ARM64 CAS. Do not run/bless that unsafe baseline as passing.
extern "C" __attribute__((visibility("default"))) int swr_memory_odd_table_probe() {
    using swoole::Table;
    using swoole::TableColumn;
    using swoole::TableRow;
    static_assert(sizeof(TableRow) == 88 && alignof(TableRow) == 8, "frozen native row ABI");
    for (unsigned width = 0; width <= 17; ++width) {
        auto *table = Table::make(64, 1.0); assert(table);
        assert(table->add_column("integer", TableColumn::TYPE_INT, 0));
        assert(table->add_column("binary", TableColumn::TYPE_STRING, width));
        auto *integer = table->get_column("integer");
        auto *binary = table->get_column("binary");
        assert(integer && binary && binary->size == width + 4);
        // Root index1 is itself unaligned with the old odd record stride,
        // reproducing the retained ARM64 CAS failure rather than only root0.
        table->set_hash_func([](const char *, size_t) -> uint64_t { return 1; });
        // Upstream's accounting: 128 unpadded rows, 64 pool slices, 64 buckets.
        const size_t row_size = sizeof(TableRow) + 12 + width;
        const size_t pool = swoole::FixedPool::sizeof_struct_impl() + 64 * (row_size + swoole::FixedPool::sizeof_struct_slice());
        assert(table->calc_memory_size() == 64 * sizeof(TableRow *) + 64 * row_size + pool);
        assert(table->create() && table->get_memory_size() == pool);
        assert(table->get_total_slice_num() == 64);
        for (unsigned id = 0; id < 16; ++id) {
            char key[16], payload[17];
            int length = snprintf(key, sizeof(key), "k%u", id);
            for (unsigned i = 0; i < width; ++i) payload[i] = char(i * 37 + id);
            TableRow *lock = nullptr;
            auto *row = table->set(key, length, &lock, nullptr); assert(row && lock);
            assert(reinterpret_cast<uintptr_t>(row) % alignof(TableRow) == 0);
            assert(reinterpret_cast<uintptr_t>(lock) % alignof(TableRow) == 0);
            table->clear_row(row);
            long value = id; row->set_value(integer, &value, 0);
            row->set_value(binary, payload, width);
            lock->unlock();
        }
        unsigned seen = 0;
        table->rewind();
        for (table->forward(); table->current()->active; table->forward()) {
            auto *row = table->current(); long value = -1;
            assert(reinterpret_cast<uintptr_t>(row) % alignof(TableRow) == 0);
            row->get_value(integer, &value); assert(value >= 0 && value < 16);
            char *payload = nullptr; uint32_t length = 99;
            row->get_value(binary, &payload, &length); assert(length == width);
            for (unsigned i = 0; i < width; ++i) assert(payload[i] == char(i * 37 + value));
            ++seen;
        }
        assert(seen == 16 && table->count() == 16);
        auto increment = [&] {
            for (unsigned i = 0; i < 128; ++i) {
                TableRow *lock = nullptr; auto *row = table->get("k0", 2, &lock); assert(row && lock);
                long value = 0; row->get_value(integer, &value); ++value;
                row->set_value(integer, &value, 0); lock->unlock();
            }
        };
        auto child = fork();
        if (child == 0) { increment(); _exit(0); }
        assert(child > 0);
        std::thread first(increment), second(increment);
        first.join(); second.join(); child_ok(child);
        TableRow *lock = nullptr; auto *row = table->get("k0", 2, &lock); assert(row && lock);
        long value = 0; row->get_value(integer, &value); assert(value == 384); lock->unlock();
        // Deleting the head promotes an overflow row; delete/reuse all conflict
        // slices afterward to cover aligned pool rows as well as primary rows.
        for (unsigned id = 0; id < 16; ++id) {
            char key[16]; int length = snprintf(key, sizeof(key), "k%u", id);
            assert(table->del(key, length));
        }
        assert(table->count() == 0);
        assert(table->get_available_slice_num() == table->get_total_slice_num());
        table->destroy();
    }
    printf("{\"passed\":true,\"odd_table_schemas\":18,\"records\":288,\"fork_cases\":18,\"shared_increments\":6912}\n");
    return 0;
}
#endif
