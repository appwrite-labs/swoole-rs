// Normal public Logger behavior through the actual production methods.
#include <php.h>
#include <Zend/zend_exceptions.h>
#include <cstdio>
#include <mutex>
#include <stdexcept>
#include <string>
#include "swoole_log.h"

static void check(bool condition, const char *message) {
    if (!condition) throw std::runtime_error(message);
}

ZEND_BEGIN_ARG_INFO_EX(arginfo_m14_storage, 0, 0, 1)
    ZEND_ARG_TYPE_INFO(0, directory, IS_STRING, 0)
ZEND_END_ARG_INFO()

PHP_FUNCTION(m14_storage) {
    char *directory;
    size_t length;
    ZEND_PARSE_PARAMETERS_START(1, 1)
        Z_PARAM_STRING(directory, length)
    ZEND_PARSE_PARAMETERS_END();
    try {
        swoole::Logger logger;
        const std::string path = std::string(directory, length) + "/backtrace.log";
        check(logger.open(path.c_str()), "open normal log");
        check(logger.set_date_format("TRACE"), "literal date");
        puts("M14_TRACE_DEFAULT_BEGIN");
        logger.put(SW_LOG_NOTICE, "default", 7);
        puts("M14_TRACE_DEFAULT_END");
        logger.display_backtrace();
        logger.set_level(SW_LOG_WARNING);
        puts("M14_TRACE_FILTER_BEGIN");
        logger.put(SW_LOG_NOTICE, "filtered", 8);
        puts("M14_TRACE_FILTER_END");
        puts("M14_TRACE_ENABLED_BEGIN");
        logger.put(SW_LOG_WARNING, "enabled", 7);
        puts("M14_TRACE_ENABLED_END");
        // Original reset changes four fields, retaining the backtrace flag.
        logger.reset();
        check(logger.set_date_format("TRACE"), "reset literal date");
        puts("M14_TRACE_RESET_BEGIN");
        logger.put(SW_LOG_NOTICE, "reset", 5);
        puts("M14_TRACE_RESET_END");
        logger.close();
        check(fflush(stdout) == 0, "trace output flush");
        array_init(return_value);
        add_assoc_long(return_value, "records", 3);
        add_assoc_long(return_value, "enabled_traces", 2);
        add_assoc_bool(return_value, "filtered", true);
        add_assoc_bool(return_value, "reset_retains_backtrace", true);
    } catch (const std::exception &error) {
        zend_throw_exception(zend_ce_exception, error.what(), 0);
    }
}

static const zend_function_entry functions[] = {
    PHP_FE(m14_storage, arginfo_m14_storage)
    PHP_FE_END
};

extern "C" {
zend_module_entry m14_storage_module_entry = {
    STANDARD_MODULE_HEADER, "m14_storage", functions,
    nullptr, nullptr, nullptr, nullptr, nullptr, "0", STANDARD_MODULE_PROPERTIES
};
ZEND_GET_MODULE(m14_storage)
}
