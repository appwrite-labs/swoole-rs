// Upstream's number helpers behind C entry points: build.rs copies the pinned base.cc
// bodies into corebase.inc unchanged. Compiled in the target's plain `char` mode, which
// swoole_type_size switches on and swoole_hex2dec promotes.
#include <cassert>
#include <cctype>
#include <cstdint>
#include <cstdlib>
#include <cstring>
#include <stdexcept>
#include <string>
#include <strings.h>

namespace corebase_upstream {
using ulong_t = unsigned long;
// swoole.h's choice where configure finds strndup.
#define sw_strndup strndup
#include "corebase.inc"
#undef sw_strndup
}  // namespace corebase_upstream

extern "C" int swr_oracle_corebase_type_size(char type) {
    return corebase_upstream::swoole_type_size(type);
}

extern "C" char *swr_oracle_corebase_dec2hex(unsigned long value, int base) {
    return corebase_upstream::swoole_dec2hex(value, base);
}

extern "C" unsigned long swr_oracle_corebase_hex2dec(const char *hex, size_t *parsed) {
    return corebase_upstream::swoole_hex2dec(hex, parsed);
}

extern "C" int swr_oracle_corebase_version_compare(const char *left, const char *right) {
    return corebase_upstream::swoole_version_compare(left, right);
}

extern "C" uint32_t swr_oracle_corebase_common_divisor(uint32_t u, uint32_t v) {
    return corebase_upstream::swoole_common_divisor(u, v);
}

extern "C" uint32_t swr_oracle_corebase_common_multiple(uint32_t u, uint32_t v) {
    return corebase_upstream::swoole_common_multiple(u, v);
}

extern "C" int swr_oracle_corebase_itoa(char *buf, long value) {
    return corebase_upstream::swoole_itoa(buf, value);
}

// swoole_get_env: 1 with `*value` set, 0 for an unset variable, -1 where std::stoi threw
// std::invalid_argument and -2 where it threw std::out_of_range.
extern "C" int swr_oracle_corebase_get_env(const char *name, int *value) {
    try {
        return corebase_upstream::swoole_get_env(name, value) ? 1 : 0;
    } catch (const std::invalid_argument &) {
        return -1;
    } catch (const std::out_of_range &) {
        return -2;
    }
}
