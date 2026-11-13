#ifndef SWOOLE_RS_H
#define SWOOLE_RS_H

#include <stdbool.h>
#include <stddef.h>
#include <stdint.h>

#define SWR_ABI_VERSION 1

#ifdef __cplusplus
extern "C" {
#endif

/* Stateless, allocation-free queries; safe on any thread, including after fork. */
uint32_t swr_v1_abi_version(void);
uint64_t swr_v1_feature_bits(void);

#ifdef __cplusplus
}
#endif

#endif
