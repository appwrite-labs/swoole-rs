/* SPDX-License-Identifier: Apache-2.0 */
#ifndef SWOOLE_RS_SIGNAL_LOOP_H
#define SWOOLE_RS_SIGNAL_LOOP_H
#include <stdbool.h>
#include <stdint.h>
#include <stddef.h>
#ifdef __cplusplus
extern "C" {
#endif
struct swr_v1_signal_descriptor_ops {
    void *(*make)(int32_t);
    void (*free)(void *);
    int32_t (*events)(void *);
    int32_t (*add)(void *, int32_t);
    void (*del)(void *);
    bool (*installed)(void);
    void (*install)(void);
    void (*erase_end)(void);
};
struct swr_v1_signal_loop_ops {
    uint8_t (*capabilities)(void);
    bool (*available)(void);
    bool (*has_reactor)(void);
    int32_t (*native_handle)(void);
    void (*callback)(int32_t);
    const struct swr_v1_signal_descriptor_ops *descriptor;
    void (*invoke)(uintptr_t, int32_t);
    void (*report)(uint8_t, int32_t, int32_t);
};
uintptr_t swr_v1_signal_loop_set(int32_t signo, uintptr_t handler, bool safety, const struct swr_v1_signal_loop_ops *ops);
uintptr_t swr_v1_signal_loop_get(int32_t signo, const struct swr_v1_signal_loop_ops *ops);
void swr_v1_signal_loop_callback(int32_t signo, const struct swr_v1_signal_loop_ops *ops);
uint32_t swr_v1_signal_loop_listener_num(void);
void swr_v1_signal_loop_clear(const struct swr_v1_signal_loop_ops *ops);
#ifdef HAVE_SIGNALFD
int32_t swr_v1_signal_loop_read(int32_t fd, const struct swr_v1_signal_loop_ops *ops);
void swr_v1_signal_loop_destroyed(const struct swr_v1_signal_loop_ops *ops);
#endif
bool swr_v1_signal_loop_exit(size_t *event_num);
#ifdef __cplusplus
}
#endif
#endif
