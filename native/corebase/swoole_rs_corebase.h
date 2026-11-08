/* SPDX-License-Identifier: Apache-2.0 */
#ifndef SWOOLE_RS_COREBASE_H
#define SWOOLE_RS_COREBASE_H
#include <stddef.h>
#include <stdint.h>
#include <stdbool.h>
#include <sys/types.h>
#ifdef __cplusplus
extern "C" {
#endif
/* Success matches the representable decimal portion of std::stoi; native code
 * retains its exception path and publishes output only after success. */
uint8_t swr_v1_corebase_parse_stoi(const char *input, int32_t *output);
/* Valid NUL-terminated input, disjoint writable parsed output. Original host
 * ctype input requirements apply. These direct libc parsers preserve original errno effects/order. */
/* Frozen target-default mode; native compiler char mode must match Rust. */
unsigned long swr_v1_corebase_hex(const char *input, size_t *parsed);
/* Actual native callers supply their compiler plain-char mode. */
unsigned long swr_v1_corebase_hex_mode(const char *input, size_t *parsed, uint8_t native_char_signed);
/* Stack-owned state; zero initialize once, keep state/output unchanged between
 * steps. No pointer retained; native IO occurs after return. Stop on !=1.
 * Return 0 done, 1 NUL-cut prefix ready, 2 path too long, 3 invalid arguments.
 * Input/output/state are disjoint; capacity <= PTRDIFF_MAX. */
struct swr_v1_corebase_mkdir_state {
    size_t length;
    size_t next;
};
int32_t swr_v1_corebase_mkdir_next(const uint8_t *input, size_t input_length, uint8_t *output,
                                 size_t capacity, struct swr_v1_corebase_mkdir_state *state);
/* swoole_fork_exec's fork: the child runs call(function) and exits 0; the parent
 * gets the pid, 0 when fork fails. call may throw. Rust also defines swoole_fork,
 * swoole_clean, swoole_thread_init/_clean and swoole_is_main_thread. */
pid_t swr_v1_core_fork_exec(void (*call)(const void *function), const void *function);
/* Rust defines these upstream functions under their C++ symbols, so swoole.h and
 * native/framing/swoole_protocol.h stay their declarations: sw_malloc/sw_free/
 * sw_calloc/sw_realloc (libc's allocator), swoole_common_divisor/_multiple,
 * swoole_version_compare, swoole_dec2hex, swoole_type_size, swoole_itoa,
 * swoole_random_string(char *, size_t), swoole_rand
 * (both overloads), swoole_random_int, swoole::microtime and
 * swoole::DataHead::dump. */
/* Also Rust's, from base.cc: swoole_shell_exec, swoole_system_random, both
 * swoole_redirect_stdout overloads, swoole_print_backtrace(_on_error),
 * swoole_exit and swoole_get_systemd_listen_fds. */
/* Process-wide core state: upstream's SwooleG scalars. swoole::Global's members
 * of these names are swoole::RustField views over the accessor pairs below.
 * Pointers (memory pool, stdout FILE *, Logger *, Server *, the AIO pipe
 * Socket *, the fatal-error and backtrace hooks) are native objects Rust only
 * holds. swr_v1_core_init returns false when already initialised; otherwise it
 * resets the record and sets running, enable_coroutine, the page size, the
 * file-content cap, the CPU count, DNS tries and the random() seed.
 * swr_v1_core_max_sockets: RLIMIT_NOFILE's soft limit as uint32_t, capped at
 * limit; -1 (errno kept) when getrlimit fails. */
bool swr_v1_core_init(void);
void swr_v1_core_reset(void);
int64_t swr_v1_core_max_sockets(uint32_t limit);
bool swr_v1_core_initialized(void);
uint16_t swr_v1_core_cpu_num(void);
int swr_v1_core_dns_tries(void);
bool swr_v1_core_running(void);
void swr_v1_core_set_running(bool value);
bool swr_v1_core_enable_signalfd(void);
void swr_v1_core_set_enable_signalfd(bool value);
bool swr_v1_core_enable_kqueue(void);
void swr_v1_core_set_enable_kqueue(bool value);
bool swr_v1_core_enable_coroutine(void);
void swr_v1_core_set_enable_coroutine(bool value);
bool swr_v1_core_print_backtrace_on_error(void);
void swr_v1_core_set_print_backtrace_on_error(bool value);
bool swr_v1_core_wait_signal(void);
void swr_v1_core_set_wait_signal(bool value);
bool swr_v1_core_dns_lookup_random(void);
void swr_v1_core_set_dns_lookup_random(bool value);
long swr_v1_core_current_task_id(void);
void swr_v1_core_set_current_task_id(long value);
int swr_v1_core_signal_fd(void);
void swr_v1_core_set_signal_fd(int value);
bool swr_v1_core_signal_alarm(void);
void swr_v1_core_set_signal_alarm(bool value);
uint32_t swr_v1_core_signal_listener_num(void);
void swr_v1_core_set_signal_listener_num(uint32_t value);
uint32_t swr_v1_core_signal_async_listener_num(void);
void swr_v1_core_set_signal_async_listener_num(uint32_t value);
long swr_v1_core_trace_flags(void);
void swr_v1_core_set_trace_flags(long value);
uint32_t swr_v1_core_pagesize(void);
void swr_v1_core_set_pagesize(uint32_t value);
uint32_t swr_v1_core_max_file_content(void);
void swr_v1_core_set_max_file_content(uint32_t value);
uint32_t swr_v1_core_aio_core_worker_num(void);
void swr_v1_core_set_aio_core_worker_num(uint32_t value);
uint32_t swr_v1_core_aio_worker_num(void);
void swr_v1_core_set_aio_worker_num(uint32_t value);
uint32_t swr_v1_core_iouring_entries(void);
void swr_v1_core_set_iouring_entries(uint32_t value);
uint32_t swr_v1_core_iouring_workers(void);
void swr_v1_core_set_iouring_workers(uint32_t value);
uint32_t swr_v1_core_iouring_flag(void);
void swr_v1_core_set_iouring_flag(uint32_t value);
double swr_v1_core_aio_max_wait_time(void);
void swr_v1_core_set_aio_max_wait_time(double value);
double swr_v1_core_aio_max_idle_time(void);
void swr_v1_core_set_aio_max_idle_time(double value);
void *swr_v1_core_memory_pool(void);
void swr_v1_core_set_memory_pool(void *value);
void *swr_v1_core_stdout(void);
void swr_v1_core_set_stdout(void *value);
void *swr_v1_core_logger(void);
void swr_v1_core_set_logger(void *value);
void *swr_v1_core_server(void);
void swr_v1_core_set_server(void *value);
void *swr_v1_core_aio_default_socket(void);
void swr_v1_core_set_aio_default_socket(void *value);
void *swr_v1_core_fatal_error(void);
void swr_v1_core_set_fatal_error(void *value);
void *swr_v1_core_print_backtrace(void);
void swr_v1_core_set_print_backtrace(void *value);
/* SwooleG.bug_report_message: c_str() valid until it next changes; append
 * text; init writes base.cc's banner, uname, compiler (null: none) and TLS
 * lines. Strings are NUL-terminated. */
const char *swr_v1_core_bug_report_message(void);
void swr_v1_core_bug_report_append(const char *text);
void swr_v1_core_bug_report_init(const char *banner, const char *compiler, const char *ssl);
/* crates/corebase/src/format.c: vfprintf to SwooleG.stdout_. No compiled source
 * calls it; core-tests/src/_lib/ssl.cpp does. */
int sw_printf(const char *format, ...) __attribute__((format(printf, 1, 2)));
#ifdef __cplusplus
}
#include <cstdio>
#include <functional>
#include <string>
namespace swoole {
/* SwooleG.name_resolvers' element; native/clients/dns.cc runs them. */
struct NameResolver {
    enum Type { TYPE_KERNEL, TYPE_PHP, TYPE_USER };
    struct Context {
        int type;
        double timeout;
        void *private_data;
        bool with_port;
        bool cluster_;
        bool final_;
        std::function<void(Context *ctx)> dtor;
        ~Context() {
            if (private_data && dtor) dtor(this);
        }
    };
    std::function<std::string(const std::string &, Context *, void *)> resolve;
    void *private_data;
    Type type;
};
}  // namespace swoole
/* Upstream's error and log API under its C++ symbols: Rust's (crates/error,
 * crates/utilities, crates/corebase). */
const char *swoole_strerror(int code);
void swoole_throw_error(int code);
void swoole_ignore_error(int code);
bool swoole_is_ignored_error(int code);
bool swoole_is_main_thread();
void swoole_set_log_level(int level);
void swoole_set_log_file(const char *file);
int swoole_get_log_level();
void swoole_set_trace_flags(long flags);
void swoole_set_print_backtrace_on_error(bool enable = true);
void swoole_set_stdout_stream(FILE *fp);
FILE *swoole_get_stdout_stream();
#endif
#endif
