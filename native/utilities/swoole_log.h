/* SPDX-License-Identifier: Apache-2.0 */
/* Swoole's include/swoole_log.h, owned here: swoole::Logger, a handle on Rust's logger
 * (crates/utilities/src/logger.rs), and the log macros, which run Rust's front door at each
 * callsite. swoole.h includes it; core-tests and the utilities gates call every Logger method. */
#pragma once

#include <cstddef>
#include <cstdint>
#include <string>
#include <cstdio>
#include <unistd.h>
#include "swoole_rs_utilities.h"

#define SW_LOG_DATE_STRLEN 128
#define SW_LOG_DEFAULT_DATE_FORMAT "%F %T"

// Explicit, because crates/corebase's tests compare them with its LogLevel.
enum {
    SW_LOG_DEBUG = 0,
    SW_LOG_TRACE = 1,
    SW_LOG_INFO = 2,
    SW_LOG_NOTICE = 3,
    SW_LOG_WARNING = 4,
    SW_LOG_ERROR = 5,
    SW_LOG_NONE = 6,
};

// Logger::set_rotation's values (log_rotation in crates/utilities/src/lib.rs); upstream's
// TEST(log, rotation), which the utilities-logger-originals gate builds unchanged, names
// all but the first.
enum {
    SW_LOG_ROTATION_SINGLE = 0,
    SW_LOG_ROTATION_MONTHLY,
    SW_LOG_ROTATION_DAILY,
    SW_LOG_ROTATION_HOURLY,
    SW_LOG_ROTATION_EVERY_MINUTE,
};

namespace swoole {
// The state and operations are Rust's (crates/utilities/src/logger.rs).
class Logger {
  private:
    SwrLogger *logger = swr_v1_logger_new(stdout, swr_logger_host);

  public:
    Logger() = default;
    Logger(const Logger &) = delete;
    Logger &operator=(const Logger &) = delete;
    ~Logger() {
        swr_v1_logger_free(logger);
    }
    bool open(const char *logfile) {
        return swr_v1_logger_open(logger, logfile);
    }
    void reopen() {
        swr_v1_logger_reopen(logger);
    }
    void set_stream(FILE *stream) {
        swr_v1_logger_set_stream(logger, stream);
    }
    /**
     * Only the `put` and `reopen` functions are thread-safe,
     * other functions must be used in a single-threaded environment.
     */
    void put(int level, const char *content, size_t length) {
        swr_v1_logger_put(logger, level, content, length);
    }
    void close() {
        swr_v1_logger_close(logger);
    }
    void reset() {
        swr_v1_logger_set(logger, SWR_LOGGER_RESET, 0);
    }
    void set_level(int lv) {
        swr_v1_logger_set_level(logger, lv);
    }
    int get_level() const {
        return swr_v1_logger_level(logger);
    }
    bool set_date_format(const char *format) {
        return swr_v1_logger_set_date_format(logger, format);
    }
    void set_rotation(int rotation) {
        swr_v1_logger_set(logger, SWR_LOGGER_ROTATION, rotation);
    }
    const char *get_real_file() {
        return swr_v1_logger_file(logger, true);
    }
    const char *get_file() const {
        return swr_v1_logger_file(logger, false);
    }
    bool is_opened() const {
        return swr_v1_logger_is_opened(logger);
    }
    void set_date_with_microseconds(bool enable) {
        swr_v1_logger_set(logger, SWR_LOGGER_MICROSECONDS, enable);
    }
    std::string gen_real_file(const std::string &file) const {
        uint8_t date[16];
        const size_t length = swr_v1_logger_rotation_date(logger, &date);
        return file + "." + std::string(reinterpret_cast<const char *>(date), length);
    }
    static std::string get_pretty_name(const std::string &pretty_function, bool strip = true) {
        size_t begin = 0, count = 0;
        if (swr_v1_utils_pretty_range(reinterpret_cast<const uint8_t *>(pretty_function.data()),
                                      pretty_function.size(), strip, &begin, &count) != 0) {
            return {};
        }
        return pretty_function.substr(begin, count);
    }

