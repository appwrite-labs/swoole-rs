/* Stays C: bailout and budget, as detailed below. */
#include "shim.h"
#include "shim_socket.h"
#include "ext/standard/file.h"
#include "php_network.h"
#include <netdb.h>

/* bailout: stream casting and its error path can call PHP. */
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

/* budget: PHP returns a raw hostent with a pointer array; copy its first IPv4 address while valid. */
int swr_zend_gethostbyname4(const char *host, struct in_addr *address) {
    struct hostent *entry = php_network_gethostbyname(host);
    if (!entry) {
        return -1;
    }
    if (entry->h_addrtype != AF_INET) {
        return 1;
    }
    memcpy(address, entry->h_addr_list[0], sizeof *address);
    return 0;
}

/* bailout: get_properties may initialize a lazy object. */
int swr_zend_try_object_properties(zend_object *object, zval *out) {
    HashTable *volatile table = NULL;
    volatile int bailed = 0;
    SWR_ZEND_TRY { table = object->handlers->get_properties(object); }
    SWR_ZEND_CATCH { bailed = 1; } SWR_ZEND_END_TRY();
    if (table) {
        GC_TRY_ADDREF(table);
        ZVAL_ARR(out, table);
        if (GC_FLAGS(table) & GC_IMMUTABLE) {
            Z_TYPE_INFO_P(out) = IS_ARRAY;
        }
    }
    return bailed;
}

/* bailout: object conversion may execute a handler. */
int swr_zend_try_convert_to_array(zval *value) {
    volatile int bailed = 0;
    SWR_ZEND_TRY { convert_to_array(value); }
    SWR_ZEND_CATCH { bailed = 1; } SWR_ZEND_END_TRY();
    return bailed;
}
