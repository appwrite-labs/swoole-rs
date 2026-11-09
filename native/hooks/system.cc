// SPDX-License-Identifier: Apache-2.0
// Compatibility derived from Swoole v6.2.4 src/coroutine/system.cc (Swoole contributors).
// What crates/hooks can't express of System and coroutine::Socket (upstream's
// src/coroutine/system.cc is deleted): configure's io_uring switches. Registering
// them installs Rust's socket host too. native/hooks/contract.md lists what stays and why.
#include "swoole_rs_hooks.h"
#include "swoole_rs_uring.h"
#include "swoole_rs_coroutine_socket.h"

static_assert(SW_EVENT_READ == 1 << 9 && SW_EVENT_WRITE == 1 << 10, "swr_v1_hook_socket_* event bits");
static_assert(SW_FD_CO_SOCKET == 6 && SW_SOCK_RAW == 7 && SW_BUFFER_SIZE_BIG == 65536 &&
                  SW_INPUT_BUFFER_SIZE == 2 * 1024 * 1024,
              "swr_v1_hook_co_socket constants");

// hook.cc's socket functions are Rust (crates/hooks/src/hooked.rs), as are the hooked
// file, sleep and resolver functions (stdio.rs); this is what they need of native: the
// io_uring engine and configure's io_uring switches.
static const swr_v1_hook_host hook_host = {
#ifdef SW_USE_IOURING
    &swr_v1_uring_native,
#else
    nullptr,
#endif
#ifdef HAVE_IOURING_STATX
    true,
#else
    false,
#endif
#ifdef HAVE_IOURING_FTRUNCATE
    true,
#else
    false,
#endif
#ifdef HAVE_IOURING_FUTEX
    true,
#else
    false,
#endif
#ifdef SW_USE_URING_SOCKET
    true,
#else
    false,
#endif
};

[[maybe_unused]] static const bool hook_host_set = (swr_v1_hook_set_host(&hook_host), true);
