#ifndef SWOOLE_RS_PROTOCOLS_H
#define SWOOLE_RS_PROTOCOLS_H
#include <stddef.h>
#include <stdint.h>
#include <stdbool.h>
#include <sys/types.h>
#include <sys/uio.h>
typedef struct swr_v1_core_proxy swr_v1_core_proxy;
#ifdef __cplusplus
extern "C" {
#endif
/* MessageBus (crates/bus), for crates/process' send_message. */
typedef struct swr_bus swr_bus;
#ifdef __cplusplus
}
#endif
#endif
