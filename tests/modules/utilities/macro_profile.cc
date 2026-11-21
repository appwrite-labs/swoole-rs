// Ordinary public macro callers, with the exact macro bodies injected only
// after the complete SDK has fixed its actual class layouts/configuration.
#include <php.h>
#include <Zend/zend_exceptions.h>
#include <cerrno>
#include <stdexcept>
#include <string>
#include "swoole.h"
#include "swoole_log.h"
#include "macro-profile.h"

static unsigned formats, levels, errors, masks, data_reads, length_reads;
static const char *payload(const char *function) {
    ++formats;
    if (std::string(function) != "zif_m14_macro_profile") {
        throw std::runtime_error("macro argument left original function scope");
    }
    return "payload";
}
static int level() { ++levels; return SW_LOG_WARNING; }
static int error_code(int code) { ++errors; return code; }
[[maybe_unused]] static long mask() { ++masks; return SW_TRACE_NORMAL; }
[[maybe_unused]] static const char *hex_data() { ++data_reads; return "0123456789abcdefZ"; }
[[maybe_unused]] static size_t hex_length() { ++length_reads; return 17; }
static void check(bool condition, const char *message) {
    if (!condition) throw std::runtime_error(message);
}
ZEND_BEGIN_ARG_INFO_EX(arginfo_macro_profile, 0, 0, 1)
    ZEND_ARG_TYPE_INFO(0, directory, IS_STRING, 0)
ZEND_END_ARG_INFO()
PHP_FUNCTION(m14_macro_profile) {
    char *directory; size_t length;
    ZEND_PARSE_PARAMETERS_START(1, 1)
        Z_PARAM_STRING(directory, length)
    ZEND_PARSE_PARAMETERS_END();
    try {
        auto *logger = sw_logger();
        logger->close(); logger->reset();
        check(logger->set_date_format("MACRO"), "literal date");
        const auto path = std::string(directory, length) + "/macro.log";
        check(logger->open(path.c_str()), "ordinary log open");
        logger->set_level(SW_LOG_NONE);
        swoole_info("%s", payload(__func__));
        swoole_notice("%s", payload(__func__));
        swoole_warning("%s", payload(__func__));
        errno = EDOM; swoole_sys_notice("%s", payload(__func__));
        check(swoole_get_last_error() == EDOM, "filtered sys_notice publishes error");
        errno = EDOM; swoole_sys_warning("%s", payload(__func__));
        check(swoole_get_last_error() == EDOM, "filtered sys_warning publishes error");
        swoole_error_log(level(), error_code(777777), "%s", payload(__func__));
        check(swoole_get_last_error() == 777777, "filtered error publishes error");
        swoole_trace_log(mask(), "%s", payload(__func__));
        swoole_debug("%s", payload(__func__));
        swoole_hex_dump(hex_data(), hex_length());
        check(formats == 0 && levels == 1 && errors == 1 && masks == 0, "filtered arguments");
        int branches = 0;
        swoole_info("filtered grammar") else ++branches;
        swoole_notice("filtered grammar") else ++branches;
#if M14_MACRO_DEBUG
        swoole_debug("filtered grammar") else ++branches;
#endif
#if M14_MACRO_TRACE
        swoole_trace_log(mask(), "filtered grammar") else ++branches;
#endif
        check(branches == 2 + M14_MACRO_DEBUG + M14_MACRO_TRACE, "bare-if else grammar");
        // Original sys_* guards use ERROR, though their put levels are lower.
        // Formatting therefore occurs at this threshold while the records filter.
        logger->set_level(SW_LOG_ERROR);
        errno = EDOM; swoole_sys_notice("%s", payload(__func__));
        errno = EDOM; swoole_sys_warning("%s", payload(__func__));
        check(formats == 2, "sys threshold preserves format evaluations");
        logger->set_level(SW_LOG_DEBUG);
        swoole_info("%s", payload(__func__));
        swoole_notice("%s", payload(__func__));
        swoole_warning("%s", payload(__func__));
        errno = EDOM; swoole_sys_notice("%s", payload(__func__));
        errno = EDOM; swoole_sys_warning("%s", payload(__func__));
        swoole_error_log(level(), error_code(888888), "%s", payload(__func__));
        swoole_ignore_error(999999);
        swoole_error_log(level(), error_code(999999), "%s", payload(__func__));
        check(swoole_get_last_error() == 999999, "ignored error still publishes");
        swoole_set_trace_flags(0);
        swoole_trace_log(mask(), "%s", payload(__func__));
        swoole_set_trace_flags(SW_TRACE_NORMAL);
        swoole_trace_log(mask(), "%s", payload(__func__));
        swoole_debug("%s", payload(__func__));
        swoole_hex_dump(hex_data(), hex_length());
        logger->close();
        check(formats == 8 + M14_MACRO_DEBUG + M14_MACRO_TRACE && levels == 4 && errors == 6,
              "fresh/repeated argument counts");
        check(masks == 2 * M14_MACRO_TRACE && data_reads == 2 * M14_MACRO_DEBUG &&
              length_reads == 2 * M14_MACRO_DEBUG, "compileoff and hex capture counts");
        array_init(return_value);
        add_assoc_long(return_value, "formats", formats);
        add_assoc_long(return_value, "levels", levels);
        add_assoc_long(return_value, "errors", errors);
        add_assoc_long(return_value, "masks", masks);
        add_assoc_long(return_value, "hex_data", data_reads);
        add_assoc_long(return_value, "hex_length", length_reads);
        add_assoc_bool(return_value, "debug", M14_MACRO_DEBUG);
        add_assoc_bool(return_value, "trace", M14_MACRO_TRACE);
    } catch (const std::exception &error) {
        zend_throw_exception(zend_ce_exception, error.what(), 0);
    }
}
static const zend_function_entry functions[] = {
    PHP_FE(m14_macro_profile, arginfo_macro_profile)
    PHP_FE_END
};
extern "C" {
zend_module_entry m14_macro_profile_module_entry = {
    STANDARD_MODULE_HEADER, "m14_macro_profile", functions,
    nullptr, nullptr, nullptr, nullptr, nullptr, "0", STANDARD_MODULE_PROPERTIES
};
ZEND_GET_MODULE(m14_macro_profile)
}
