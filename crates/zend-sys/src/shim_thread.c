/* Stays C (docs/zend.md, What stays C): globals (PG, SG and EG of a ZTS request),
 * version (ZTS-only TSRM calls, refresh_memory_manager), bailout (a thread's
 * request, stream casts), allocator and budget (hash iteration). */
#include "shim.h"
#include "shim_thread.h"
#include "SAPI.h"
#include "php_main.h"
#include "zend_exceptions.h"

bool swr_zend_array_is_list(const zend_array *array) {
    return zend_array_is_list((zend_array *) array);
}

/* ZEND_HASH_FOREACH's walk, one element per call: the next defined slot at or
 * after *position, which then points past it. */
zval *swr_zend_hash_next(const HashTable *table, HashPosition *position, zend_string **key, zend_ulong *index) {
    for (uint32_t at = *position; at < table->nNumUsed; at++) {
        bool packed = HT_IS_PACKED(table);
        zval *value = packed ? &table->arPacked[at] : &table->arData[at].val;
        if (Z_TYPE_P(value) == IS_UNDEF) continue;
        *key = packed ? NULL : table->arData[at].key;
        *index = packed ? at : table->arData[at].h;
        *position = at + 1;
        return value;
    }
    *position = table->nNumUsed;
    return NULL;
}

bool swr_zend_handle_numeric_str(const zend_string *key, zend_ulong *index) {
    return _zend_handle_numeric_str(ZSTR_VAL(key), ZSTR_LEN(key), index);
}

zend_long swr_zend_dval_to_lval_safe(double value) {
    return zend_dval_to_lval_safe(value);
}

zend_long swr_zend_resource_handle(const zval *value) {
    return Z_TYPE_P(value) == IS_RESOURCE ? Z_RES_HANDLE_P(value) : 0;
}

int swr_zend_smart_strcmp(zend_string *a, zend_string *b) {
    return zendi_smart_strcmp(a, b);
}

php_stream *swr_zend_stream_open(const char *path, const char *mode) {
    return php_stream_open_wrapper_ex(path, mode, 0, NULL, NULL);
}

int swr_zend_stream_select_fd(zval *value, int *fd) {
    php_stream *stream;
    php_stream_from_zval_no_verify(stream, value);
    if (!stream) {
        return 1;
    }
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

/* The jump buffer of this thread's script scope, while its script runs. */
ZEND_TLS JMP_BUF *script_bailout;

void swr_zend_thread_enter(void) {
#ifdef ZTS
    ts_resource(0);
    PG(expose_php) = false;
    PG(auto_globals_jit) = true;
    PG(enable_dl) = false;
#endif
}

int swr_zend_thread_request(const char *file, bool internal, const char *path_translated, int argc,
                            void (*prologue)(void *, zval *),
                            void (*epilogue)(void *), void *context) {
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
    /* Argument copies stay addressable if the internal call bails out. */
    struct invocation {
        zend_fcall_info fci;
        zval args;
        zval result;
    } *call = internal ? ecalloc(1, sizeof(*call)) : NULL;
    if (!internal) {
        zend_stream_init_filename(&file_handle, file);
        file_handle.primary_script = true;
    }
    zend_first_try {
        script_bailout = EG(bailout);
        prologue(context, call ? &call->args : NULL);
        if (call) {
            zend_function *fn = zend_hash_str_find_ptr(EG(function_table), file, strlen(file));
            if (!fn || fn->type != ZEND_INTERNAL_FUNCTION) {
                zend_throw_error(NULL, "Internal thread entry %s is not available in this request", file);
            } else if (Z_TYPE(call->args) != IS_ARRAY || zend_fcall_info_args_ex(&call->fci, fn, &call->args) == SUCCESS) {
                zend_call_known_function(fn, NULL, NULL, &call->result, call->fci.param_count, call->fci.params, NULL);
            }
            if (EG(exception)) {
                zend_exception_error(EG(exception), E_ERROR);
            }
        } else {
            php_execute_script(&file_handle);
        }
    }
    zend_end_try();
    script_bailout = NULL;
    epilogue(context);
    if (call) {
        zend_first_try {
            zend_fcall_info_args_clear(&call->fci, true);
            zval_ptr_dtor(&call->args);
            zval_ptr_dtor(&call->result);
        }
        zend_end_try();
        efree(call);
    } else {
        zend_destroy_file_handle(&file_handle);
    }
    php_request_shutdown(NULL);
    return EG(exit_status);
#else
    (void) internal;
    (void) file;
    (void) path_translated;
    (void) argc;
    (void) prologue;
    (void) epilogue;
    (void) context;
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

void swr_zend_thread_set_argv(const zval *argv, zend_long argc) {
    zval copy, count;
    ZVAL_COPY(&copy, argv);
    ZVAL_LONG(&count, argc);
    zend_hash_update(&EG(symbol_table), ZSTR_KNOWN(ZEND_STR_ARGV), &copy);
    zend_hash_update(&EG(symbol_table), ZSTR_KNOWN(ZEND_STR_ARGC), &count);
}

void swr_zend_thread_script_bailout(void) {
    if (script_bailout) {
        EG(bailout) = script_bailout;
    } else {
        swr_zend_guard_clear();
    }
    zend_bailout();
}
