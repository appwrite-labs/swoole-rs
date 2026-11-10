/* Stays C (docs/zend.md, What stays C): globals (PG, SG and EG of a ZTS request),
 * version (ZTS-only TSRM calls, refresh_memory_manager), bailout (a thread's
 * request, stream casts), allocator and budget (hash iteration, numeric strings). */
#include "shim.h"
#include "shim_thread.h"
#include "SAPI.h"
#include "php_main.h"

bool swr_zend_array_is_list(const zend_array *array) {
    return zend_array_is_list((zend_array *) array);
}

void swr_zend_hash_foreach_kv(const HashTable *table,
                              void (*each)(zend_string *key, zend_ulong index, zval *value, void *context),
                              void *context) {
    zend_string *key;
    zend_ulong index;
    zval *value;
    ZEND_HASH_FOREACH_KEY_VAL((HashTable *) table, index, key, value) {
        ZVAL_DEREF(value);
        each(key, index, value, context);
        if (swr_zend_guard_pending()) break;
    }
    ZEND_HASH_FOREACH_END();
}

zval *swr_zend_hash_next(const HashTable *table, HashPosition *position, zend_string **key, zend_ulong *index) {
    zval *value = zend_hash_get_current_data_ex((HashTable *) table, position);
    if (!value) return NULL;
    if (zend_hash_get_current_key_ex((HashTable *) table, key, index, position) != HASH_KEY_IS_STRING) *key = NULL;
    zend_hash_move_forward_ex((HashTable *) table, position);
    return value;
}

bool swr_zend_handle_numeric_str(const char *key, size_t length, zend_ulong *index) {
    return _zend_handle_numeric_str(key, length, index);
}

zend_long swr_zend_dval_to_lval_safe(double value) {
    return zend_dval_to_lval_safe(value);
}

zend_long swr_zend_resource_handle(const zval *value) {
    return Z_RES_HANDLE_P(value);
}

uint8_t swr_zend_is_numeric_string(const char *text, size_t length, zend_long *lval, double *dval) {
    return is_numeric_string(text, length, lval, dval, false);
}

int swr_zend_smart_strcmp(zend_string *a, zend_string *b) {
    return zendi_smart_strcmp(a, b);
}

php_stream *swr_zend_stream_open(const char *path, const char *mode) {
    return php_stream_open_wrapper_ex(path, mode, 0, NULL, NULL);
}

int swr_zend_stream_select_fd(php_stream *stream, int *fd) {
    volatile int result = 1;
    SWR_ZEND_TRY {
        if (php_stream_cast(stream, PHP_STREAM_AS_FD_FOR_SELECT | PHP_STREAM_CAST_INTERNAL, (void **) fd, 1) ==
            SUCCESS) {
            result = 0;
        }
    }
    SWR_ZEND_CATCH {
        result = 2;
    }
    SWR_ZEND_END_TRY();
    return result;
}

bool swr_zend_stream_fd(php_stream *stream, int *fd) {
    return php_stream_cast(stream, PHP_STREAM_AS_FD, (void **) fd, 0) == SUCCESS;
}

void swr_zend_register_stream_constant(const char *name, php_stream *stream) {
    zend_constant constant;
    php_stream_to_zval(stream, &constant.value);
    ZEND_CONSTANT_SET_FLAGS(&constant, CONST_CS, 0);
    constant.name = zend_string_init_interned(name, strlen(name), false);
    zend_register_constant(&constant);
}

void swr_zend_thread_enter(void) {
#ifdef ZTS
    ts_resource(0);
    PG(expose_php) = false;
    PG(auto_globals_jit) = true;
    PG(enable_dl) = false;
#endif
}

int swr_zend_thread_request(const char *file, const char *path_translated, int argc, void (*prologue)(void *),
                            void (*epilogue)(void *), void *context, void **bailout) {
#ifdef ZTS
    if (php_request_startup() != SUCCESS) {
        EG(exit_status) = 1;
        return 1;
    }
    PG(during_request_startup) = false;
    SG(sapi_started) = false;
    SG(headers_sent) = 1;
    SG(request_info).no_headers = true;
    SG(request_info).path_translated = (char *) path_translated;
    SG(request_info).argc = argc;
#if PHP_VERSION_ID >= 80500
    refresh_memory_manager();
#endif
    zend_file_handle file_handle;
    zend_stream_init_filename(&file_handle, file);
    file_handle.primary_script = true;
    zend_first_try {
        *bailout = EG(bailout);
        prologue(context);
        php_execute_script(&file_handle);
    }
    zend_end_try();
    *bailout = NULL;
    epilogue(context);
    zend_destroy_file_handle(&file_handle);
    php_request_shutdown(NULL);
    return EG(exit_status);
#else
    return 1;
#endif
}

void swr_zend_thread_leave(void) {
#ifdef ZTS
    ts_free_thread();
#endif
}

zval *swr_zend_thread_main_request(const char **path, int *argc) {
    *path = SG(request_info).path_translated;
    *argc = SG(request_info).argc;
    return zend_hash_find_ind(&EG(symbol_table), ZSTR_KNOWN(ZEND_STR_ARGV));
}

bool swr_zend_tsrm_is_main_thread(void) {
#ifdef ZTS
    return tsrm_is_main_thread();
#else
    return true;
#endif
}

bool swr_zend_tsrm_is_shutdown(void) {
#ifdef ZTS
    return tsrm_is_shutdown();
#else
    return false;
#endif
}

const char *swr_zend_tsrm_api_name(void) {
#ifdef ZTS
    return tsrm_api_name();
#else
    return "";
#endif
}

void swr_zend_thread_set_argv(zval *argv, zend_long argc) {
    zval count;
    ZVAL_LONG(&count, argc);
    zend_hash_update(&EG(symbol_table), ZSTR_KNOWN(ZEND_STR_ARGV), argv);
    zend_hash_update(&EG(symbol_table), ZSTR_KNOWN(ZEND_STR_ARGC), &count);
}

void swr_zend_bailout_to(void *target) {
    if (target) {
        EG(bailout) = (JMP_BUF *) target;
    }
    zend_bailout();
}
