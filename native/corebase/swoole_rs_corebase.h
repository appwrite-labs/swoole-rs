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
/* Nonempty input is readable for length bytes; length <= PTRDIFF_MAX.
 * Returns prefix length; native std::string construction happens afterward. */
size_t swr_v1_corebase_dirname(const uint8_t *input, size_t length);
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
/* Zeroed stack cursor, never a native owner/registry. Execute each returned
 * action after Rust returns, then supply a copied boolean when requested.
 * 0 finishes; UINT32_MAX rejects invalid cursor/operation. No retained pointer.
 * THREAD_INIT argument is main-thread bool; THREAD_CLEAN ignores it;
 * CLEAN argument is native hook-slot count. cursor.index selects hook slot.
 * FORK/FORK_EXEC use pinned native flags; after FORK_CALL, observed is
 * 0=failure, 1=child, 2=parent. Other observations are copied booleans.
 * Preserve operation/argument and stop on DONE/invalid. No Rust frame spans
 * fork or native allocation/callback.  */
struct swr_v1_corebase_lifecycle_state { uint32_t phase; uint32_t index; };
enum swr_v1_corebase_lifecycle_operation {
    SWR_COREBASE_THREAD_INIT = 1, SWR_COREBASE_THREAD_CLEAN = 2, SWR_COREBASE_CLEAN = 3,
    SWR_COREBASE_FORK = 4, SWR_COREBASE_FORK_EXEC = 5
};
enum swr_v1_corebase_lifecycle_action {
    SWR_COREBASE_DONE = 0, SWR_COREBASE_OBSERVE_BUFFER = 1,
    SWR_COREBASE_ALLOCATE_BUFFER = 2, SWR_COREBASE_BLOCK_SIGNALS = 3,
    SWR_COREBASE_PUBLISH_MAIN = 4, SWR_COREBASE_OBSERVE_TIMER = 5,
    SWR_COREBASE_FREE_TIMER = 6, SWR_COREBASE_OBSERVE_REACTOR = 7,
    SWR_COREBASE_FREE_REACTOR = 8, SWR_COREBASE_DELETE_BUFFER = 9,
    SWR_COREBASE_OBSERVE_HOOK = 10, SWR_COREBASE_DELETE_HOOK = 11,
    SWR_COREBASE_CLEAR_SIGNALS = 12, SWR_COREBASE_CLEAN_THREAD = 13,
    SWR_COREBASE_OBSERVE_LOGGER = 14, SWR_COREBASE_CLOSE_LOGGER = 15,
    SWR_COREBASE_DELETE_LOGGER = 16, SWR_COREBASE_DELETE_POOL = 17,
    SWR_COREBASE_RESET_GLOBAL = 18, SWR_COREBASE_OBSERVE_COROUTINE = 19,
    SWR_COREBASE_REJECT_COROUTINE = 20, SWR_COREBASE_OBSERVE_ASYNC = 21,
    SWR_COREBASE_REJECT_ASYNC = 22, SWR_COREBASE_FORK_CALL = 23,
    SWR_COREBASE_WARN_FORK = 24, SWR_COREBASE_SLEEP_RETRY = 25,
    SWR_COREBASE_REPLACE_POOL = 26, SWR_COREBASE_REOPEN_LOGGER = 27,
    SWR_COREBASE_FREE_FORK_REACTOR = 28, SWR_COREBASE_OBSERVE_AFTER_FORK = 29,
    SWR_COREBASE_CALL_AFTER_FORK = 30, SWR_COREBASE_EXEC_CALLBACK = 31,
    SWR_COREBASE_EXIT_CHILD = 32, SWR_COREBASE_FAILED_EXEC = 33,
    SWR_COREBASE_OBSERVE_TASK_BYTE = 47, SWR_COREBASE_WARN_TASK_ABSOLUTE = 48,
    SWR_COREBASE_ACCESS_TASK = 49, SWR_COREBASE_MKDIR_TASK = 50,
    SWR_COREBASE_WARN_TASK_CREATE = 51, SWR_COREBASE_PUBLISH_TASK = 52,
    SWR_COREBASE_OBSERVE_TASK_LENGTH = 53, SWR_COREBASE_WARN_TASK_LONG = 54,
    SWR_COREBASE_REJECT_TASK = 55, SWR_COREBASE_ACCEPT_TASK = 56
};
uint32_t swr_v1_corebase_lifecycle_next(uint32_t operation,
    struct swr_v1_corebase_lifecycle_state *state, uint64_t observed, uint32_t argument);
/* Task directory: zeroed exclusive cursor; copied observations are native at(0)
 * byte, access>=0 / mkdir success, then freshly published string length.
 * Native owns lock, IO, warning, format/assignment and exception effects.
 * Stop on ACCEPT_TASK/REJECT_TASK/UINT32_MAX; no Rust borrow spans effects. */
uint32_t swr_v1_corebase_task_tmp_next(struct swr_v1_corebase_lifecycle_state *state,
                                    uint64_t observed, uint32_t limit);
enum swr_v1_corebase_hook_add_action_kind {
    SWR_COREBASE_HOOK_CREATE_FRONT = 1,
    SWR_COREBASE_HOOK_CREATE_BACK = 2,
    SWR_COREBASE_HOOK_PUSH_FRONT = 3,
    SWR_COREBASE_HOOK_PUSH_BACK = 4
};
/* Native owns list allocation/publication and Callback copy/insertion effects. */
uint32_t swr_v1_corebase_hook_add_action(uint8_t list_present, int32_t push_back);
/* One character of swoole_random_string(std::string &, size_t), drawn with swoole_rand(0, 61). */
char swr_v1_corebase_random_char(void);
/* Rust defines these upstream functions under their C++ symbols, so swoole.h and
 * swoole_protocol.h stay their declarations: sw_malloc/sw_free/sw_calloc/
 * sw_realloc (libc's allocator), swoole_common_divisor/_multiple,
 * swoole_version_compare, swoole_dec2hex, swoole_type_size, swoole_itoa,
 * swoole_random_string(char *, size_t), swoole_dump_ascii/_hex, swoole_rand
 * (both overloads), swoole_random_int, swoole::microtime and
 * swoole::DataHead::dump. */
/* Also Rust's, from base.cc: swoole_shell_exec, swoole_system_random, both
 * swoole_redirect_stdout overloads, swoole_print_backtrace(_on_error),
 * swoole_exit and swoole_get_systemd_listen_fds; and, in crates/utilities,
 * swoole_set_log_level, swoole_get_log_level and swoole_set_log_file. */
/* Polls ready(context) with doubling sleeps; ready must not throw. */
bool swr_v1_corebase_wait_for(bool (*ready)(void *), void *context, int timeout_ms);
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
bool swr_v1_core_signal_dispatch(void);
void swr_v1_core_set_signal_dispatch(bool value);
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
#ifdef __cplusplus
}
#endif
#endif
