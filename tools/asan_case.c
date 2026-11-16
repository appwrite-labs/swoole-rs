// compiler-rt's case-comparison interceptors fold ASCII regardless of LC_CTYPE.
// The sanitizer build links these strong definitions instead: libc's bytewise
// semantics, with every read instrumented by ASan/UBSan. No production build
// uses them, and the oracle bodies and locale tests stay unchanged.
#include <ctype.h>
#include <stddef.h>

int strncasecmp(const char *left, const char *right, size_t length) {
    while (length--) {
        unsigned char a = *left++, b = *right++;
        int difference = tolower(a) - tolower(b);
        if (difference || !a) return difference;
    }
    return 0;
}

int strcasecmp(const char *left, const char *right) {
    return strncasecmp(left, right, (size_t)-1);
}
