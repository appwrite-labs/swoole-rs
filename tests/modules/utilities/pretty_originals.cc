// Original GoogleTest assertions call actual loaded public utility methods.
#include <php.h>
#include <cstdio>
#include <cerrno>
#include <cstdlib>
#include <cstring>
#include <mutex>
#include <string>
#include <vector>
#include <gtest/gtest.h>
#include "swoole_log.h"
using namespace swoole;
#include "logger-originals-config.h"
#include "pretty_originals.inc"

ZEND_BEGIN_ARG_INFO_EX(arginfo_m14_storage, 0, 0, 1)
    ZEND_ARG_TYPE_INFO(0, directory, IS_STRING, 0)
ZEND_END_ARG_INFO()

PHP_FUNCTION(m14_storage) {
    char *directory;
    size_t length;
    ZEND_PARSE_PARAMETERS_START(1, 1)
        Z_PARAM_STRING(directory, length)
    ZEND_PARSE_PARAMETERS_END();
    std::string program = "m14-originals";
    std::string filter = M14_GTEST_FILTER;
    std::string output = "--gtest_output=xml:" + std::string(directory, length) + "/gtest.xml";
    char *arguments[] = {program.data(), filter.data(), output.data(), nullptr};
    int count = 3;
    ::testing::InitGoogleTest(&count, arguments);
#ifdef M14_PRIVATE_LOG_FILE
    const char *previous = file;
    std::string private_file = std::string(directory, length) + "/original.log";
    file = private_file.c_str();
#endif
    int status = RUN_ALL_TESTS();
#ifdef M14_PRIVATE_LOG_FILE
    file = previous;
#endif
    fflush(stdout);
    array_init(return_value);
    add_assoc_long(return_value, "gtest_exit", status);
    add_assoc_long(return_value, "original_cases", ::testing::UnitTest::GetInstance()->test_to_run_count());
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
