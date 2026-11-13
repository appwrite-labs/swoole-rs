/* Stays C (docs/zend.md, What stays C): bailout (stream casts), version
 * (zend_zval_value_name from 8.3) and budget (Z_OBJPROP_P, convert_to_array,
 * php_stream_set_option). */
#include "shim.h"
#include "shim_socket.h"
#include "ext/standard/file.h"

int swr_zend_stream_socketd(zval *zstream, php_stream **stream, int *fd) {
    volatile int result = 1;
    SWR_ZEND_TRY {
        *stream = (php_stream *) zend_fetch_resource2_ex(zstream, "stream", php_file_le_stream(), php_file_le_pstream());
        if (*stream && php_stream_cast(*stream, PHP_STREAM_AS_SOCKETD, (void **) fd, 1) == SUCCESS) {
            result = 0;
        }
    }
    SWR_ZEND_CATCH {
        result = 2;
    }
    SWR_ZEND_END_TRY();
    return result;
}

void swr_zend_stream_unbuffer(php_stream *stream) {
    php_stream_set_option(stream, PHP_STREAM_OPTION_READ_BUFFER, PHP_STREAM_BUFFER_NONE, NULL);
}

HashTable *swr_zend_object_properties(zval *object) {
    return Z_OBJPROP_P(object);
}

HashTable *swr_zend_convert_to_array(zval *value) {
    convert_to_array(value);
    return Z_ARRVAL_P(value);
}

/* Whether `value` is a PHP value or a reference to one: a slot the engine keeps for
 * itself (IS_INDIRECT, IS_PTR, ...) has no type name. */
static bool swr_zval_named(const zval *value) {
    uint8_t kind = Z_TYPE_P(value);
    if (kind == IS_REFERENCE) {
        kind = Z_TYPE_P(Z_REFVAL_P(value));
    }
    return kind <= IS_RESOURCE;
}

const char *swr_zend_zval_value_name(const zval *value) {
    if (!swr_zval_named(value)) {
        return "mixed";
    }
#if PHP_VERSION_ID >= 80300
    return zend_zval_value_name(value);
#else
    return zend_zval_type_name(value);
#endif
}

const char *swr_zend_zval_type_name(const zval *value) {
    return swr_zval_named(value) ? zend_zval_type_name(value) : "mixed";
}
