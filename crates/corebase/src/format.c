/* SPDX-License-Identifier: Apache-2.0 */
/* Compatibility derived from Swoole v6.2.4 src/core/base.cc (Swoole contributors). */
/* base.cc's printf family, which stable Rust can't define (C variadics); Rust
 * bounds the result (crates/corebase/src/host.rs) and exports each under its public
 * name (lib.rs's format_exports!): a cdylib exports only Rust's symbols. */
#include <stdarg.h>
#include <stdbool.h>
#include <stddef.h>
#include <stdio.h>

size_t swr_v1_core_format_result(char *buf, size_t size, int formatted, bool zero_query);
void *swr_v1_core_stdout(void);

size_t swr_c_sw_vsnprintf(char *buf, size_t size, const char *format, va_list args) {
    return swr_v1_core_format_result(buf, size, vsnprintf(buf, size, format, args), false);
}

size_t swr_c_sw_snprintf(char *buf, size_t size, const char *format, ...) {
    va_list args;
    va_start(args, format);
    int retval = vsnprintf(buf, size, format, args);
    va_end(args);
    return swr_v1_core_format_result(buf, size, retval, true);
}

int swr_c_sw_printf(const char *format, ...) {
    va_list args;
    va_start(args, format);
    int retval = vfprintf(swr_v1_core_stdout(), format, args);
    va_end(args);
    return retval;
}
