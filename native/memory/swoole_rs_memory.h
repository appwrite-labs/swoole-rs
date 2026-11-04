#pragma once
#include <stdbool.h>
#include <stddef.h>
#include <stdint.h>
#ifdef __cplusplus
namespace swoole { struct TableRow; struct TableColumn; class Table; }
typedef swoole::TableRow swr_v1_memory_table_link;
typedef swoole::TableColumn swr_v1_memory_table_column_link;
typedef swoole::Table swr_v1_memory_table;
extern "C" {
#else
typedef struct swr_v1_memory_table_row swr_v1_memory_table_link;
typedef struct swr_v1_memory_table_column swr_v1_memory_table_column_link;
typedef struct swr_v1_memory_table swr_v1_memory_table;
#endif
/* Single-use whole SharedMemory operation cursor. OS constants/results are
 * copied Native facts, never guessed Rust libc values or Native class objects.
 * phase0 starts; Native changes only memory/descriptor/result after each effect.
 * All effects run after return; state/output/live extents are disjoint. */
typedef struct swr_v2_memory_mapping_operation {
    size_t requested;
    uint8_t *source, *memory;
    size_t total;
    uint32_t kind, phase;
    int32_t flags, descriptor, result;
    uint32_t anonymous;
    int32_t anonymous_flags;
} swr_v2_memory_mapping_operation;
typedef struct swr_v2_memory_mapping_effect {
    void *memory;
    size_t size;
    int32_t descriptor, flags;
    uint32_t action;
    int32_t result;
} swr_v2_memory_mapping_effect;
enum swr_v2_memory_mapping_kind {
    SWR_MEMORY_MAPPING_ALLOCATE=0, SWR_MEMORY_MAPPING_RELEASE=1,
    SWR_MEMORY_MAPPING_PROTECT=2, SWR_MEMORY_MAPPING_REALLOCATE=3
};
enum swr_v2_memory_mapping_action {
    SWR_MEMORY_MAPPING_OPEN=1, SWR_MEMORY_MAPPING_MAP=2,
    SWR_MEMORY_MAPPING_WARN_ALLOCATE=3, SWR_MEMORY_MAPPING_UNMAP=4,
    SWR_MEMORY_MAPPING_WARN_RELEASE=5, SWR_MEMORY_MAPPING_SET_PROTECTION=6,
    SWR_MEMORY_MAPPING_INVALID_PROTECTION=7,
    SWR_MEMORY_MAPPING_ALLOCATE_PAYLOAD=8, SWR_MEMORY_MAPPING_RELEASE_NEW_PAYLOAD=9,
    SWR_MEMORY_MAPPING_RELEASE_OLD_PAYLOAD=10
};
/*1 requests the action,0 completes (memory/result),-1 rejects control/header.
 * Existing mapping_size/product/initialize/extent/relocate contracts remain. */
int32_t swr_v2_memory_mapping_step(swr_v2_memory_mapping_operation *, swr_v2_memory_mapping_effect *);

/* Borrowed C PODs; no callback, destructor or allocation callback enters Rust. */
typedef struct swr_v1_memory_fixed_slice {
    uint8_t lock;
    struct swr_v1_memory_fixed_slice *next, *prev;
} swr_v1_memory_fixed_slice;
typedef struct swr_v1_memory_fixed {
    void *memory;
    size_t size;
    swr_v1_memory_fixed_slice *head, *tail;
    uint32_t slice_num, slice_use, slice_size;
    uint8_t shared, allocated;
} swr_v1_memory_fixed;
typedef struct swr_v1_memory_fixed_layout_result {
    size_t total, usable;
    uint32_t slices, width;
} swr_v1_memory_fixed_layout_result;
/* owned=1: count>=2, available=0, width rounded to8. owned=0: count=0,
 * available is the borrowed arena length, original odd widths retained. */
int32_t swr_v1_memory_fixed_layout(uint32_t count, uint32_t width, size_t available,
                                 uint32_t owned, swr_v1_memory_fixed_layout_result *);
/* Serialized read-only pool snapshot; output is independent/disjoint. Spare
 * subtraction preserves defined unsigned wrap; actual public widths unchanged. */
typedef struct swr_v1_memory_fixed_usage_result { uint32_t spare, total, width; } swr_v1_memory_fixed_usage_result;
int32_t swr_v1_memory_fixed_usage(const swr_v1_memory_fixed *, swr_v1_memory_fixed_usage_result *);
/* Native executes the returned ownership action after Rust returns:
 *0 borrowed/no effect,1/2 heap/shared allocation,3/4 heap/shared free,-1 invalid.
 * Used by actual FixedPool and RingBuffer constructors/rollback/destructors. */
int32_t swr_v1_memory_arena_lifecycle(uint32_t shared, uint32_t owned, uint32_t phase);
/* Independent serialized debug cursor/output; no C++ object or native callback.
 * Cursor starts zero; step1 emits a record,0 ends,-1 rejects. max0 emits2 as pin.
 * Live arena links remain valid throughout traversal; output/cursor disjoint. */
typedef struct swr_v1_memory_fixed_debug_cursor {
    swr_v1_memory_fixed_slice *next;
    uint64_t line;
} swr_v1_memory_fixed_debug_cursor;
typedef struct swr_v1_memory_fixed_debug_record {
    swr_v1_memory_fixed_slice *slice, *prev, *next;
    uint8_t *data;
    uint64_t line;
    uint8_t lock, self_link;
} swr_v1_memory_fixed_debug_record;
int32_t swr_v1_memory_fixed_debug_step(const swr_v1_memory_fixed *, swr_v1_memory_fixed_debug_cursor *,
                                      int32_t max_lines, swr_v1_memory_fixed_debug_record *);
/* Separate immutable plan; exclusive arena covers its complete total bytes. */
int32_t swr_v1_memory_fixed_construct(swr_v1_memory_fixed *, const swr_v1_memory_fixed_layout_result *,
                                    uint32_t shared, uint32_t owned);
int32_t swr_v1_memory_fixed_init(swr_v1_memory_fixed *);
void *swr_v1_memory_fixed_alloc(swr_v1_memory_fixed *);
int32_t swr_v1_memory_fixed_free(swr_v1_memory_fixed *, void *);
/* Independent channel state snapshot; no C++ class, lock or pipe crosses Rust. */
typedef struct swr_v1_memory_channel {
    int64_t head, tail;
    size_t size;
    uint8_t head_tag, tail_tag;
    int32_t num, max_num;
    size_t bytes;
    int32_t flags, maxlen;
    void *mem;
} swr_v1_memory_channel;
int32_t swr_v1_memory_channel_in(swr_v1_memory_channel *, const void *, int32_t);
int32_t swr_v1_memory_channel_out(swr_v1_memory_channel *, void *, int32_t, uint32_t consume);
/* Independent typed field addresses, obtained by native member access. Targets
 * are disjoint, live, aligned, serialized and initialized (except during init).
 * Descriptor/plan/payload are disjoint. No C++ class, lock or pipe is borrowed. */
typedef struct swr_v1_memory_channel_fields {
    int64_t *head, *tail;
    size_t *size;
    char *head_tag, *tail_tag;
    int32_t *num, *max_num;
    size_t *bytes;
    int32_t *flags, *maxlen;
    void **mem;
} swr_v1_memory_channel_fields;
typedef struct swr_v1_memory_channel_plan {
    size_t total, size, header_size;
    int32_t maxlen, flags;
} swr_v1_memory_channel_plan;
int32_t swr_v1_memory_channel_storage_plan(size_t, size_t, int32_t, size_t, swr_v1_memory_channel_plan *);
int32_t swr_v1_memory_channel_storage_init(const swr_v1_memory_channel_fields *, const swr_v1_memory_channel_plan *, void *);
int32_t swr_v1_memory_channel_storage_in(const swr_v1_memory_channel_fields *, const void *, int32_t);
int32_t swr_v1_memory_channel_storage_out(const swr_v1_memory_channel_fields *, void *, int32_t);
/* Independent read-only scalars for full(); unsigned occupancy wraps as C++. */
typedef struct swr_v1_memory_channel_capacity {
    int64_t head, tail;
    size_t size, bytes;
    int32_t num;
    uint8_t head_tag, tail_tag;
} swr_v1_memory_channel_capacity;
int32_t swr_v1_memory_channel_full(const swr_v1_memory_channel_capacity *);
enum swr_v1_memory_channel_action {
    SWR_CHANNEL_ALLOC_HEAP = 1, SWR_CHANNEL_ALLOC_SHARED = 2, SWR_CHANNEL_INITIALIZE = 3,
    SWR_CHANNEL_CREATE_LOCK = 4, SWR_CHANNEL_CREATE_PIPE = 5, SWR_CHANNEL_DELETE_LOCK = 6,
    SWR_CHANNEL_DELETE_PIPE = 7, SWR_CHANNEL_FREE_HEAP = 8, SWR_CHANNEL_FREE_SHARED = 9
};
typedef struct swr_v1_memory_channel_lifecycle_result {
    uint32_t count, actions[4];
} swr_v1_memory_channel_lifecycle_result;
/* phase0: constructor effects, phase1: teardown effects, in native execution order. */
int32_t swr_v1_memory_channel_lifecycle(int32_t flags, uint32_t phase, swr_v1_memory_channel_lifecycle_result *);
/* Rust-owned opaque heap; public node POD layout stays unchanged. */
typedef struct swr_v1_memory_heap_node {
    uint64_t priority;
    uint32_t position;
    void *data;
} swr_v1_memory_heap_node;
/* Independent snapshot and index; serialized with heap mutations. Each step
 * copies one live node and advances index in Rust; terminal output is cleared.
 *1 record,0 end,-1 invalid; native printf executes only after return. */
int32_t swr_v1_memory_heap_debug_step(const void *, size_t *, swr_v1_memory_heap_node *);
int32_t swr_v1_memory_heap_create(size_t capacity, uint32_t max, void **);
void swr_v1_memory_heap_release(void *);
size_t swr_v1_memory_heap_count(const void *);
swr_v1_memory_heap_node *swr_v1_memory_heap_at(const void *, size_t index);
int32_t swr_v1_memory_heap_compare(const void *, uint64_t a, uint64_t b);
swr_v1_memory_heap_node *swr_v1_memory_heap_push(void *, uint64_t, void *);
int32_t swr_v1_memory_heap_update(void *, uint64_t, swr_v1_memory_heap_node *);
int32_t swr_v1_memory_heap_remove(void *, swr_v1_memory_heap_node *);
void *swr_v1_memory_heap_pop(void *);
/* Rust-owned page registry/configuration. Native performs page/mutex/getpid and
 * wrapper effects after return; inherited shared pages remain attached on reset. */
typedef struct swr_v1_memory_global_effect {
    void *page;
    uint32_t page_size, aligned, action;
} swr_v1_memory_global_effect;
enum swr_v1_memory_global_action {
    SWR_GLOBAL_ALLOC_HEAP = 1, SWR_GLOBAL_ALLOC_SHARED = 2,
    SWR_GLOBAL_FREE_HEAP = 3, SWR_GLOBAL_FREE_SHARED = 4, SWR_GLOBAL_RESET_SHARED = 5
};
/* open: -2 invalid config, -1 allocation failure,0 success; shared is0/1.
 * Outputs/descriptors disjoint from registry/pages; calls serialized by native.
 * phase0 allocation plan; phase1 validates requested before owner/reset choice;
 * phase2 drains one page in insertion order, action0 means no pages remain.
 * Native executes each free before next drain; release only after full drain.
 * No Rust reference survives a page effect or native wrapper replacement. */
int32_t swr_v1_memory_global_open(uint32_t requested, uint32_t system_page, uint32_t shared,
                                  int64_t creator, void **);
int32_t swr_v1_memory_global_step(void *, uint32_t phase, int64_t current, uint32_t requested,
                                  swr_v1_memory_global_effect *);
int32_t swr_v1_memory_global_page_size(uint32_t requested, uint32_t system_page, uint32_t *);
int32_t swr_v1_memory_global_request_valid(const void *, uint32_t requested, uint32_t *aligned);
int32_t swr_v1_memory_global_create(uint32_t page_size, void **);
int32_t swr_v1_memory_global_add_page(void *, void *page);
int32_t swr_v1_memory_global_alloc(void *, uint32_t size, void **);
size_t swr_v1_memory_global_capacity(const void *);
size_t swr_v1_memory_global_memory_size(const void *);
void *swr_v1_memory_global_pop_page(void *);
int32_t swr_v1_memory_global_release(void *);
/* Producer-only snapshot. Atomic slot/header synchronization is shared. */
typedef struct swr_v1_memory_ring {
    void *memory;
    uint32_t size, alloc_offset, collect_offset, alloc_count;
    uint8_t status;
    uint32_t *free_count;
} swr_v1_memory_ring;
int32_t swr_v1_memory_ring_layout(uint32_t requested, uint32_t *total, uint32_t *payload);
void *swr_v1_memory_ring_alloc(swr_v1_memory_ring *, uint32_t requested);
int32_t swr_v1_memory_ring_free(void *memory, uint32_t size, uint32_t *free_count, void *data);
/* Actual shared arena POD base; no C++ class layout or mutable whole-header
 * reference crosses Rust. Producer fields and atomic counter are separate. */
typedef struct swr_v1_memory_ring_storage {
    void *memory;
    uint8_t shared, status;
    uint32_t size, alloc_offset, collect_offset, alloc_count;
    volatile uint32_t free_count;
} swr_v1_memory_ring_storage;
/* Fresh exclusive total-byte arena, before any producer/consumer publication. */
int32_t swr_v1_memory_ring_construct(swr_v1_memory_ring_storage *, uint32_t total, uint32_t shared);
void *swr_v1_memory_ring_storage_alloc(swr_v1_memory_ring_storage *, uint32_t);
/* Checked native shared-mapping byte plans, including the8-byte header. */
typedef struct swr_v1_memory_mapping_extent_result {
    void *base;
    size_t total;
} swr_v1_memory_mapping_extent_result;
/* Native supplies live mapping storage; actual 8-byte header remains in arena.
 * Outputs are separate exclusive PODs. Relocation requires disjoint mappings. */
void *swr_v1_memory_mapping_initialize(void *base, size_t total);
int32_t swr_v1_memory_mapping_extent(const void *payload, swr_v1_memory_mapping_extent_result *);
int32_t swr_v1_memory_mapping_relocate(const void *old, void *new_);
int32_t swr_v1_memory_mapping_size(size_t requested, size_t *total);
int32_t swr_v1_memory_mapping_product(size_t count, size_t width, size_t *requested);
int32_t swr_v1_memory_mapping_copy(size_t old_total, size_t new_total, size_t *bytes);
/* swoole::Table (crates/memory/src/table.rs). Rust owns the table; the host
 * allocates its header (header_size() bytes, 8-aligned, shared memory) and,
 * for create, its arena (arena_size() bytes). Every row pointer here is a row
 * of the table or the iterator's copy of one; a bucket's chain is read and
 * written only under the bucket's lock. */
typedef struct swr_v1_memory_table_row {
    volatile uint32_t lock_;
    int32_t lock_pid;
    uint8_t active, key_len;
    swr_v1_memory_table_link *next;
    char key[64];
    char data[0];
} swr_v1_memory_table_row;
/* The leading fields of a column, which Rust allocates. */
typedef struct swr_v1_memory_table_column {
    uint32_t type; /* 1 int, 2 float, 3 string */
    uint32_t size; /* bytes in a row: 8, or a string's capacity plus its 4-byte length */
    size_t index;  /* offset in a row's data */
} swr_v1_memory_table_column;
typedef struct swr_v1_memory_table_stats {
    size_t conflict_count;
    uint32_t conflict_max_level;
    int64_t insert_count, update_count, delete_count;
} swr_v1_memory_table_stats;
typedef uint64_t (*swr_v1_memory_table_hash)(const char *key, size_t length);
/* Logs one whole warning line ("Class::method(): message"). */
typedef void (*swr_v1_memory_table_warning)(const char *message, size_t length);
typedef void (*swr_v1_memory_table_error)(int code);
size_t swr_v1_memory_table_header_size(void);
bool swr_v1_memory_table_init(swr_v1_memory_table *, uint32_t rows, float conflict, swr_v1_memory_table_hash,
                              swr_v1_memory_table_warning, swr_v1_memory_table_error);
void swr_v1_memory_table_set_hash(swr_v1_memory_table *, swr_v1_memory_table_hash);
bool swr_v1_memory_table_add_column(swr_v1_memory_table *, const char *name, size_t length, uint32_t kind, size_t size);
swr_v1_memory_table_column_link *swr_v1_memory_table_find_column(const swr_v1_memory_table *, const char *name, size_t length);
swr_v1_memory_table_column_link *swr_v1_memory_table_column_at(const swr_v1_memory_table *, size_t index);
size_t swr_v1_memory_table_column_count(const swr_v1_memory_table *, bool unique);
const char *swr_v1_memory_table_column_name(const swr_v1_memory_table_column *, size_t *length);
size_t swr_v1_memory_table_memory_total(const swr_v1_memory_table *);
size_t swr_v1_memory_table_arena_size(const swr_v1_memory_table *);
bool swr_v1_memory_table_create(swr_v1_memory_table *, void *arena);
/* Frees the table's own parts; returns its arena (or NULL) for the host to free. */
void *swr_v1_memory_table_destroy(swr_v1_memory_table *);
bool swr_v1_memory_table_created(const swr_v1_memory_table *);
bool swr_v1_memory_table_ready(const swr_v1_memory_table *);
size_t swr_v1_memory_table_size(const swr_v1_memory_table *);
float swr_v1_memory_table_conflict_proportion(const swr_v1_memory_table *);
size_t swr_v1_memory_table_memory_size(const swr_v1_memory_table *);
size_t swr_v1_memory_table_count(const swr_v1_memory_table *);
swr_v1_memory_table_stats swr_v1_memory_table_counters(const swr_v1_memory_table *);
uint32_t swr_v1_memory_table_slices(const swr_v1_memory_table *, bool available);
swr_v1_memory_table_link *swr_v1_memory_table_set(const swr_v1_memory_table *, const char *key, uint16_t length,
                                                  swr_v1_memory_table_link **lock, int *flags);
swr_v1_memory_table_link *swr_v1_memory_table_get(const swr_v1_memory_table *, const char *key, uint16_t length,
                                                  swr_v1_memory_table_link **lock);
bool swr_v1_memory_table_exists(const swr_v1_memory_table *, const char *key, uint16_t length);
bool swr_v1_memory_table_del(const swr_v1_memory_table *, const char *key, uint16_t length);
swr_v1_memory_table_link *swr_v1_memory_table_by_index(const swr_v1_memory_table *, uint32_t index);
void swr_v1_memory_table_rewind(const swr_v1_memory_table *);
void swr_v1_memory_table_forward(const swr_v1_memory_table *);
swr_v1_memory_table_link *swr_v1_memory_table_current(const swr_v1_memory_table *);
void swr_v1_memory_table_clear_row(const swr_v1_memory_table *, swr_v1_memory_table_row *);
void swr_v1_memory_table_row_lock(swr_v1_memory_table_row *, swr_v1_memory_table_warning);
void swr_v1_memory_table_row_unlock(swr_v1_memory_table_row *);
void swr_v1_memory_table_row_clear(swr_v1_memory_table_row *);
void swr_v1_memory_table_set_value(swr_v1_memory_table_row *, const swr_v1_memory_table_column *, const void *value,
                                   size_t length, swr_v1_memory_table_warning);
void swr_v1_memory_table_clear_value(swr_v1_memory_table_row *, const swr_v1_memory_table_column *);
void swr_v1_memory_table_get_number(const swr_v1_memory_table_row *, const swr_v1_memory_table_column *, void *value);
void swr_v1_memory_table_get_string(const swr_v1_memory_table_row *, const swr_v1_memory_table_column *, char **value,
                                    uint32_t *length);
#ifdef __cplusplus
}
#endif
