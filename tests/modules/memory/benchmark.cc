#include "swoole_memory.h"
#include "swoole_heap.h"
#include "swoole_channel.h"
#include "swoole_table.h"
#include "table_compat.h"
using BenchmarkTable = swoole::Table;
#include <cassert>
#include <chrono>
#include <cstdio>
#include <cstring>

using Clock = std::chrono::steady_clock;
static uint64_t elapsed(Clock::time_point start) {
    return std::chrono::duration_cast<std::chrono::nanoseconds>(Clock::now() - start).count();
}

// Same actual constructors for the untimed preflight and later measurement.
static void fixed_construction(unsigned loops, bool owned, uint64_t &checksum) {
    alignas(8) unsigned char arena[48 + 32 * (24 + 64)];
    const auto exercise = [&](swoole::FixedPool &pool) {
        assert(pool.get_number_of_total_slice() == 32);
        assert(pool.get_number_of_spare_slice() == 32);
        auto *value = static_cast<unsigned char *>(pool.alloc(64)); assert(value);
        value[0] = 0x5a; checksum += value[0];
        assert(pool.get_number_of_spare_slice() == 31);
        pool.free(value);
        assert(pool.get_number_of_spare_slice() == 32);
    };
    if (owned) {
        for (unsigned i = 0; i < loops; ++i) {
            swoole::FixedPool pool(32, 64, false); exercise(pool);
        }
    } else {
        for (unsigned i = 0; i < loops; ++i) {
            swoole::FixedPool pool(64, arena, sizeof(arena), false); exercise(pool);
        }
    }
}
extern "C" __attribute__((visibility("default"))) int swr_memory_construction_check(void) {
    uint64_t checksum = 0;
    fixed_construction(5, true, checksum); fixed_construction(5, false, checksum);
    assert(checksum == 900);
    printf("{\"passed\":true,\"owned_constructions\":5,\"borrowed_constructions\":5,\"checksum\":%llu,\"timed\":false}\n",
           static_cast<unsigned long long>(checksum));
    return 0;
}

using TestColumn = const swoole::TableColumn *;
static bool column_valid(TestColumn c) { return c != nullptr; }
static size_t column_index(TestColumn c) { return c->index; }
static uint32_t column_width(TestColumn c) { return c->size; }
static void column_get(swoole::TableRow *row, TestColumn c, long *value) { row->get_value(c, value); }
static void column_set(swoole::TableRow *row, TestColumn c, const long *value) { row->set_value(c, value, 0); }

// Shared by the untimed preflight and later optimized measurements.
static BenchmarkTable *column_schema(unsigned count, unsigned prefix = 0) {
    auto *table = BenchmarkTable::make(64, 0.2); assert(table);
    for (unsigned i = 0; i < count; ++i) {
        std::string name(prefix, 'x');
        if (prefix) name[prefix / 2] = '\0';
        name += "column" + std::to_string(i);
        assert(table->add_column(name, swoole::TableColumn::TYPE_INT, 0));
        const auto column = table->get_column(name);
        assert(column_valid(column) && column_index(column) == i * 8 && column_name(column) == name);
    }
    assert(table->get_column_size() == count);
    return table;
}
static void column_traversal(const BenchmarkTable *table, unsigned count, unsigned loops, uint64_t &checksum) {
    for (unsigned pass = 0; pass < loops; ++pass) {
        unsigned seen = 0;
        assert(column_list_size(table) == count);
        for (size_t i = 0; i < count; ++i) {
            const auto *column = column_list_at(table, i);
            assert(column_valid(column) && column_index(column) == seen * 8 && column_width(column) == 8);
            checksum += column_index(column) / 8 + 1;
            ++seen;
        }
        assert(seen == count);
    }
}
// Same name observation used by the PHP full-row consumer: both borrow the
// stored name.
static void column_names(const BenchmarkTable *table, unsigned count, unsigned passes, uint64_t &checksum) {
    for (unsigned pass = 0; pass < passes; ++pass) {
        unsigned seen = 0;
        for (size_t k = 0; k < column_list_size(table); ++k) {
            const auto name = column_name_view(column_list_at(table, k));
            const char *bytes = name.data();
            const size_t length = name.size();
            // Consume every byte (including embedded NUL), without reconstructing
            // metadata in the control or timing Zend/PHP/startup itself.
            checksum = (checksum ^ length) * 1099511628211ULL;
            for (size_t i = 0; i < length; ++i) checksum = (checksum ^ static_cast<unsigned char>(bytes[i])) * 1099511628211ULL;
            ++seen;
        }
        assert(seen == count);
    }
}
static void column_lifecycle(unsigned rounds, uint64_t &checksum) {
    for (unsigned i = 0; i < rounds; ++i) {
        auto *table = column_schema(17);
        column_traversal(table, 17, 1, checksum);
        table->destroy();
    }
}
extern "C" __attribute__((visibility("default"))) int swr_memory_columns_check(void) {
    uint64_t checksum = 0;
    column_lifecycle(3, checksum);
    auto *table = column_schema(65);
    column_traversal(table, 65, 5, checksum);
    table->destroy();
    assert(checksum == 11184);
    uint64_t names = 1469598103934665603ULL;
    for (unsigned prefix : {0u, 80u}) {
        auto *names_table = column_schema(65, prefix);
        column_names(names_table, 65, 2, names);
        names_table->destroy();
    }
    assert(names == 12300970927948216643ULL);
    printf("{\"passed\":true,\"schemas\":3,\"columns_per_schema\":17,\"cursor_columns\":65,\"cursor_passes\":5,\"checksum\":%llu,\"name_schemas\":2,\"name_passes\":2,\"name_checksum\":%llu,\"timed\":false}\n",
           static_cast<unsigned long long>(checksum), static_cast<unsigned long long>(names));
    return 0;
}

