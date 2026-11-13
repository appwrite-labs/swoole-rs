#ifndef SWOOLE_RS_UTILITIES_H
#define SWOOLE_RS_UTILITIES_H
#include <stdint.h>
#include <stddef.h>
#include <stdbool.h>
#include <stdio.h>
#ifdef __cplusplus
extern "C" {
#endif
/* Complete callback-free macro programs. Context is unique during each call;
 * no fact, argument, buffer, global or callback is retained by Rust.
 * Operations0..9: info,notice,warning,debug,trace,sys_notice,sys_warning,
 * error_log,error,sys_error. Actions0..7: done,publish,threshold,mask,ignored,
 * format,put,exit. Facts only answer the immediately preceding action.
 * unsigned_width0=signed,32/64=original usual unsigned integer comparison. */
struct SwrLogFrontdoor { uint32_t phase, admitted; };
struct SwrLogAction { uint32_t action; int32_t level; };
struct SwrLogAction swr_v2_utils_log_frontdoor_step(struct SwrLogFrontdoor *, uint32_t, int64_t, int64_t, uint32_t, uint64_t);
/* Whole hex record sequence. Native effects write actual row arrays, classify
 * fresh original promoted char and return exact sprintf counts after Rust returns.
 * Actions:0done1header2hex-format3ctype4text-byte5text-dot6row7footer. */
struct SwrLogHex { uint32_t phase; size_t offset, index, hex_length, text_length; };
uint32_t swr_v2_utils_log_hex_step(struct SwrLogHex *, size_t, int32_t);
/* Native-compatible aligned shared u32 atomic, native release store remains outside Rust. */
void swr_v1_utils_spinlock(const uint32_t *, uint32_t cpus, uint32_t backoff_limit);
#ifdef __linux__ /* matches the Rust cfg(target_os = "linux") */
/* Futex wait/wake on a shared aligned native u32 atomic.
 * Caller owns lifetime; no callback or allocation; preserves native errno. */
int32_t swr_v1_utils_wait(const uint32_t *, double timeout);
int32_t swr_v1_utils_wakeup(const uint32_t *, int32_t n);
#endif
/* Borrowed bytes and independent aligned outputs; returns a substring span, never allocates. */
int32_t swr_v1_utils_pretty_range(const uint8_t *, size_t, uint8_t strip, size_t *, size_t *);
/* swoole::Logger (crates/utilities/src/logger.rs). Worker identity, backtraces
 * and stdout redirection call back into the host (swr_logger_host, crates/process), and
 * diagnostics into corebase's log host, after the logger's lock is released. */
typedef struct SwrLogger SwrLogger;
struct SwrLoggerIdentity { int worker_id, worker_pid; char worker_symbol; };
struct SwrLoggerHost {
    void (*redirect)(int fd);
    void (*backtrace)(void);
    struct SwrLoggerIdentity (*identity)(void);
};
extern const struct SwrLoggerHost swr_logger_host;
enum SwrLoggerSetting {
    SWR_LOGGER_ROTATION = 0, SWR_LOGGER_MICROSECONDS = 1,
    SWR_LOGGER_BACKTRACE = 2, SWR_LOGGER_RESET = 3
};
SwrLogger *swr_v1_logger_new(FILE *stdout_stream, struct SwrLoggerHost host);
void swr_v1_logger_free(SwrLogger *);
bool swr_v1_logger_open(const SwrLogger *, const char *file);
void swr_v1_logger_reopen(const SwrLogger *);
void swr_v1_logger_close(const SwrLogger *);
void swr_v1_logger_set_stream(const SwrLogger *, FILE *stream);
void swr_v1_logger_put(const SwrLogger *, int level, const char *content, size_t length);
int swr_v1_logger_level(const SwrLogger *);
void swr_v1_logger_set_level(const SwrLogger *, int level);
void swr_v1_logger_set(const SwrLogger *, enum SwrLoggerSetting, int value);
bool swr_v1_logger_set_date_format(const SwrLogger *, const char *format);
bool swr_v1_logger_is_opened(const SwrLogger *);
/* get_file (false) or get_real_file (true), valid until the file next changes. */
const char *swr_v1_logger_file(const SwrLogger *, bool real);
size_t swr_v1_logger_rotation_date(const SwrLogger *, uint8_t (*date)[16]);
#ifdef __cplusplus
}
#include <type_traits>
#include <utility>
namespace swoole_rs_utilities {
struct LogFacts { int64_t first, second; uint32_t unsigned_width; uint64_t high; };
inline LogFacts log_facts(int64_t first, int64_t second) { return {first, second, 0, 0}; }
// ADL detects enum operator overloads; classes always take the exact external
// expression path. Neither type inspection nor common_type evaluates operands.
template<class T> auto log_external_compare(int)
    -> decltype(operator>=(std::declval<T>(), std::declval<int>()), std::true_type{});
template<class> auto log_external_compare(...) -> std::false_type;
template<class T> struct LogNumericLevel {
    using Plain = typename std::remove_cv<typename std::remove_reference<T>::type>::type;
    static constexpr bool value =
        (std::is_arithmetic<Plain>::value || std::is_enum<Plain>::value
#ifdef __SIZEOF_INT128__
         || std::is_same<Plain, __int128>::value || std::is_same<Plain, unsigned __int128>::value
#endif
        ) && !decltype(log_external_compare<T>(0))::value;
};
// Host floating >= is an external ABI/FENV primitive, including long double.
// It executes only after Rust requests the operand observation, without any
// Rust frame or borrow spanning its floating exceptions/rounding environment.
template<class Common> inline typename std::enable_if<std::is_floating_point<Common>::value, LogFacts>::type
log_numeric_facts(Common value, int threshold) {
    return {value >= threshold ? 1 : 0, threshold, 256, 0};
}
template<class Common> inline typename std::enable_if<!std::is_floating_point<Common>::value, LogFacts>::type
log_numeric_facts(Common value, int threshold) {
    uint64_t high = 0;
    uint32_t kind = std::is_unsigned<Common>::value ? uint32_t(sizeof(Common) * 8) : 0;
#ifdef __SIZEOF_INT128__
    if (sizeof(Common) > 8) {
        high = static_cast<uint64_t>(static_cast<unsigned __int128>(value) >> 64);
        kind = std::is_signed<Common>::value ? 129 : 128;
        // Some standard libraries do not classify compiler extended integers.
        if (std::is_same<Common, __int128>::value) kind = 129;
    }
#endif
    return {static_cast<int64_t>(value), threshold, kind, high};
}
template<class Level> inline typename std::enable_if<LogNumericLevel<Level &&>::value, LogFacts>::type
log_level_facts(Level &&value, int threshold) {
    using Plain = typename std::remove_reference<Level>::type;
    using Common = typename std::common_type<Plain, int>::type;
    return log_numeric_facts<Common>(static_cast<Common>(value), threshold);
}
// Unexecuted numeric arm must remain well-formed for noncopyable user operands.
template<class Level> inline typename std::enable_if<!LogNumericLevel<Level &&>::value, LogFacts>::type
log_level_facts(Level &&, int) { return {}; }
inline bool log_numeric_admit(LogFacts facts) {
    SwrLogFrontdoor state = {2, 0};
    return swr_v2_utils_log_frontdoor_step(&state, 7, facts.first, facts.second,
                                           facts.unsigned_width, facts.high).action == 4;
}
inline bool log_ignored_result(bool ignored) {
    SwrLogFrontdoor state = {8, 0};
    return swr_v2_utils_log_frontdoor_step(&state, 7, ignored, 0, 0, 0).action == 5;
}
inline LogFacts log_complete_condition(bool accepted) { return {accepted, 0, 257, 0}; }
/* Bare-if macros split the pure guard from the fixed post-format continuation.
 * No user argument executes inside a helper/lambda, preserving __func__ and the
 * caller's function scope. Native && only dispatches the requested mask fact. */
inline bool log_frontdoor_admit(uint32_t operation, int64_t threshold) {
    SwrLogFrontdoor state = {};
    (void) swr_v2_utils_log_frontdoor_step(&state, operation, 0, 0, 0, 0);
    const SwrLogAction next = swr_v2_utils_log_frontdoor_step(&state, operation, 0, threshold, 0, 0);
    return next.action == 3 || next.action == 5;
}
inline bool log_frontdoor_mask(int64_t mask, int64_t flags) {
    SwrLogFrontdoor state = {3, 0};
    return swr_v2_utils_log_frontdoor_step(&state, 4, mask, flags, 0, 0).action == 5;
}
}
#endif
#endif
