#ifndef SWOOLE_RS_TLS_CONSTANTS_H
#define SWOOLE_RS_TLS_CONSTANTS_H

/* Generated from crates/tls by cbindgen (tools/headers.sh); do not edit. */

#include <stdint.h>
#include "swoole_rs_corebase_constants.h"
#define SW_SSL_ALL (SW_SSL_SSLv2 | SW_SSL_SSLv3 | SW_SSL_TLSv1 | SW_SSL_TLSv1_1 | SW_SSL_TLSv1_2 | SW_SSL_TLSv1_3)

typedef enum swSSLCreateFlag {
  SW_SSL_SERVER = 1,
  SW_SSL_CLIENT = 2,
} swSSLCreateFlag;

typedef enum swSSLVersion {
  SW_SSL_SSLv2 = (1 << 1),
  SW_SSL_SSLv3 = (1 << 2),
  SW_SSL_TLSv1 = (1 << 3),
  SW_SSL_TLSv1_1 = (1 << 4),
  SW_SSL_TLSv1_2 = (1 << 5),
  SW_SSL_TLSv1_3 = (1 << 6),
  SW_SSL_DTLS = (1 << 7),
} swSSLVersion;

typedef enum swSSLMethod {
  SW_SSLv23_METHOD = 0,
  SW_SSLv3_METHOD = 1,
  SW_SSLv3_SERVER_METHOD = 2,
  SW_SSLv3_CLIENT_METHOD = 3,
  SW_SSLv23_SERVER_METHOD = 4,
  SW_SSLv23_CLIENT_METHOD = 5,
  SW_TLSv1_METHOD = 6,
  SW_TLSv1_SERVER_METHOD = 7,
  SW_TLSv1_CLIENT_METHOD = 8,
  SW_TLSv1_1_METHOD = 9,
  SW_TLSv1_1_SERVER_METHOD = 10,
  SW_TLSv1_1_CLIENT_METHOD = 11,
  SW_TLSv1_2_METHOD = 12,
  SW_TLSv1_2_SERVER_METHOD = 13,
  SW_TLSv1_2_CLIENT_METHOD = 14,
  SW_DTLS_CLIENT_METHOD = 15,
  SW_DTLS_SERVER_METHOD = 16,
} swSSLMethod;



#endif  /* SWOOLE_RS_TLS_CONSTANTS_H */
