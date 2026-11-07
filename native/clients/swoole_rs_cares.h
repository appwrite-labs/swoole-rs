/* SPDX-License-Identifier: Apache-2.0 */
#ifndef SWOOLE_RS_CARES_H
#define SWOOLE_RS_CARES_H
#ifdef SW_USE_CARES
#include <stddef.h>
#include <stdint.h>
#ifdef __cplusplus
extern "C" {
#endif
/* The coroutine and logging effects; Rust drives the reactor itself. */
struct swr_v1_clients_cares_ops {
    void *(*current)(void);
    uint8_t (*suspending)(void *);
    void (*yield_)(void *, double);
    uint8_t (*canceled)(void *);
    uint8_t (*timedout)(void *);
    void (*resume)(void *);
    void (*warning)(uint8_t, int32_t);
    void (*trace)(uint8_t, int32_t, int32_t, size_t);
};
void swr_v1_clients_cares_lookup(const struct swr_v1_clients_cares_ops *ops, const char *domain,
    int32_t family, double timeout, const char *server, uint16_t port, void *output,
    void (*push)(void *, const uint8_t *, size_t));
#ifdef __cplusplus
}
#endif
#endif /* SW_USE_CARES */
#endif
