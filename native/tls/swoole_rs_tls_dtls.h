/* SPDX-License-Identifier: Apache-2.0 */
#pragma once
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
extern "C" {
swr_v1_tls_dtls_prepared swr_v1_tls_dtls_new(swr_v1_socket *socket);
void swr_v1_tls_dtls_clear(swr_v1_tls_dtls_session *owner);
void swr_v1_tls_dtls_free(swr_v1_tls_dtls_session *owner);
int swr_v1_tls_dtls_assign(swr_v1_tls_dtls_session *destination, const swr_v1_tls_dtls_session *source);
int swr_v1_tls_dtls_append_buffer(swr_v1_tls_dtls_session *owner, void *buffer);
int swr_v1_tls_dtls_append(swr_v1_tls_dtls_session *owner, const uint8_t *data, size_t length);
size_t swr_v1_tls_dtls_length(const swr_v1_tls_dtls_session *owner);
/* Session::init and Session::listen; context is the session's SSLContext. */
bool swr_v1_tls_dtls_init(swr_v1_tls_dtls_session *owner, const void *context);
bool swr_v1_tls_dtls_session_listen(swr_v1_tls_dtls_session *owner);
}

static_assert(sizeof(swr_v1_tls_dtls_fields) == 16 &&
              offsetof(swr_v1_tls_dtls_fields, listened) == 8 &&
              offsetof(swr_v1_tls_dtls_fields, peek) == 9, "DTLS canonical scalar ABI");
