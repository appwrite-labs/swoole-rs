/* SPDX-License-Identifier: Apache-2.0 */
#pragma once
#include "swoole_rs_corebase_string_slot.h"
#ifdef __cplusplus
extern "C" {
#endif
typedef struct swr_v1_core_proxy swr_v1_core_proxy;
typedef struct {
  uint8_t state, dont_handshake, version, method, dns_tunnel;
  int32_t port, target_port, socket_type;
} swr_v1_core_proxy_fields;
typedef struct {
  swr_v1_core_proxy *owner;
  swr_v1_core_proxy_fields *fields;
  const swr_v1_core_string *strings[4];
  uint8_t *buffer;
} swr_v1_core_proxy_prepared;
typedef struct {
  int32_t status;
  uint32_t index;
} swr_v1_core_proxy_copy_result;
swr_v1_core_proxy_prepared swr_v1_core_proxy_new(bool socks);
void swr_v1_core_proxy_free(swr_v1_core_proxy *);
swr_v1_core_proxy_copy_result
swr_v1_core_proxy_assign(const swr_v1_core_proxy *, const swr_v1_core_proxy *);
swr_v1_core_proxy_copy_result
swr_v1_core_proxy_configure(swr_v1_core_proxy *,
                            const swr_v1_core_string_view_t *, int32_t, bool,
                            int32_t);
#ifdef __cplusplus
}
#endif
