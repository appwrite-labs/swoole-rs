/* SPDX-License-Identifier: Apache-2.0 */
/* Compatibility derived from Swoole v6.2.4 src/core/base.cc (Swoole contributors). */
/* base.cc's C variadics, which stable Rust can't define: the printf family, whose
 * result Rust bounds (crates/corebase/src/host.rs), and the fatal-error hook. */
#include "swoole_rs_corebase_state.h"

#include <stdarg.h>
#include <stdio.h>

void *swr_v1_core_stdout(void);

size_t sw_vsnprintf(char *buf, size_t size, const char *format, va_list args) {
    return swr_v1_core_format_result(buf, size, vsnprintf(buf, size, format, args), false);
}

size_t sw_snprintf(char *buf, size_t size, const char *format, ...) {
    va_list args;
    va_start(args, format);
    int retval = vsnprintf(buf, size, format, args);
    va_end(args);
    return swr_v1_core_format_result(buf, size, retval, true);
}

int sw_printf(const char *format, ...) {
    va_list args;
    va_start(args, format);
    int retval = vfprintf(swr_v1_core_stdout(), format, args);
    va_end(args);
    return retval;
}

void swr_v1_core_fatal_error_hook(int code, const char *format, ...) {
    swr_v1_core_error_line line;
    size_t length = sw_snprintf(line.text, sizeof line.text, "(ERROR %d): ", code);
    va_list args;
    va_start(args, format);
    sw_vsnprintf(line.text + length, sizeof line.text - length, format, args);
    va_end(args);
    swr_v1_core_fatal_line(&line);
}