    void display_backtrace() {
        swr_v1_logger_set(logger, SWR_LOGGER_BACKTRACE, 1);
    }
};
}  // namespace swoole

swoole::Logger *sw_logger();
#define __SW_FUNC__ (swoole::Logger::get_pretty_name(__PRETTY_FUNCTION__).c_str())

// Native dispatch executes each requested effect at the original macro callsite.
#define SWR_UTILS_LOG_RUN(op, initial_phase, initial_length, publish, observe_condition, observed_mask, ignored, format, put_level) \
    do { \
        SwrLogFrontdoor swr_state = {initial_phase, 0}; \
        swoole_rs_utilities::LogFacts swr_facts = {}; \
        size_t swr_length = (initial_length); \
        for (;;) { \
            const SwrLogAction swr_next = swr_v2_utils_log_frontdoor_step(&swr_state, op, swr_facts.first, swr_facts.second, swr_facts.unsigned_width, swr_facts.high); \
            if (swr_next.action == 0) break; \
            const int32_t swr_level = swr_next.level; \
            (void) swr_level; \
            switch (swr_next.action) { \
            case 1: publish; break; \
            case 2: observe_condition; break; \
            case 3: swr_facts = swoole_rs_utilities::log_facts((observed_mask), swr_v1_core_trace_flags()); break; \
            case 4: swr_facts.first = (ignored); break; \
            case 5: swr_length = (format); break; \
            case 6: sw_logger()->put((put_level), sw_error, swr_length); break; \
            case 7: swoole_exit(1); break; \
            } \
        } \
    } while (0)

#define swoole_info(str, ...) if (swoole_rs_utilities::log_frontdoor_admit(0, swoole_get_log_level())) { size_t _sw_error_len = sw_snprintf(sw_error, SW_ERROR_MSG_SIZE, str, ##__VA_ARGS__); SWR_UTILS_LOG_RUN(0, 4, _sw_error_len, (void) 0, (void) 0, 0, 0, 0, swr_level); }

#define swoole_notice(str, ...) if (swoole_rs_utilities::log_frontdoor_admit(1, swoole_get_log_level())) { size_t _sw_error_len = sw_snprintf(sw_error, SW_ERROR_MSG_SIZE, str, ##__VA_ARGS__); SWR_UTILS_LOG_RUN(1, 4, _sw_error_len, (void) 0, (void) 0, 0, 0, 0, swr_level); }

#define swoole_sys_notice(str, ...) SWR_UTILS_LOG_RUN(5, 0, 0, swoole_set_last_error(errno), swr_facts = swoole_rs_utilities::log_level_facts(0, swoole_get_log_level()), 0, 0, (sw_snprintf(sw_error, SW_ERROR_MSG_SIZE, "%s(:%d): " str ", Error: %s[%d]", __SW_FUNC__, __LINE__, ##__VA_ARGS__, swoole_strerror(errno), errno)), swr_level)

#define swoole_warning(str, ...) SWR_UTILS_LOG_RUN(2, 0, 0, (void) 0, swr_facts = swoole_rs_utilities::log_level_facts(0, swoole_get_log_level()), 0, 0, (sw_snprintf(sw_error, SW_ERROR_MSG_SIZE, "%s(): " str, __SW_FUNC__, ##__VA_ARGS__)), swr_level)

#define swoole_sys_warning(str, ...) SWR_UTILS_LOG_RUN(6, 0, 0, swoole_set_last_error(errno), swr_facts = swoole_rs_utilities::log_level_facts(0, swoole_get_log_level()), 0, 0, (sw_snprintf(sw_error, SW_ERROR_MSG_SIZE, "%s(): " str ", Error: %s[%d]", __SW_FUNC__, ##__VA_ARGS__, swoole_strerror(errno), errno)), swr_level)

