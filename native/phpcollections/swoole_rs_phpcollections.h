#ifndef SWOOLE_RS_PHPCOLLECTIONS_H
#define SWOOLE_RS_PHPCOLLECTIONS_H
#include <stddef.h>
#include <stdint.h>
#ifdef __cplusplus
extern "C" {
#endif
/* Swoole\Coroutine\Channel and Swoole\Table (Rust): C++ lends the coroutine
 * Channel's and TableV2's operations; Rust registers the classes and its
 * zim_swoole_channel_coro_* and zim_swoole_table_* methods are the tables'
 * handlers. */
struct _zend_class_entry;
struct swr_v1_phpcollections_channel_ops {
    void *(*create)(size_t capacity);
    void (*destroy)(void *chan);
    uint8_t (*push)(void *chan, void *data, double timeout);
    void *(*pop)(void *chan, double timeout);
    void *(*pop_data)(void *chan);
    int32_t (*error)(void *chan);
    uint8_t (*close)(void *chan);
    size_t (*length)(void *chan);
    uint8_t (*is_empty)(void *chan);
    uint8_t (*is_full)(void *chan);
    size_t (*consumers)(void *chan);
    size_t (*producers)(void *chan);
    void (*unconstructed)(void);
};
void swr_v1_phpcollections_channel_minit(int32_t module_number,
                                         uint8_t use_shortname,
                                         const struct swr_v1_phpcollections_channel_ops *ops);
struct swr_v1_memory_records;
struct swr_v1_memory_table_row;
struct swr_v1_phpcollections_table_ops {
    void *(*make)(uint32_t rows, float conflict);
    void (*destroy)(void *table);
    struct swr_v1_memory_records *(*records)(void *table, size_t **item_size, const uint8_t **created);
    uint8_t (*create)(void *table);
    uint8_t (*ready)(void *table);
    size_t (*count)(void *table);
    size_t (*size)(void *table);
    size_t (*memory_size)(void *table);
    void (*stats)(void *table, int64_t *values);
    struct swr_v1_memory_table_row *(*set)(void *table, const char *key, uint16_t length,
                                           struct swr_v1_memory_table_row **lock, int32_t *flags);
    struct swr_v1_memory_table_row *(*get)(void *table, const char *key, uint16_t length,
                                           struct swr_v1_memory_table_row **lock);
    uint8_t (*del)(void *table, const char *key, uint16_t length);
    uint8_t (*exists)(void *table, const char *key, uint16_t length);
    void (*unlock)(struct swr_v1_memory_table_row *row);
    void (*rewind)(void *table);
    void (*forward)(void *table);
    struct swr_v1_memory_table_row *(*current)(void *table);
    void (*unconstructed)(void);
    void (*fatal)(int32_t level, const char *message);
    void (*key_too_long)(const char *key);
    void (*set_failed)(const char *key, int32_t length);
    void (*no_column)(const char *column);
    void (*too_long)(struct swr_v1_memory_table_row *row, const char *field, size_t length);
    void (*log_warning)(const char *message);
};
void swr_v1_phpcollections_table_minit(struct _zend_class_entry *exception,
                                       const struct swr_v1_phpcollections_table_ops *ops);
#ifdef __cplusplus
}
#endif
#endif
