/* SPDX-License-Identifier: Apache-2.0 */
/* Swoole's include/swoole_buffer.h, owned here: the socket record's output queue, one
 * Rust queue handle whose members only Rust calls (crates/corebase/src/buffer/cpp.rs). Upstream's
 * file is deleted, so `#include "swoole_buffer.h"` finds this copy on the Rust prefix's
 * include path. */
#pragma once

#include <queue>
#include "swoole_rs_buffer.h"
#include <sys/uio.h>

namespace swoole {
class Buffer;
}  // namespace swoole
