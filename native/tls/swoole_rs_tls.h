/* SPDX-License-Identifier: Apache-2.0 */
#ifndef SWOOLE_RS_TLS_H
#define SWOOLE_RS_TLS_H
#include <stdbool.h>
#include <stddef.h>
#include <stdint.h>
#include <openssl/ssl.h>
#ifdef __cplusplus
extern "C" {
#endif

/* SSLContext: Rust makes every OpenSSL call that creates and configures the
 * SSL_CTX and owns its callbacks; native code keeps the configuration fields and
 * the info callback. Strings are a std::string's c_str() and length(), lent for
 * one call. warn logs "<function>(): <message>" at warning level. */
typedef struct swr_v1_tls_str { const char *data; size_t length; } swr_tls_str;
struct swr_v1_tls_config {
    swr_tls_str ciphers, ecdh_curve, dhparam, cert_file, key_file, passphrase, client_cert_file,
        cafile, capath;
    uint32_t protocols; /* create writes back the normalized mask */
    uint8_t verify_depth;
    bool http, http_v2, prefer_server_ciphers, disable_compress, verify_peer;
    void (*warn)(const char *function, const char *message);
    /* The SSLContext; the password callback reads its current passphrase through passphrase_of. */
    const void *owner;
    swr_tls_str (*passphrase_of)(const void *owner);
};
/* 0 ok or already, 1 OPENSSL_init_ssl failed, 2 SSL_get_ex_new_index failed. */
int swr_v1_tls_init(void);
void swr_v1_tls_destroy(void);
int swr_v1_tls_connection_index(void);
int swr_v1_tls_port_index(void);
/* The error queue's last line ("" when empty), drained; valid until this thread's next call. */
const char *swr_v1_tls_error(void);
/* A configured context, or null after logging why. */
SSL_CTX *swr_v1_tls_context_create(struct swr_v1_tls_config *config);
bool swr_v1_tls_set_capath(SSL_CTX *ctx, const struct swr_v1_tls_config *config);
bool swr_v1_tls_set_ciphers(SSL_CTX *ctx, const struct swr_v1_tls_config *config);
bool swr_v1_tls_set_client_certificate(SSL_CTX *ctx, const struct swr_v1_tls_config *config);
bool swr_v1_tls_set_ecdh_curve(SSL_CTX *ctx, const struct swr_v1_tls_config *config);
bool swr_v1_tls_set_dhparam(SSL_CTX *ctx, const struct swr_v1_tls_config *config);

/* DTLS BIO decisions: copied scalars only. */
struct swr_v1_tls_read_plan { int32_t length; uint8_t consume, retry; };
struct swr_v1_tls_control_plan { long result; uint32_t effect; };
/* Native OpenSSL command -> module-local tag; observed value only for tags0..2.
 * 0 EOF,1 GET_CLOSE,2 PENDING,3 SET_CLOSE,4 success,5 GET_PEER,6 MTU,
 * 7 overhead,8 PEEK,9 zero, other unknown. Effects0 none/1 shutdown/2 peer/3 peek/4 warning. */
/* Negative failure0; zero pending1; positive publish2. BoringSSL keeps native SSL_is_dtls. */
#ifdef __cplusplus
}
#endif
#endif
