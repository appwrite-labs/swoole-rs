/* SPDX-License-Identifier: Apache-2.0 */
#ifndef SWOOLE_RS_COREBASE_H
#define SWOOLE_RS_COREBASE_H
#include <stdint.h>
#include <stdbool.h>
#ifdef __cplusplus
extern "C" {
#endif
/* Success matches the representable decimal portion of std::stoi; native code
 * retains its exception path and publishes output only after success. */
uint8_t swr_v1_corebase_parse_stoi(const char *input, int32_t *output);
/* Rust also defines swoole_fork, swoole_clean, swoole_thread_init/_clean and
 * swoole_is_main_thread. */
/* swoole_init's body (crates/corebase/src/lifecycle.rs), which crates/ffi's
 * swoole_init calls once it has registered the leaves. */
void swr_v1_core_swoole_init(void);
/* Rust defines these upstream functions under their C++ symbols, so swoole.h and
 * native/framing/swoole_protocol.h stay their declarations: sw_malloc/sw_free/
 * sw_calloc (libc's allocator), swoole_common_divisor/_multiple,
 * swoole_version_compare, swoole_dec2hex, swoole_type_size, swoole_itoa, swoole_rand
 * (both overloads), swoole_random_int, swoole::microtime and
 * swoole::DataHead::dump. */
/* Also Rust's, from base.cc: swoole_shell_exec, swoole_system_random, both
 * swoole_redirect_stdout overloads, swoole_print_backtrace(_on_error),
 * swoole_exit, swoole_set_log_level, swoole_set/get_stdout_stream and sw_printf
 * (crates/corebase/src/format.c). No compiled C++ calls them. */
/* Process-wide core state: upstream's SwooleG scalars that C++ reads. swoole::Global's
 * members of these names are swoole::RustField views over the accessor pairs below.
 * Pointers (the fatal-error hook) are native objects Rust only holds. */
uint16_t swr_v1_core_cpu_num(void);
bool swr_v1_core_running(void);
void swr_v1_core_set_running(bool value);
bool swr_v1_core_enable_coroutine(void);
void swr_v1_core_set_enable_coroutine(bool value);
long swr_v1_core_trace_flags(void);
uint32_t swr_v1_core_pagesize(void);
void swr_v1_core_set_pagesize(uint32_t value);
void *swr_v1_core_fatal_error(void);
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
 * crates/utilities, crates/corebase). */
const char *swoole_strerror(int code);
void swoole_throw_error(int code);
void swoole_ignore_error(int code);
bool swoole_is_ignored_error(int code);
bool swoole_is_main_thread();
int swoole_get_log_level();
void swoole_set_trace_flags(long flags);
void swoole_set_print_backtrace_on_error(bool enable = true);
#endif
#endif
