// The pinned get_method and parse_multipart_boundary from http.cc and swoole.h's
// swoole_strcaseeq and swoole_str_istarts_with behind C entry points: build.rs copies
// them unchanged. Each folds case through libc's strncasecmp, so in the process
// locale, as the retired tests/modules/http/locale_gate.py's oracle did. The candidate
// is crates/http's helpers (tests/modules/http/locale.rs).
#include <cstddef>
#include <cstdint>
#include <cstring>
#include <strings.h>

namespace upstream_locale {
#include "http_frame_enums.inc"
#include "http_locale.inc"
}  // namespace upstream_locale

extern "C" {
int swr_oracle_http_get_method(const char *method, size_t length) {
    return upstream_locale::get_method(method, length);
}
// The boundary's offset in `at` and its length, written only when one is found.
bool swr_oracle_http_boundary(const char *at, size_t length, size_t offset, size_t *found_offset, int *found_length) {
    char *boundary = nullptr;
    int boundary_length = -1;
    bool found = upstream_locale::parse_multipart_boundary(at, length, offset, &boundary, &boundary_length);
    if (found) {
        *found_offset = boundary - at;
        *found_length = boundary_length;
    }
    return found;
}
unsigned swr_oracle_http_strcaseeq(const char *a, size_t a_length, const char *b, size_t b_length) {
    return upstream_locale::swoole_strcaseeq(a, a_length, b, b_length);
}
unsigned swr_oracle_http_istarts_with(const char *text, size_t length, const char *prefix, size_t prefix_length) {
    return upstream_locale::swoole_str_istarts_with(text, length, prefix, prefix_length);
}
}