// Called only in an integrator-reserved timing window, optimized and unsanitized.
extern "C" __attribute__((visibility("default"))) int swr_memory_benchmark(unsigned loops) {
    if (loops == 0 || loops > 1000000) return 2;
    uint64_t checksum = 0, fixed_ns, heap_ns, channel_ns, ring_ns, global_ns, table_ns, table_clear8_ns, table_clear33_ns, fixed_construct_owned_ns, fixed_construct_borrowed_ns, table_schema17_ns, table_columns65_ns, table_names65_ns, table_long_names65_ns;
    const unsigned schema_rounds = std::min(loops, 256u);
    {
        swoole::FixedPool pool(1024, 64, false);
        auto start = Clock::now();
        for (unsigned i = 0; i < loops; ++i) {
            auto *p = static_cast<unsigned char *>(pool.alloc(64)); assert(p);
            p[0] = i; checksum += p[0]; pool.free(p);
        }
        fixed_ns = elapsed(start);
    }
    {
        swoole::Heap heap(128, swoole::Heap::MIN_HEAP);
        for (unsigned i = 1; i <= 64; ++i) assert(heap.push(i, reinterpret_cast<void *>(uintptr_t(i))));
        auto start = Clock::now();
        for (unsigned i = 0; i < loops; ++i) {
            auto *token = heap.pop(); assert(token); checksum += reinterpret_cast<uintptr_t>(token);
            assert(heap.push(i % 128, token));
        }
        heap_ns = elapsed(start);
    }
    {
        auto *channel = swoole::Channel::make(65536, 64, swoole::SW_CHAN_LOCK); assert(channel);
        char input[64]{}, output[64]{};
        auto start = Clock::now();
        for (unsigned i = 0; i < loops; ++i) {
            input[0] = i; assert(channel->push(input, sizeof(input)) == SW_OK);
            assert(channel->pop(output, sizeof(output)) == sizeof(output)); checksum += static_cast<unsigned char>(output[0]);
        }
        channel_ns = elapsed(start); channel->destroy();
    }
    {
        swoole::RingBuffer pool(65536, false);
        auto start = Clock::now();
        for (unsigned i = 0; i < loops; ++i) {
            auto *p = static_cast<unsigned char *>(pool.alloc(64)); assert(p);
            p[0] = i; checksum += p[0]; pool.free(p);
        }
        ring_ns = elapsed(start);
    }
    {
        swoole::GlobalMemory pool(1048576, false);
        auto start = Clock::now();
        for (unsigned i = 0; i < loops; ++i) {
            auto *p = static_cast<unsigned char *>(pool.alloc(64)); assert(p && p[0] == 0);
            p[0] = i; checksum += p[0];
        }
        global_ns = elapsed(start);
    }
    {
        auto *table = BenchmarkTable::make(1024, 0.2); assert(table);
        char names[8][16]{}, keys[64][16]{}; size_t lengths[64];
        for (unsigned i = 0; i < 8; ++i) { snprintf(names[i], sizeof(names[i]), "column%u", i); assert(table->add_column(names[i], swoole::TableColumn::TYPE_INT, 0)); }
        assert(table->create());
        for (unsigned i = 0; i < 64; ++i) {
            lengths[i] = snprintf(keys[i], sizeof(keys[i]), "row%u", i);
            swoole::TableRow *lock = nullptr; auto *row = table->set(keys[i], lengths[i], &lock, nullptr); assert(row); table->clear_row(row); lock->unlock();
        }
        auto start = Clock::now();
        for (unsigned i = 0; i < loops; ++i) {
            swoole::TableRow *lock = nullptr; auto *row = table->get(keys[i % 64], lengths[i % 64], &lock); assert(row);
            const auto column = table->get_column(names[i % 8]); assert(column_valid(column));
            long value = 0; column_get(row, column, &value); value++; column_set(row, column, &value); checksum += value; lock->unlock();
        }
        table_ns = elapsed(start); table->destroy();
    }
    for (unsigned columns : {8u, 33u}) {
        auto *table = BenchmarkTable::make(64, 0.2); assert(table);
        for (unsigned i = 0; i < columns; ++i) {
            const auto kind = static_cast<swoole::TableColumn::Type>(1 + i % 3);
            assert(table->add_column("clear" + std::to_string(i), kind, 4));
        }
        assert(table->create());
        swoole::TableRow *lock = nullptr;
        auto *row = table->set("clear", 5, &lock, nullptr); assert(row);
        auto start = Clock::now();
        for (unsigned i = 0; i < loops; ++i) {
            memset(row->data, 0xa5, columns * 8);
            table->clear_row(row);
            const auto *data = reinterpret_cast<const unsigned char *>(row->data);
            assert(data[(columns - 1) * 8] == 0 && data[20] == 0xa5);
            checksum += data[(columns - 1) * 8] + data[20];
        }
        if (columns == 8) table_clear8_ns = elapsed(start);
        else table_clear33_ns = elapsed(start);
        lock->unlock(); table->destroy();
    }
    {
        auto start = Clock::now(); fixed_construction(loops, true, checksum);
        fixed_construct_owned_ns = elapsed(start);
        start = Clock::now(); fixed_construction(loops, false, checksum);
        fixed_construct_borrowed_ns = elapsed(start);
    }
    {
        // Bounded factory lifetime work: global core allocation is monotonic in
        // the reference, so never multiply this by unbounded timing loops.
        auto start = Clock::now(); column_lifecycle(schema_rounds, checksum);
        table_schema17_ns = elapsed(start);
        auto *table = column_schema(65);
        start = Clock::now(); column_traversal(table, 65, loops, checksum);
        table_columns65_ns = elapsed(start); table->destroy();
    }
    for (unsigned prefix : {0u, 80u}) {
        auto *table = column_schema(65, prefix);
        auto start = Clock::now();
        column_names(table, 65, loops, checksum);
        const auto duration = elapsed(start);
        if (prefix) table_long_names65_ns = duration;
        else table_names65_ns = duration;
        table->destroy();
    }
    printf("{\"loops\":%u,\"checksum\":%llu,\"fixed_ns\":%llu,\"heap_ns\":%llu,\"channel_ns\":%llu,\"ring_ns\":%llu,\"global_ns\":%llu,\"table_ns\":%llu,\"table_clear8_ns\":%llu,\"table_clear33_ns\":%llu,\"fixed_construct_owned_ns\":%llu,\"fixed_construct_borrowed_ns\":%llu,\"table_schema_rounds\":%u,\"table_schema17_ns\":%llu,\"table_columns65_ns\":%llu,\"table_names65_ns\":%llu,\"table_long_names65_ns\":%llu}\n", loops,
           (unsigned long long) checksum, (unsigned long long) fixed_ns, (unsigned long long) heap_ns,
           (unsigned long long) channel_ns, (unsigned long long) ring_ns, (unsigned long long) global_ns, (unsigned long long) table_ns,
           (unsigned long long) table_clear8_ns, (unsigned long long) table_clear33_ns,
           (unsigned long long) fixed_construct_owned_ns, (unsigned long long) fixed_construct_borrowed_ns, schema_rounds,
           (unsigned long long) table_schema17_ns, (unsigned long long) table_columns65_ns,
           (unsigned long long) table_names65_ns, (unsigned long long) table_long_names65_ns);
    return 0;
}
