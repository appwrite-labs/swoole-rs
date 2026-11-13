/* SPDX-License-Identifier: Apache-2.0 */
// The TLS switches C++ reads: OpenSSL's header macros (corebase/switches.cc) and the TLS
// constants (swoole_socket.h). crates/tls owns SSLContext and defines swoole_ssl_* under
// their upstream symbols.
#pragma once

#include "swoole.h"

#include <openssl/ssl.h>
#include "swoole_rs_tls_constants.h"

#if OPENSSL_VERSION_MAJOR < 3
#error "OpenSSL 3 or later is required"
#endif

// OpenSSL 3 always has DTLS.
#define SW_SUPPORT_DTLS
