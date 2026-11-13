/* Stays C (docs/zend.md, What stays C): bailout (MINIT hooks, ini registration,
 * phpinfo), globals (CG(compiler_options), SIGG, EG(function_table)) and version
 * (register_user_shutdown_function in 8.5, ZEND_COMPILE_PRELOAD,
 * PHP_STREAM_FLAG_NO_CLOSE). */
#include "shim.h"
#include "shim_module.h"
#include <dlfcn.h>
#include <unistd.h>
#include "SAPI.h"
#include "php_main.h"
#include "php_open_temporary_file.h"
#include "ext/standard/basic_functions.h"
#include "ext/standard/file.h"

int swr_zend_try_hook(void (*hook)(void)) {
    volatile int bailed = 0;
    SWR_ZEND_TRY {
        hook();
    }
    SWR_ZEND_CATCH {
        bailed = 1;
    }
    SWR_ZEND_END_TRY();
    return bailed;
}

const char *swr_zend_sapi_name(void) {
    return sapi_module.name;
}

const char *swr_zend_temporary_directory(void) {
    return php_get_temporary_directory();
}

int swr_zend_compiling_preload(void) {
#ifdef ZEND_COMPILE_PRELOAD
    return (CG(compiler_options) & ZEND_COMPILE_PRELOAD) != 0;
#else
    return 0;
#endif
}

void *swr_zend_execute_ex_reset(void) {
    if (EXPECTED(zend_execute_ex == execute_ex)) {
        return NULL;
    }
    void (*hook)(zend_execute_data *) = zend_execute_ex;
    zend_execute_ex = execute_ex;
    return (void *) hook;
}

void swr_zend_execute_ex_restore(void *hook) {
    if (hook) {
        zend_execute_ex = (void (*)(zend_execute_data *)) hook;
    }
}

void swr_zend_signals_uncheck(void) {
#ifdef ZEND_SIGNALS
    /* Disable warning even in ZEND_DEBUG because we may register our own signal handlers */
    SIGG(check) = 0;
#endif
}

static pid_t loader;
__attribute__((constructor)) static void swr_zend_loaded(void) {
    loader = getpid();
}

void swr_zend_leak_check(void) {
    int (*check)(void) = (int (*)(void)) dlsym(RTLD_DEFAULT, "__lsan_do_recoverable_leak_check");
    if (check && getpid() == loader) {
        check();
    }
}

void swr_zend_register_shutdown_function(const char *name) {
#if PHP_VERSION_ID >= 80500
    php_shutdown_function_entry entry = {
        .fci_cache = empty_fcall_info_cache,
        .params = NULL,
        .param_count = 0,
    };
    size_t length = strlen(name);
    zend_function *function = (zend_function *) zend_hash_str_find_ptr(EG(function_table), name, length);
    ZEND_ASSERT(function);
    entry.fci_cache.function_handler = function;
    register_user_shutdown_function(name, length, &entry);
#else
    php_shutdown_function_entry entry;
    zval function_name;
    /* user_shutdown_function_dtor releases the name, so it is not freed here. */
    ZVAL_STRING(&function_name, name);
    zend_fcall_info_init(&function_name, 0, &entry.fci, &entry.fci_cache, NULL, NULL);
    register_user_shutdown_function(Z_STRVAL(function_name), Z_STRLEN(function_name), &entry);
#endif
}

void swr_zend_stream_constant_no_close(const char *name, size_t length) {
#ifdef PHP_STREAM_FLAG_NO_CLOSE
    zval *zstream = zend_get_constant_str(name, length);
    if (!zstream) {
        return;
    }
    php_stream *stream =
        (php_stream *) zend_fetch_resource2_ex(zstream, NULL, php_file_le_stream(), php_file_le_pstream());
    if (!stream) {
        return;
    }
    stream->flags |= PHP_STREAM_FLAG_NO_CLOSE;
#endif
}

int swr_zend_try_module_ini(const zend_ini_entry_def *entries, int number, int type) {
    volatile int bailed = 0;
    SWR_ZEND_TRY { zend_register_ini_entries_ex(entries, number, type); }
    SWR_ZEND_CATCH { bailed = 1; } SWR_ZEND_END_TRY();
    return bailed;
}

#include "ext/standard/info.h"
int swr_zend_try_info_start(void) {
    volatile int bailed = 0;
    SWR_ZEND_TRY { php_info_print_table_start(); } SWR_ZEND_CATCH { bailed = 1; } SWR_ZEND_END_TRY();
    return bailed;
}
int swr_zend_try_info_header(const char *key, const char *value) {
    volatile int bailed = 0;
    SWR_ZEND_TRY { php_info_print_table_header(2, key, value); } SWR_ZEND_CATCH { bailed = 1; } SWR_ZEND_END_TRY();
    return bailed;
}
int swr_zend_try_info_row(const char *key, const char *value) {
    volatile int bailed = 0;
    SWR_ZEND_TRY { php_info_print_table_row(2, key, value); } SWR_ZEND_CATCH { bailed = 1; } SWR_ZEND_END_TRY();
    return bailed;
}
int swr_zend_try_info_end(void) {
    volatile int bailed = 0;
    SWR_ZEND_TRY { php_info_print_table_end(); } SWR_ZEND_CATCH { bailed = 1; } SWR_ZEND_END_TRY();
    return bailed;
}
int swr_zend_try_info_ini(void *module) {
    volatile int bailed = 0;
    SWR_ZEND_TRY { display_ini_entries((zend_module_entry *) module); } SWR_ZEND_CATCH { bailed = 1; } SWR_ZEND_END_TRY();
    return bailed;
}
