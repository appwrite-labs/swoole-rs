// SPDX-License-Identifier: Apache-2.0
// The std::shared_ptr<SSLContext> holders and DTLS sessions crates/ports/src/tls_state.rs
// keeps for a ListenPort: constructing and destroying them is C++, which Rust can't do.
#include "swoole_server.h"

using swoole::SSLContext;
using Held = std::shared_ptr<SSLContext>;

// A new default context, in `context`, and its holder.
extern "C" void *swr_v1_ports_context_new(SSLContext **context) {
    auto *held = new Held(std::make_shared<SSLContext>());
    *context = held->get();
    return held;
}

extern "C" void swr_v1_ports_context_free(void *held) {
    delete static_cast<Held *>(held);
}

// ListenPort::create_dtls_session's session over `sock`, sharing the held context; null when
// it fails to initialize.
extern "C" void *swr_v1_ports_dtls_session_new(void *held, swoole::network::Socket *sock) {
#ifdef SW_SUPPORT_DTLS
    auto *session = new swoole::dtls::Session(sock, *static_cast<Held *>(held));
    if (!session->init()) {
        delete session;
        return nullptr;
    }
    return session;
#else
    return nullptr;
#endif
}
