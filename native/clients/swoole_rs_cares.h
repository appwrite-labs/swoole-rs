/* SPDX-License-Identifier: Apache-2.0 */
#ifndef SWOOLE_RS_CARES_H
#define SWOOLE_RS_CARES_H
#include <stddef.h>
#include <stdint.h>
#ifdef __cplusplus
extern "C" {
#endif
struct swr_v1_clients_cares_ops {
    uint8_t (*handlers)(void);
    void (*install)(void);
    void *(*current)(void);
    uint8_t (*suspending)(void *);
    void (*yield_)(void *, double);
    uint8_t (*canceled)(void *);
    uint8_t (*timedout)(void *);
    void (*resume)(void *);
    void (*defer)(void (*)(void *), void *);
    void *(*make)(int32_t, void *);
    int32_t (*events)(void *);
    void (*add)(void *, int32_t);
    void (*set)(void *, int32_t);
    void (*del)(void *);
    void (*free)(void *);
    void (*error)(int32_t);
    void (*warning)(uint8_t, int32_t);
    void (*trace)(uint8_t, int32_t, int32_t, size_t);
};
void swr_v1_clients_cares_process(void *data, int32_t fd, uint8_t write);
void swr_v1_clients_cares_cleanup(void);
void swr_v1_clients_cares_lookup(const struct swr_v1_clients_cares_ops *ops, const char *domain,
    int32_t family, double timeout, const char *server, uint16_t port, void *output,
    void (*push)(void *, const uint8_t *, size_t));
#ifdef __cplusplus
}
#endif
#endif
