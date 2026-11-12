// Private test extension: actual public Logger class and loaded production methods.
// No copied class, private-field access, syscall interception or production edits.
#include <php.h>
#include <Zend/zend_exceptions.h>
#include <climits>
#include <atomic>
#include <chrono>
#include <cstdio>
#include <ctime>
#include <mutex>
#include <stdexcept>
#include <string>
#include <thread>
#include <vector>
#include "swoole_log.h"

static void check(bool condition, const char *message) {
    if (!condition) throw std::runtime_error(message);
}

static std::string contents(FILE *stream) {
    check(fflush(stream) == 0 && fseek(stream, 0, SEEK_SET) == 0, "stream rewind");
    std::string result;
    char buffer[256];
    size_t count;
    while ((count = fread(buffer, 1, sizeof(buffer), stream))) result.append(buffer, count);
    check(!ferror(stream), "stream read");
    return result;
}

struct ConfigResults {
    size_t formats = 0, accepted = 0, rejected = 0, resets = 0;
};

struct PrivateStream {
    FILE *value = tmpfile();
    PrivateStream() { check(value != nullptr, "configuration private stream"); }
    ~PrivateStream() { fclose(value); }
};

struct DiagnosticSink {
    swoole::Logger *logger = sw_logger();
    int level = logger->get_level();
    explicit DiagnosticSink(FILE *stream) {
        // The fixture runs in a fresh CLI process with the default stdout sink.
        check(!logger->is_opened(), "configuration expects unopened CLI logger");
        logger->set_level(SW_LOG_DEBUG);
        logger->set_stream(stream);
    }
    ~DiagnosticSink() {
        logger->set_stream(stdout);
        logger->set_level(level);
    }
};

// Logger::put dates a record by std::chrono::system_clock. ::time() reads Linux's
// coarse clock, which can trail it by a tick, so a record written just after a
// second boundary could fall outside a ::time() bracket taken on either side.
static time_t record_clock() {
    return std::chrono::system_clock::to_time_t(std::chrono::system_clock::now());
}

static bool date_prefix(const std::string &record, const char *format, time_t before, time_t after) {
    for (time_t observed : {before, after}) {
        char date[SW_LOG_DATE_STRLEN];
        size_t length = std::strftime(date, sizeof(date), format, std::localtime(&observed));
        if (length && record.compare(0, length + 2, "[" + std::string(date, length) + " ") == 0) return true;
    }
    return false;
}

