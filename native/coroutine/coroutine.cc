// SPDX-License-Identifier: Apache-2.0
// Compatibility derived from Swoole v6.2.4 src/coroutine/base.cc (Swoole contributors).
// The build's coroutine switches, which only a configured C++ compile sees;
// crates/coroutine reads them and owns the rest of swoole::Coroutine (facade.rs).
#include "swoole_coroutine.h"

const uint32_t swr_cpp_coroutine_build = 0
#ifdef SW_USE_THREAD_CONTEXT
                                         | SWR_CTX_THREAD
#endif
#ifdef SW_CONTEXT_PROTECT_STACK_PAGE
                                         | SWR_CTX_STACK_PROTECT
#endif
#ifdef SW_CONTEXT_DETECT_STACK_USAGE
                                         | SWR_CTX_STACK_MARK
#endif
#ifdef SW_CORO_TIME
                                         | SWR_CO_TIME
#endif
    ;
