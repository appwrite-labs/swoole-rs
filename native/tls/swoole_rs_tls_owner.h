/* SPDX-License-Identifier: Apache-2.0 */
#ifndef SWOOLE_RS_TLS_OWNER_H
#define SWOOLE_RS_TLS_OWNER_H
#include "swoole_rs_corebase_string_slot.h"
#include <stdbool.h>
#include <stddef.h>
#include <stdint.h>
#include <openssl/ssl.h>
#if OPENSSL_VERSION_MAJOR < 3
#error "OpenSSL 3 or later is required"
#endif
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
/* crates/tls/src/owned.rs asserts the same layout. */
static_assert(sizeof(swr_v1_tls_owner_fields) == 32 &&
                  offsetof(swr_v1_tls_owner_fields, allow_self_signed) == 11 &&
                  offsetof(swr_v1_tls_owner_fields, protocols) == 12 &&
                  offsetof(swr_v1_tls_owner_fields, create_flag) == 16 &&
                  offsetof(swr_v1_tls_owner_fields, context) == 24,
              "Rust OwnedTlsFields layout");
typedef struct {
  const swr_v1_tls_owner *owner;
  swr_v1_tls_owner_fields *fields;
  const swr_v1_core_string *strings[11];
} swr_v1_tls_owner_prepared;
/* Logs "<function>(): <message>" at warning level. */
void swr_v1_tls_warning(const char *function, const char *message);
swr_v1_tls_owner_prepared swr_v1_tls_owner_new(void (*warn)(const char *,
                                                            const char *));
/* Drops one reference; the last releases the owner. */
void swr_v1_tls_owner_free(const swr_v1_tls_owner *);
/* The owner's fields and string slots. */
swr_v1_tls_owner_prepared swr_v1_tls_owner_view(const swr_v1_tls_owner *);
int swr_v1_tls_owner_set_path(const swr_v1_tls_owner *, uint32_t, const char *,
                              size_t);
#ifdef __cplusplus
}
#endif
#endif
