// SPDX-License-Identifier: Apache-2.0
// M15's switch, which its config.m4 patch defines in --fix-halfclose builds: selects
// the repaired Socket::ssl_shutdown and handle_sendfile in crates/tls (tls.md).
#include "swoole.h"
#include "swoole_rs_reactor.h"

#ifdef SW_HALFCLOSE_PREQUEUE
[[maybe_unused]] static const bool halfclose_socket_registered = (swr_v1_socket_set_halfclose_repairs(true), true);
#endif
