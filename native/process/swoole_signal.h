/* SPDX-License-Identifier: Apache-2.0 */
/* Swoole's include/swoole_signal.h, owned here: the signal functions C++ still calls.
 * They are Rust under their upstream symbols (crates/process/src/signal.rs). */
#pragma once

#include "swoole.h"

namespace swoole {
typedef void (*SignalHandler)(int);
}  // namespace swoole

SW_API swoole::SignalHandler swoole_signal_set(int signo, swoole::SignalHandler func, int restart, int mask);
// Returns true only when this call saves and changes the mask; that caller must restore it with unblock_all().
SW_API bool swoole_signal_block_all();
