#ifndef SWOOLE_RS_PHPCOLLECTIONS_H
#define SWOOLE_RS_PHPCOLLECTIONS_H
#include <stddef.h>
#include <stdint.h>
#ifdef __cplusplus
extern "C" {
#endif
/* Swoole\Coroutine\Channel (Rust): C++ lends the coroutine Channel's
 * operations; Rust registers the class and its zim_swoole_channel_coro_*
 * methods are its handlers. Swoole\Table is all Rust (php_swoole_table_minit). */
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
#ifdef __cplusplus
}
#endif
#endif
