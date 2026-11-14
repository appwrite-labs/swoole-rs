// Actual public production method; only the oracle is extracted from the pin.
#include <php.h>
#include <Zend/zend_exceptions.h>
#include <cstdio>
#include <mutex>
#include <random>
#include <stdexcept>
#include <string>
#include <vector>
#include "swoole_log.h"
#include "pretty_reference.inc"

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
        std::vector<std::string> names = {"void ns::Class::method(int)", "int x()", "::f()", "aa::bb()",
                                           "no bracket", "()", "a::::b()", std::string("a\0::b()", 8)};
        for (size_t colons = 0; colons < 65; ++colons) for (size_t prefix = 0; prefix < 8; ++prefix) {
            names.push_back(std::string(prefix, 'a') + std::string(colons, ':') + "f()");
            names.push_back("void " + std::string(prefix, 'a') + std::string(colons, ':') + "f(int)");
        }
        std::mt19937 random(982341);
        for (size_t i = 0; i < 20000; ++i) {
            std::string name;
            for (size_t n = random() % 80; n > 0; --n) name += "ab :()_"[random() % 8];
            names.push_back(name);
        }
        size_t cases = 0;
        for (const auto &name : names) for (bool strip : {false, true}) {
            if (swoole::Logger::get_pretty_name(name, strip) != reference_pretty(name, strip)) {
                throw std::runtime_error("actual public pretty-name differs from pinned body");
            }
            ++cases;
        }
        array_init(return_value);
        add_assoc_long(return_value, "pretty_cases", cases);
        add_assoc_long(return_value, "colon_names", 1040);
        add_assoc_long(return_value, "strip_modes", 2);
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
