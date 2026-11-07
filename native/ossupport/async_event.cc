// SPDX-License-Identifier: Apache-2.0
// What stable Rust can't express of AsyncThreads (crates/hooks/src/async_threads.rs):
// an AsyncEvent's request is a std::shared_ptr, so copying and freeing one is C++.
// The copy is null when it cannot allocate.
#include "swoole_async.h"

#include <new>

extern "C" swoole::AsyncEvent *swr_v1_os_async_event_copy(const swoole::AsyncEvent *event) {
    return new (std::nothrow) swoole::AsyncEvent(*event);
}

extern "C" void swr_v1_os_async_event_free(swoole::AsyncEvent *event) {
    delete event;
}
