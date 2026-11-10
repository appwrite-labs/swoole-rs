/* SPDX-License-Identifier: Apache-2.0 */
/* Swoole's include/swoole_signal.h, owned here: the signal functions C++ still calls.
 * They are Rust under their upstream symbols (crates/reactor/src/signal_upstream.rs,
 * crates/process/src/signal.rs). */
#pragma once

#include "swoole.h"

#include <csignal>

namespace swoole {
typedef void (*SignalHandler)(int);
}  // namespace swoole

/* `safety` defers the handler to swoole_signal_dispatch(), outside the signal frame. */
SW_API swoole::SignalHandler swoole_signal_set(int signo, swoole::SignalHandler func, bool safety = false);
SW_API swoole::SignalHandler swoole_signal_set(int signo, swoole::SignalHandler func, int restart, int mask);
SW_API swoole::SignalHandler swoole_signal_get_handler(int signo);
SW_API void swoole_signal_clear();
// Returns true only when this call saves and changes the mask; that caller must restore it with unblock_all().
SW_API bool swoole_signal_block_all();
SW_API void swoole_signal_dispatch();
