/* SPDX-License-Identifier: Apache-2.0 */
#ifndef SWOOLE_RS_COREBASE_H
#define SWOOLE_RS_COREBASE_H
#include <stdint.h>
#include <stdbool.h>
#ifdef __cplusplus
extern "C" {
#endif
/* Rust also defines swoole_init, swoole_fork and swoole_clean.
 * Thread initialization, cleanup and main-thread queries use Rust paths. */
/* Rust defines these upstream functions under their C++ symbols, so swoole.h
 * keeps their declarations: swoole_common_divisor/_multiple, swoole_version_compare,
 * swoole_dec2hex, swoole_random_int and swoole::DataHead::dump. swoole::microtime is Rust's too;
 * no C++ calls it. */
/* Also Rust's, from base.cc: swoole_shell_exec, swoole_system_random, both
 * swoole_redirect_stdout overloads, swoole_print_backtrace(_on_error),
 * swoole_exit, swoole_set/get_stdout_stream and sw_printf
 * (crates/corebase/src/format.c). No compiled C++ calls them. */
/* SwooleG.fatal_error, a native object Rust only holds: null until a host sets it. */
typedef void (*SwrFatalError)(int code, const char *format, ...);
SwrFatalError swr_v1_core_fatal_error(void);
#ifdef __cplusplus
}
namespace swoole {
/* The name resolvers' types. The list and the Context, which native code only
 * points to, are crates/corebase/src/name_resolver.rs (swr_v1_core_name_resolver_*). */
struct NameResolver {
    struct Context;
};
}  // namespace swoole
/* Upstream's error and log API under its C++ symbols: Rust's (crates/error,
 * crates/corebase). */
const char *swoole_strerror(int code);
void swoole_throw_error(int code);
void swoole_ignore_error(int code);
bool swoole_is_ignored_error(int code);
void swoole_set_trace_flags(long flags);
void swoole_set_print_backtrace_on_error(bool enable = true);
#endif
#endif
