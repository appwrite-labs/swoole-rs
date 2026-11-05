#include "shim_host.h"
#include <stdarg.h>
#include <stdio.h>
#include <stdlib.h>
#include "zend_builtin_functions.h"
#ifdef ZEND_MAX_EXECUTION_TIMERS
#include "zend_max_execution_timer.h"
#endif

/* crates/phpswoole. */
extern zend_class_entry *swoole_error_ce;
void swr_v1_phpswoole_fatal_exit(int code);

void swr_zend_module_template(zend_module_entry *entry) {
    static const zend_module_entry header = {STANDARD_MODULE_HEADER_EX, NULL, NULL, NULL, NULL, NULL, NULL,
                                             NULL, NULL, NULL, NULL, STANDARD_MODULE_PROPERTIES};
    *entry = header;
}

void swr_zend_after_fork(void *unused) {
    (void) unused;
#if PHP_VERSION_ID >= 80500
    refresh_memory_manager();
#endif
#ifdef ZEND_MAX_EXECUTION_TIMERS
    zend_max_execution_timer_init();
#endif
}

void swr_zend_print_stack_trace(void) {
    fprintf(stderr, "\nStack trace:\n");
    zval backtrace;
    zend_fetch_debug_backtrace(&backtrace, 0, 0, 0);
    zend_string *trace = zend_trace_to_string(Z_ARRVAL(backtrace), false);
    ZEND_WRITE(ZSTR_VAL(trace), ZSTR_LEN(trace));
    zend_string_release(trace);
    zval_ptr_dtor(&backtrace);
}

void swr_zend_fatal_error(int code, const char *format, ...) {
    va_list args, copy;
    va_start(args, format);
    va_copy(copy, args);
    int length = vsnprintf(NULL, 0, format, copy);
    va_end(copy);
    char *message = length < 0 ? NULL : malloc((size_t) length + 1);
    if (message) {
        vsnprintf(message, (size_t) length + 1, format, args);
    }
    va_end(args);
    zend_object *exception = zend_throw_exception(swoole_error_ce, message ? message : "", code);
    free(message);
    /* Prints the exception; the VM is not terminated. */
    zend_exception_error(exception, E_ERROR);
    swr_v1_phpswoole_fatal_exit(code);
}
