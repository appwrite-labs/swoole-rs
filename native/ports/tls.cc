// SPDX-License-Identifier: Apache-2.0
// The std::shared_ptr<SSLContext> holders crates/ports/src/lifecycle_native.rs
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

// Another holder of `held`'s context, for a DTLS session that shares it.
extern "C" void *swr_v1_ports_context_share(void *held) {
    return new Held(*static_cast<Held *>(held));
}
