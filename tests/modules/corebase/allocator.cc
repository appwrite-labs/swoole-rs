// SPDX-License-Identifier: Apache-2.0
// Test-only standard strndup interposer. Native allocation runs after Rust returns.
#include "swoole.h"
#include "swoole_protocol.h"
#include <cstdlib>
#include <cstring>
#include <cerrno>

static bool fail_allocation;
static size_t allocations;
static size_t requested;

extern "C" char *strndup(const char *input, size_t maximum)
#ifdef __linux__
    noexcept
#endif
{
    ++allocations;
    requested = maximum;
    // A real native allocator may call back into numeric helpers without a borrow.
    size_t parsed = 0;
    if (swoole_hex2dec("FFtail", &parsed) != 16833333 || parsed != 6) std::abort();
    if (fail_allocation) {
        errno = ENOMEM;
        return nullptr;
    }
    size_t length = strnlen(input, maximum);
    char *result = static_cast<char *>(malloc(length + 1));
    if (result) {
        memcpy(result, input, length);
        result[length] = 0;
    }
    return result;
}

int main() {
    SwooleG.std_allocator = {malloc, calloc, realloc, free};
    allocations = 0;
    char *result = swoole_dec2hex(2684326179UL, 16);
    if (!result || strcmp(result, "9fff9123") || allocations != 1 || requested != 8) return 1;
    sw_free(result);
    fail_allocation = true;
    errno = 0;
    result = swoole_dec2hex(0, 2);
    if (result || allocations != 2 || requested != 1 || errno != ENOMEM) return 2;
    fail_allocation = false;
    result = swoole_dec2hex(0, 36);
    if (!result || strcmp(result, "0") || allocations != 3 || requested != 1) return 3;
    sw_free(result);
}
