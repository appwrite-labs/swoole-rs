/* SPDX-License-Identifier: Apache-2.0 */
/* Swoole's include/swoole_coroutine_system.h, owned here: the System members C++ still
 * calls. sleep, init_reactor and the waitpids are Rust under upstream's symbols
 * (crates/hooks, crates/process); write_file, gethostbyname and
 * gethostbyname_impl_with_async are native/hooks/system.cc. */
#pragma once

#include "swoole_coroutine.h"

namespace swoole {
namespace coroutine {
class System {
  public:
    static void init_reactor(Reactor *reactor);
    static int sleep(double sec);
    static ssize_t write_file(const char *file, const char *buf, size_t length, bool lock = false, int flags = 0);
    static std::string gethostbyname(const std::string &hostname, int domain, double timeout = -1);
    static pid_t waitpid(pid_t _pid, int *_stat_loc, int _options, double timeout = -1);
    static pid_t waitpid_safe(pid_t _pid, int *_stat_loc, int _options);
};
std::string gethostbyname_impl_with_async(const std::string &hostname, int domain, double timeout = -1);
}  // namespace coroutine
}  // namespace swoole
