/* Stays C (docs/zend.md, What stays C): variadic (SwooleG.fatal_error), version
 * (STANDARD_MODULE_HEADER_EX, refresh_memory_manager, the execution timers,
 * PHP_VERSION), globals (the TSRM module globals, EG(exit_status)) and allocator
 * (the backtrace's string and array). */
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
/* crates/phpprocess. */
void php_swoole_process_signal_clean(void);

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
    /* swoole_fork() has removed the inherited signal handlers. */
    php_swoole_process_signal_clean();
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

#ifdef ZTS
ts_rsrc_id swoole_globals_id;
#endif

void *swr_zend_module_globals_init(void *nts, size_t size, void (*ctor)(void *)) {
#ifdef ZTS
    (void) nts;
    ts_allocate_id(&swoole_globals_id, size, (ts_allocate_ctor) ctor, NULL);
    return &swoole_globals_id;
#else
    (void) size;
    ctor(nts);
    return nts;
#endif
}

void *swr_zend_module_globals(void *nts) {
#ifdef ZTS
    (void) nts;
    return TSRMG_BULK(swoole_globals_id, void *);
#else
    return nts;
#endif
}

const char *swr_zend_php_version(void) {
    return PHP_VERSION;
}

void swr_zend_set_exit_status(int status) {
    EG(exit_status) = status;
}
