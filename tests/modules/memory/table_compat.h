#pragma once
// The pinned Table keeps its columns and counters in public fields; the Rust
// one (SWOOLE_RS_MEMORY) answers through accessors. The probes read both
// through these.
#include "swoole_table.h"
#include <string>
#include <string_view>

#ifdef SWOOLE_RS_MEMORY
inline size_t column_list_size(const swoole::Table *table) { return table->get_column_list_size(); }
inline const swoole::TableColumn *column_list_at(const swoole::Table *table, size_t i) { return table->get_column_at(i); }
inline std::string_view column_name_view(const swoole::TableColumn *column) {
    size_t length = 0;
    const char *name = swr_v1_memory_table_column_name(column, &length);
    return std::string_view(name, length);
}
inline swr_v1_memory_table_stats table_stats(const swoole::Table *table) { return table->stats(); }
#else
struct swr_v1_memory_table_stats {
    size_t conflict_count;
    uint32_t conflict_max_level;
    int64_t insert_count, update_count, delete_count;
};
inline size_t column_list_size(const swoole::Table *table) { return table->column_list->size(); }
inline const swoole::TableColumn *column_list_at(const swoole::Table *table, size_t i) { return (*table->column_list)[i]; }
inline std::string_view column_name_view(const swoole::TableColumn *column) { return column->name; }
inline swr_v1_memory_table_stats table_stats(const swoole::Table *table) {
    return {table->conflict_count, table->conflict_max_level, table->insert_count, table->update_count, table->delete_count};
}
#endif
inline std::string column_name(const swoole::TableColumn *column) { return std::string(column_name_view(column)); }
