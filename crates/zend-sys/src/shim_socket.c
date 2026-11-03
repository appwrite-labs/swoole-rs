#include "shim_socket.h"
#include "ext/standard/file.h"

int swr_zend_stream_socketd(zval *zstream, php_stream **stream, int *fd) {
    volatile int result = 1;
    zend_try {
        *stream = (php_stream *) zend_fetch_resource2_ex(zstream, "stream", php_file_le_stream(), php_file_le_pstream());
        if (*stream && php_stream_cast(*stream, PHP_STREAM_AS_SOCKETD, (void **) fd, 1) == SUCCESS) {
            result = 0;
        }
    }
    zend_catch {
        result = 2;
    }
    zend_end_try();
    return result;
}

void swr_zend_stream_unbuffer(php_stream *stream) {
    php_stream_set_option(stream, PHP_STREAM_OPTION_READ_BUFFER, PHP_STREAM_BUFFER_NONE, NULL);
}

HashTable *swr_zend_object_properties(zval *object) {
    return Z_OBJPROP_P(object);
}

const char *swr_zend_zval_value_name(const zval *value) {
#if PHP_VERSION_ID >= 80300
    return zend_zval_value_name(value);
#else
    return zend_zval_type_name(value);
#endif
}
