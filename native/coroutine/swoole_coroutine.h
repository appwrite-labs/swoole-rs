/* SPDX-License-Identifier: Apache-2.0 */
/* Swoole's include/swoole_coroutine.h, owned here: what native/ and the vendored code
 * still call of swoole::Coroutine. Rust owns the scheduler and each coroutine's record
 * (crates/coroutine); the operations are its swr_v1_coroutine_* functions and upstream
 * symbols. Upstream's coroutine headers are deleted, so `#include "swoole_coroutine.h"`
 * finds this copy on the Rust prefix's include path. */
#pragma once

#include "swoole_string.h"
#include "swoole_socket.h"
#include "swoole_reactor.h"
#include "swoole_async.h"
#include "swoole_util.h"

#include "swoole_rs_coroutine.h"

static_assert(sizeof(void *) != 8 || (sizeof(swr_v1_coroutine) == 96 && sizeof(swr_v1_coroutine_scheduler) == 136),
              "crates/coroutine layout");

namespace swoole {
// A coroutine's record (a swr_v1_coroutine, then crates/coroutine's Context), which C++
// only points to.
class Coroutine {
  public:
    Coroutine() = delete;

    static Coroutine *get_current() {
        swr_v1_coroutine *co = swr_v1_coroutine_scheduler_get()->current;
        return co ? static_cast<Coroutine *>(co->native) : nullptr;
    }

    static Coroutine *get_current_safe() {
        Coroutine *co = get_current();
        if (sw_unlikely(!co)) {
            swoole_fatal_error(SW_ERROR_CO_OUT_OF_COROUTINE, "API must be called in the coroutine");
        }
        return co;
    }

    static long get_socket_bound_cid() {
        return swr_v1_coroutine_scheduler_get()->socket_bound_cid;
    }
};
}  // namespace swoole
