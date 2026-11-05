/* SPDX-License-Identifier: Apache-2.0 */
#ifndef SWOOLE_RS_TLS_OWNER_H
#define SWOOLE_RS_TLS_OWNER_H
#include "swoole_rs_corebase_string_slot.h"
#include "swoole_rs_tls.h"
#ifdef __cplusplus
extern "C" {
#endif
typedef struct swr_v1_tls_owner swr_v1_tls_owner;
typedef struct {
  uint8_t http, http_v2, prefer_server_ciphers, session_tickets, stapling,
      stapling_verify;
  uint8_t disable_tls_host_name, grease, verify_depth, disable_compress,
      verify_peer, allow_self_signed;
  uint32_t protocols;
  uint8_t create_flag;
  SSL_CTX *context;
} swr_v1_tls_owner_fields;
typedef struct {
  const swr_v1_tls_owner *owner;
  swr_v1_tls_owner_fields *fields;
  const swr_v1_core_string *strings[11];
} swr_v1_tls_owner_prepared;
typedef struct {
  size_t size, alignment, offsets[15];
} swr_v1_tls_owner_layout_t;
swr_v1_tls_owner_layout_t swr_v1_tls_owner_layout(void);
void swr_v1_tls_info_log(void (*callback)(void));
swr_v1_tls_owner_prepared swr_v1_tls_owner_new(void (*warn)(const char *,
                                                            const char *));
const swr_v1_tls_owner *swr_v1_tls_owner_clone(const swr_v1_tls_owner *);
void swr_v1_tls_owner_free(const swr_v1_tls_owner *);
void swr_v1_tls_owner_bind(const swr_v1_tls_owner *, void *identity);
void swr_v1_tls_owner_detach(const swr_v1_tls_owner *);
bool swr_v1_tls_owner_create(const swr_v1_tls_owner *);
typedef struct {
  int32_t status;
  uint32_t index;
} swr_v1_tls_owner_copy;
swr_v1_tls_owner_copy swr_v1_tls_owner_assign(const swr_v1_tls_owner *,
                                              const swr_v1_tls_owner *);
bool swr_v1_tls_owner_set_capath(const swr_v1_tls_owner *);
bool swr_v1_tls_owner_set_ciphers(const swr_v1_tls_owner *);
bool swr_v1_tls_owner_set_client_certificate(const swr_v1_tls_owner *);
bool swr_v1_tls_owner_set_ecdh_curve(const swr_v1_tls_owner *);
bool swr_v1_tls_owner_set_dhparam(const swr_v1_tls_owner *);
int swr_v1_tls_owner_set_path(const swr_v1_tls_owner *, uint32_t, const char *,
                              size_t);
struct swr_v1_socket;
int swr_v1_tls_owner_socket_context(const swr_v1_tls_owner *,
                                    struct swr_v1_socket *, bool, bool);
typedef struct {
  int status;
  unsigned long error;
} swr_v1_tls_owner_session_result;
swr_v1_tls_owner_session_result swr_v1_tls_owner_socket_session(
    const swr_v1_tls_owner *, struct swr_v1_socket *,
    const swr_v1_core_string *, void (*)(SSL *, bool));
#ifdef __cplusplus
}
#endif
#endif