static ConfigResults configuration_normal() {
    PrivateStream records, diagnostics;
    DiagnosticSink sink(diagnostics.value);
    swoole::Logger configured;
    configured.set_stream(records.value);
    ConfigResults result;
    // Nine defined cases from the old format fixture plus an owned literal.
    // The old non-ASCII format and interrupted/failure matrix stay unqualified.
    const std::vector<std::string> formats = {"", "%F %T", "%c", "%Z", "%%",
        std::string(127, 'A'), std::string(128, 'A'), std::string(255, 'A'),
        std::string("X\0%F", 4), "OWNED"};
    for (const auto &format : formats) {
        check(configured.set_date_format("KEEP"), "configuration retained baseline");
        const size_t offset = contents(records.value).size();
        time_t before = record_clock();
        char date[SW_LOG_DATE_STRLEN];
        const bool expected = std::strftime(date, sizeof(date), format.c_str(), std::localtime(&before)) != 0;
        std::string input = format;
        const bool accepted = configured.set_date_format(input.c_str());
        check(accepted == expected, "public date admission matches host strftime");
        // Valid caller-owned memory changes only after the public call returns.
        // A successful date must have been copied; rejection must retain KEEP.
        input.assign(512, 'Z');
        configured.put(SW_LOG_INFO, "configuration", 13);
        time_t after = record_clock();
        const auto record = contents(records.value).substr(offset);
        check(date_prefix(record, accepted ? format.c_str() : "KEEP", before, after), "date copy or rejection retention");
        check(record.find("\tINFO\tconfiguration\n") != std::string::npos, "configuration output record");
        ++result.formats;
        accepted ? ++result.accepted : ++result.rejected;
    }
    const auto warnings = contents(diagnostics.value);
    size_t count = 0, position = 0;
    while ((position = warnings.find("The date format string[length=", position)) != std::string::npos) {
        ++count; ++position;
    }
    check(count == result.rejected, "each rejected format emits its real diagnostic");
    for (int flags = 0; flags < 32; ++flags) {
        check(configured.set_date_format("OLD"), "reset nondefault date");
        configured.set_date_with_microseconds(flags & 1);
        configured.set_rotation(flags - 10);
        configured.set_level(flags - 20);
        configured.reset();
        check(configured.get_level() == SW_LOG_INFO, "reset restores level");
        const size_t offset = contents(records.value).size();
        time_t before = record_clock();
        const auto path = configured.gen_real_file("reset");
        configured.put(SW_LOG_INFO, "configuration-reset", 19);
        time_t after = record_clock();
        const auto record = contents(records.value).substr(offset);
        check(date_prefix(record, SW_LOG_DEFAULT_DATE_FORMAT, before, after), "reset restores date and disables microseconds");
        check(record.find("\tINFO\tconfiguration-reset\n") != std::string::npos, "reset retains caller stream");
        bool daily = false;
        for (time_t observed : {before, after}) {
            char date[16];
            const auto length = std::strftime(date, sizeof(date), "%Y%m%d", std::localtime(&observed));
            daily = daily || path == "reset." + std::string(date, length);
        }
        check(daily, "reset restores rotation");
        ++result.resets;
    }
    configured.close();
    check(fputs("still-owned\n", records.value) >= 0, "configuration never owns caller stream");
    return result;
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
        const std::string root(directory, length);
        swoole::Logger logger;
        check(!logger.is_opened() && logger.get_level() == SW_LOG_INFO, "default construction");
        check(std::string(logger.get_file()).empty() && std::string(logger.get_real_file()).empty(), "default paths");
        size_t levels = 0;
        for (int level : {INT_MIN, -1, 0, 1, 2, 3, 4, 5, 6, 7, INT_MAX}) {
            logger.set_level(level);
            check(logger.get_level() == (level < 0 ? 0 : level > 6 ? 6 : level), "level storage");
            ++levels;
        }
        // Ordinary long/binary strings, all rotation selectors, real host libc.
        size_t dates = 0;
        const std::vector<std::string> paths = {"", "plain", ".", std::string("a\0b", 3),
                                               std::string(127, 'p'), std::string(4096, 'q')};
        for (int rotation : {INT_MIN, -1, 0, 1, 2, 3, 4, 5, INT_MAX}) {
            logger.set_rotation(rotation);
            const char *format = rotation == 1 ? "%Y%m" : rotation == 3 ? "%Y%m%d%H" :
                                 rotation == 4 ? "%Y%m%d%H%M" : "%Y%m%d";
            for (const auto &path : paths) {
                time_t before = time(nullptr);
                auto actual = logger.gen_real_file(path);
                time_t after = time(nullptr);
                char first[16], last[16];
                size_t a = strftime(first, sizeof(first), format, localtime(&before));
                size_t b = strftime(last, sizeof(last), format, localtime(&after));
                check(a && b && (actual == path + "." + std::string(first, a) ||
                                 actual == path + "." + std::string(last, b)), "filename generation");
                check(actual.compare(0, path.size(), path) == 0, "binary prefix preserved");
                ++dates;
            }
        }
        logger.reset();
        const std::string path = root + "/" + std::string(180, 'f');
        check(logger.open(path.c_str()) && logger.is_opened(), "open long filename");
        check(std::string(logger.get_file()) == path && std::string(logger.get_real_file()) == path, "open path storage");
        check(logger.set_date_format("STORE"), "literal date format");
        logger.put(SW_LOG_NOTICE, "before", 6);
        check(rename(path.c_str(), (root + "/old-file").c_str()) == 0, "rename live file");
        logger.reopen();
        check(logger.is_opened() && std::string(logger.get_file()) == path, "reopen copies path before close");
        logger.put(SW_LOG_NOTICE, "after", 5);
        const char *real_path = logger.get_real_file();
        logger.close();
        check(!logger.is_opened() && std::string(logger.get_file()).empty(), "close clears logical path");
        check(logger.get_real_file() == real_path && std::string(real_path) == path, "close retains real path storage");

        // Reset changes configuration while retaining the actual opened FILE/path.
        const std::string rotating = root + "/reset-file";
        logger.set_rotation(SW_LOG_ROTATION_DAILY);
        logger.set_date_with_microseconds(true);
        logger.set_level(SW_LOG_NONE);
        check(logger.open(rotating.c_str()), "open rotated file");
        const std::string real = logger.get_real_file();
        logger.reset();
        check(logger.is_opened() && logger.get_level() == SW_LOG_INFO &&
              std::string(logger.get_file()) == rotating && std::string(logger.get_real_file()) == real,
              "reset retains file custody");
        logger.put(SW_LOG_NOTICE, "reset", 5);
        logger.close();

        // Externally supplied streams remain caller-owned; explicit close is a
        // no-op when no file was opened by Logger. All handles stay valid.
        FILE *first = tmpfile(), *second = tmpfile();
        check(first && second, "private streams");
        check(logger.open((root + "/replaced-file").c_str()), "open file before stream replacement");
        logger.put(SW_LOG_NOTICE, "before-replacement", 18);
        logger.set_stream(first);
        check(!logger.is_opened() && std::string(logger.get_file()).empty(), "actual open FILE closed before stream publication");
        check(logger.set_date_format("STREAM"), "stream date");
        logger.put(SW_LOG_NOTICE, "first", 5);
        logger.set_stream(second);
        logger.put(SW_LOG_NOTICE, "second", 6);
        logger.close();
        check(fputs("caller-owned\n", first) >= 0 && fputs("caller-owned\n", second) >= 0, "caller retains streams");
        auto first_text = contents(first), second_text = contents(second);
        check(first_text.find("\tNOTICE\tfirst\ncaller-owned\n") != std::string::npos &&
              second_text.find("\tNOTICE\tsecond\ncaller-owned\n") != std::string::npos, "stream publication");
        logger.set_stream(stdout);
        check(fclose(first) == 0 && fclose(second) == 0, "caller closes streams");

        // Public header permits put/reopen concurrently. One writer is the only
        // calendar caller; SINGLE rotation keeps reopen out of localtime.
        logger.reset();
        check(logger.set_date_format("REOPEN"), "concurrent date");
        check(logger.open((root + "/concurrent").c_str()), "concurrent file");
        std::atomic<bool> start{false};
        std::exception_ptr errors[2];
        std::thread writer([&] {
            while (!start.load()) std::this_thread::yield();
            try {
                for (int i = 0; i < 1024; ++i) {
                    auto message = "record-" + std::to_string(i);
                    logger.put(SW_LOG_NOTICE, message.data(), message.size());
                }
            } catch (...) { errors[0] = std::current_exception(); }
        });
        std::thread reopener;
        try {
            reopener = std::thread([&] {
                while (!start.load()) std::this_thread::yield();
                try {
                    for (int i = 0; i < 64; ++i) logger.reopen();
                } catch (...) { errors[1] = std::current_exception(); }
            });
        } catch (...) {
            start.store(true);
            writer.join();
            logger.close();
            throw;
        }
        start.store(true);
        writer.join();
        reopener.join();
        for (auto error : errors) if (error) std::rethrow_exception(error);
        check(logger.is_opened(), "concurrent file retained");
        logger.close();
        const auto configuration = configuration_normal();
        array_init(return_value);
        add_assoc_long(return_value, "configuration_formats", configuration.formats);
        add_assoc_long(return_value, "configuration_accepted", configuration.accepted);
        add_assoc_long(return_value, "configuration_rejected", configuration.rejected);
        add_assoc_long(return_value, "configuration_resets", configuration.resets);
        add_assoc_long(return_value, "level_cases", levels);
        add_assoc_long(return_value, "filename_cases", dates);
        add_assoc_long(return_value, "reopen_records", 2);
        add_assoc_long(return_value, "caller_streams", 2);
        add_assoc_long(return_value, "thread_records", 1024);
        add_assoc_long(return_value, "thread_reopens", 64);
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
