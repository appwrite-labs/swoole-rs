// Upstream's hashes, pretty-name slicing and logger formatting behind C entry points:
// build.rs copies the pinned bodies into utilities.inc unchanged. Compiled once per
// `char` signedness (SWR_UTILS_ENGINE), since the hashes promote `char`.
#include <cerrno>
#include <cstdarg>
#include <cstdint>
#include <cstdio>
#include <cstring>
#include <string>

#define SWR_CAT3(a, b, c) a##_##b##_##c
#define SWR_CAT(a, b, c) SWR_CAT3(a, b, c)
#define SWR_EXPORT(name) SWR_CAT(swr_oracle_utils, SWR_UTILS_ENGINE, name)

namespace SWR_UTILS_ENGINE {
using ulong_t = unsigned long;
#include "utilities.inc"
}  // namespace SWR_UTILS_ENGINE

using namespace SWR_UTILS_ENGINE;

extern "C" uint64_t SWR_EXPORT(hash)(uint32_t algorithm, const char *key, size_t length) {
    switch (algorithm) {
    case 0:
        return swoole_hash_php(key, length);
    case 1:
        return swoole_hash_jenkins(key, length);
    default:
        return swoole_hash_austin(key, length);
    }
}

extern "C" int SWR_EXPORT(log_level)(int level) {
    return log_level(level);
}

extern "C" const char *SWR_EXPORT(log_label)(int level) {
    return log_label(level);
}

extern "C" const char *SWR_EXPORT(log_rotation)(int rotation) {
    return log_rotation(rotation);
}

// `pretty` copied into `output` (capacity `length`); returns its length.
extern "C" size_t SWR_EXPORT(pretty)(const char *name, size_t length, bool strip, char *output) {
    const std::string result = pretty(std::string(name, length), strip);
    memcpy(output, result.data(), result.size());
    return result.size();
}

// `log_line` with errno set to EINTR first; `error` is errno after it.
extern "C" size_t SWR_EXPORT(log_line)(char *output,
                                       size_t capacity,
                                       const char *date,
                                       int date_precision,
                                       char symbol,
                                       int pid,
                                       int worker,
                                       int level,
                                       const char *content,
                                       int content_precision,
                                       int *error) {
    errno = EINTR;
    const size_t n =
        log_line(output, capacity, date, date_precision, symbol, pid, worker, level, content, content_precision);
    *error = errno;
    return n;
}

// `log_micros` with errno set to EINTR first; `error` is errno after it.
extern "C" size_t SWR_EXPORT(log_micros)(char *output, size_t capacity, long long micros, int *error) {
    errno = EINTR;
    const size_t n = log_micros(output, capacity, micros);
    *error = errno;
    return n;
}
