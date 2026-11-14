/* SPDX-License-Identifier: Apache-2.0 */
/* Compatibility derived from Swoole v6.2.4 src/core/base.cc (Swoole contributors). */
/* base.cc's fatal_error, SwooleG.fatal_error until PHP's replaces it, and libc's
 * stdout: a C variadic and a macro, which stable Rust can't define or name. */
#include <stdarg.h>
#include <stddef.h>
#include <stdio.h>

char *swr_v1_error_buffer(void);
size_t sw_snprintf(char *buf, size_t size, const char *format, ...);
size_t sw_vsnprintf(char *buf, size_t size, const char *format, va_list args);
void *swr_v1_core_logger(void);
void swr_v1_logger_put(const void *logger, int level, const char *content, size_t length);
void swr_v1_ffi_exit(int status);

#define ERROR_MSG_SIZE 16384 /* SW_ERROR_MSG_SIZE */
#define LOG_ERROR 5          /* SW_LOG_ERROR */

void swr_v1_ffi_fatal_error(int code, const char *format, ...) {
    char *error = swr_v1_error_buffer();
    va_list args;
    size_t length = sw_snprintf(error, ERROR_MSG_SIZE, "(ERROR %d): ", code);
    va_start(args, format);
    length += sw_vsnprintf(error + length, ERROR_MSG_SIZE - length, format, args);
    va_end(args);
    /* SwooleG.logger, a swoole::Logger, whose one member is crates/utilities' Logger. */
    void *const *logger = swr_v1_core_logger();
    if (logger) {
        swr_v1_logger_put(*logger, LOG_ERROR, error, length);
    }
    swr_v1_ffi_exit(1);
}

void *swr_v1_ffi_stdout(void) {
    return stdout;
}
