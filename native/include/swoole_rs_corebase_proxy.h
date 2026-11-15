/* SPDX-License-Identifier: Apache-2.0 */
#pragma once
#include <stdbool.h>
#include "swoole_rs_corebase_string_slot.h"
#ifdef __cplusplus
extern "C" {
#endif
typedef struct swr_v1_core_proxy swr_v1_core_proxy;
typedef struct {
  uint8_t state;
  bool dont_handshake;
  uint8_t version, method, dns_tunnel;
  int32_t port, target_port, socket_type;
} swr_v1_core_proxy_fields;
typedef struct {
  swr_v1_core_proxy *owner;
  swr_v1_core_proxy_fields *fields;
  const swr_v1_core_string *strings[4];
  uint8_t *buffer;
} swr_v1_core_proxy_prepared;
swr_v1_core_proxy_prepared swr_v1_core_proxy_new(bool socks);
void swr_v1_core_proxy_free(swr_v1_core_proxy *);
#ifdef __cplusplus
}
#endif
