#include "shim.h"
#include "shim_thread.h"

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
    }
    ZEND_HASH_FOREACH_END();
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

void swr_zend_stream_no_close(php_stream *stream) {
    stream->flags |= PHP_STREAM_FLAG_NO_CLOSE;
}

void swr_zend_register_stream_constant(const char *name, php_stream *stream) {
    zend_constant constant;
    php_stream_to_zval(stream, &constant.value);
    ZEND_CONSTANT_SET_FLAGS(&constant, CONST_CS, 0);
    constant.name = zend_string_init_interned(name, strlen(name), false);
    zend_register_constant(&constant);
}