#define swoole_fatal_error(code, str, ...) ((void (*)(int, const char *, ...)) swr_v1_core_fatal_error())(code, str, ##__VA_ARGS__)

#define swoole_error_log(level, error, str, ...) SWR_UTILS_LOG_RUN(7, 0, 0, swoole_set_last_error(error), if (swoole_rs_utilities::LogNumericLevel<decltype((level))>::value) { swr_facts = swoole_rs_utilities::log_complete_condition(swoole_rs_utilities::log_numeric_admit(swoole_rs_utilities::log_level_facts((level), swoole_get_log_level())) && swoole_rs_utilities::log_ignored_result(swoole_is_ignored_error(error))); } else { swr_facts = swoole_rs_utilities::log_complete_condition((level >= swoole_get_log_level() && !swoole_is_ignored_error(error)) ? true : false); }, 0, swoole_is_ignored_error(error), (sw_snprintf(sw_error, SW_ERROR_MSG_SIZE, "%s() (ERRNO %d): " str, __SW_FUNC__, error, ##__VA_ARGS__)), level)

#ifdef SW_DEBUG
#define swoole_debug(str, ...) if (swoole_rs_utilities::log_frontdoor_admit(3, swoole_get_log_level())) { size_t _sw_error_len = sw_snprintf(sw_error, SW_ERROR_MSG_SIZE, "%s(:%d): " str, __SW_FUNC__, __LINE__, ##__VA_ARGS__); SWR_UTILS_LOG_RUN(3, 4, _sw_error_len, (void) 0, (void) 0, 0, 0, 0, swr_level); }

#define swoole_hex_dump(data, length) \
    do { \
        const char *swr_data = (data); \
        size_t swr_size = (length); \
        SwrLogHex swr_hex = {}; \
        int32_t swr_previous = 0; \
        char swr_hex_bytes[49], swr_text_bytes[17]; \
        for (;;) { \
            const uint32_t swr_action = swr_v2_utils_log_hex_step(&swr_hex, swr_size, swr_previous); \
            if (swr_action == 0) break; \
            switch (swr_action) { \
            case 1: case 7: swoole_debug("+----------+------------+-----------+-----------+------------+------------------+"); break; \
            case 2: swr_previous = sprintf(swr_hex_bytes + swr_hex.hex_length, "%02x ", swr_data[swr_hex.index] & 0xff); break; \
            case 3: swr_previous = isprint((int) swr_data[swr_hex.index]); break; \
            case 4: swr_previous = sprintf(swr_text_bytes + swr_hex.text_length, "%c", swr_data[swr_hex.index]); break; \
            case 5: swr_previous = sprintf(swr_text_bytes + swr_hex.text_length, "%c", '.'); break; \
            case 6: swoole_debug("| %08zx | %-48s| %-16s |", swr_hex.offset, swr_hex_bytes, swr_text_bytes); break; \
            } \
        } \
    } while (0)
#else
#define swoole_debug(str, ...)
#define swoole_hex_dump(data, length)
#endif

enum : long {
    SW_TRACE_NORMAL = 1u << 21,
    SW_TRACE_CO_CURL = 1u << 29,
};

#ifdef SW_LOG_TRACE_OPEN
#define swoole_trace_log(what, str, ...) if (swoole_rs_utilities::log_frontdoor_admit(4, swoole_get_log_level()) && swoole_rs_utilities::log_frontdoor_mask(what, swr_v1_core_trace_flags())) { size_t _sw_error_len = sw_snprintf(sw_error, SW_ERROR_MSG_SIZE, "%s(:%d): " str, __SW_FUNC__, __LINE__, ##__VA_ARGS__); SWR_UTILS_LOG_RUN(4, 4, _sw_error_len, (void) 0, (void) 0, 0, 0, 0, swr_level); }
#else
#define swoole_trace_log(what, str, ...)
#endif

#define swoole_trace(str, ...) swoole_trace_log(SW_TRACE_NORMAL, str, ##__VA_ARGS__)
