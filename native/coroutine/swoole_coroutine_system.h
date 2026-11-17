/* SPDX-License-Identifier: Apache-2.0 */
/* Coroutine process waits used by the native callers. */
#pragma once

#include "swoole_coroutine.h"

namespace swoole {
namespace coroutine {
class System {
  public:
    static pid_t waitpid(pid_t _pid, int *_stat_loc, int _options, double timeout = -1);
    static pid_t waitpid_safe(pid_t _pid, int *_stat_loc, int _options);
};
}  // namespace coroutine
}  // namespace swoole
