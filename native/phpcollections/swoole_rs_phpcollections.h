#ifndef SWOOLE_RS_PHPCOLLECTIONS_H
#define SWOOLE_RS_PHPCOLLECTIONS_H
#include <stddef.h>
#include <stdint.h>
#ifdef __cplusplus
extern "C" {
#endif
/* Swoole\Coroutine\Channel and Swoole\Table (Rust): C++ lends the coroutine
 * Channel's operations and the Table's allocation, hash and error reports;
 * Rust registers the classes and its zim_swoole_channel_coro_* and
 * zim_swoole_table_* methods are the tables' handlers. */
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
struct swr_v1_phpcollections_table_ops {
    void *(*make)(uint32_t rows, float conflict);
    uint8_t (*create)(void *table);
    void (*destroy)(void *table);
    void (*unconstructed)(void);
    void (*fatal)(int32_t level, const char *message);
    void (*key_too_long)(const char *key);
    void (*set_failed)(const char *key, int32_t length);
    void (*no_column)(const char *column);
};
void swr_v1_phpcollections_table_minit(struct _zend_class_entry *exception,
                                       const struct swr_v1_phpcollections_table_ops *ops);
#ifdef __cplusplus
}
#endif
#endif
