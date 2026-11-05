/* SPDX-License-Identifier: Apache-2.0 */
#pragma once
#include "swoole_rs_tls.h"
#include <openssl/bio.h>
#include <stddef.h>
#include <stdint.h>
namespace swoole { namespace network { struct Socket; } }
struct swr_v1_socket;
struct swr_v1_tls_dtls_session;
struct swr_v1_tls_dtls_fields {
    swoole::network::Socket *socket;
    bool listened;
    bool peek;
};
struct swr_v1_tls_dtls_prepared {
    swr_v1_tls_dtls_session *owner;
    swr_v1_tls_dtls_fields *fields;
};
struct swr_v1_tls_dtls_read_view {
    uint16_t length;
    bool empty;
    bool peek;
};
extern "C" {
swr_v1_tls_dtls_prepared swr_v1_tls_dtls_new(swr_v1_socket *socket);
void swr_v1_tls_dtls_clear(swr_v1_tls_dtls_session *owner);
void swr_v1_tls_dtls_free(swr_v1_tls_dtls_session *owner);
int swr_v1_tls_dtls_assign(swr_v1_tls_dtls_session *destination, const swr_v1_tls_dtls_session *source);
int swr_v1_tls_dtls_append_buffer(swr_v1_tls_dtls_session *owner, void *buffer);
int swr_v1_tls_dtls_append(swr_v1_tls_dtls_session *owner, const uint8_t *data, size_t length);
swr_v1_tls_dtls_read_view swr_v1_tls_dtls_view(const swr_v1_tls_dtls_session *owner);
swr_v1_tls_read_plan swr_v1_tls_dtls_read(swr_v1_tls_dtls_session *owner, uint8_t *data, int requested);
size_t swr_v1_tls_dtls_length(const swr_v1_tls_dtls_session *owner);
swr_v1_tls_control_plan swr_v1_tls_dtls_control(swr_v1_tls_dtls_session *owner, uint32_t tag, long observed, long value, void *ptrval);
uint32_t swr_v1_tls_dtls_listen(swr_v1_tls_dtls_session *owner, int result);
BIO_METHOD *swr_v1_tls_dtls_methods(void);
BIO_METHOD *swr_v1_tls_dtls_methods_publish(BIO_METHOD *candidate);
BIO_METHOD *swr_v1_tls_dtls_methods_take(void);
}

static_assert(sizeof(swr_v1_tls_dtls_fields) == 16 &&
              offsetof(swr_v1_tls_dtls_fields, listened) == 8 &&
              offsetof(swr_v1_tls_dtls_fields, peek) == 9, "DTLS canonical scalar ABI");
