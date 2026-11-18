#include "swoole_rs_phphost.h"
#ifdef SW_USE_CURL
#include "swoole_rs_curl.h"
#include <sys/uio.h>

extern "C" {
zend_class_entry *swoole_coroutine_curl_handle_ce;
zend_object_handlers swoole_coroutine_curl_handle_handlers;
extern const size_t swoole_curl_object_size = sizeof(php_curl);
extern const size_t swoole_curl_object_offset = XtOffsetOf(php_curl, std);
}

#if PHP_VERSION_ID >= 80400
static const zend_object_handlers *original_handlers;
extern "C" void swoole_curl_free_original(zend_object *object) {
    original_handlers->free_obj(object);
}
#endif

/* Install the Rust object handlers in the system curl class at module startup.
 * The fields in Zend's handler table and the php_curl offset are ABI-specific. */
extern "C" void swoole_curl_register_handle() {
    auto &handlers = swoole_coroutine_curl_handle_handlers;
    swoole_coroutine_curl_handle_ce = curl_ce;
    curl_ce->create_object = swoole_curl_create_object;
#if PHP_VERSION_ID >= 80400
    original_handlers = curl_ce->default_object_handlers;
#endif
#if PHP_VERSION_ID >= 80300
    curl_ce->default_object_handlers = &handlers;
#endif
    handlers = std_object_handlers;
    handlers.offset = XtOffsetOf(php_curl, std);
    handlers.free_obj = swoole_curl_free_obj;
    handlers.get_gc = swoole_curl_get_gc;
    handlers.get_constructor = swoole_curl_get_constructor;
    handlers.clone_obj = swoole_curl_clone_obj;
    handlers.cast_object = swoole_curl_cast_object;
    handlers.compare = [](zval *, zval *) { return ZEND_UNCOMPARABLE; };
    curl_ce->ce_flags |= ZEND_ACC_FINAL | ZEND_ACC_NO_DYNAMIC_PROPERTIES;
#if PHP_VERSION_ID < 80400
    zend_declare_property_null(curl_ce, ZEND_STRL("private_data"), ZEND_ACC_PUBLIC);
#endif
}

extern "C" php_curl *swoole_curl_new_clone(zval *result) {
    zend_object *object = swoole_curl_create_object(swoole_coroutine_curl_handle_ce);
    ZVAL_OBJ(result, object);
    return curl_from_obj(object);
}

extern "C" void swoole_curl_set_easy(php_curl *ch, CURL *easy) {
    ch->cp = easy;
}

/* The two bundled php_curl layouts share these fields. Only a CurlHandle
 * object can be projected to php_curl (the class is final). */
extern "C" php_curl *swoole_curl_object_handle(zend_object *object) {
    return object->ce == swoole_coroutine_curl_handle_ce ? curl_from_obj(object) : nullptr;
}

/* Callable caches replaced callback zvals in PHP 8.4. Keep their layout
 * here with the stream records until the callback port. */
#if PHP_VERSION_ID >= 80400
static void trace_callback(zend_get_gc_buffer *gc, zend_fcall_info_cache &callback) {
    if (ZEND_FCC_INITIALIZED(callback)) {
        zend_get_gc_buffer_add_fcc(gc, &callback);
    }
}
#else
template<class T> static void trace_callback(zend_get_gc_buffer *gc, T *callback) {
    if (callback) {
        zend_get_gc_buffer_add_zval(gc, &callback->func_name);
    }
}
#endif

template<class T> static void trace_stream(zend_get_gc_buffer *gc, T *stream) {
    if (stream) {
#if PHP_VERSION_ID >= 80400
        trace_callback(gc, stream->fcc);
#else
        trace_callback(gc, stream);
#endif
        zend_get_gc_buffer_add_zval(gc, &stream->stream);
    }
}

extern "C" void swoole_curl_trace_callbacks(zend_get_gc_buffer *gc, php_curl *ch) {
    auto &h = ch->handlers;
    trace_stream(gc, h.read);
    trace_stream(gc, h.write);
    trace_stream(gc, h.write_header);
    trace_callback(gc, h.progress);
#if PHP_VERSION_ID >= 80200
    trace_callback(gc, h.xferinfo);
#endif
    trace_callback(gc, h.fnmatch);
#if PHP_VERSION_ID >= 80400
    trace_callback(gc, h.debug);
#if LIBCURL_VERSION_NUM >= 0x075000
    trace_callback(gc, h.prereq);
#endif
#endif
#if PHP_VERSION_ID >= 80300 && LIBCURL_VERSION_NUM >= 0x075400
    trace_callback(gc, h.sshhostkey);
#endif
}

extern "C" void swoole_curl_trace_values(zend_get_gc_buffer *gc, php_curl *ch) {
    zend_get_gc_buffer_add_zval(gc, &ch->postfields);
    zend_get_gc_buffer_add_zval(gc, &ch->handlers.std_err);
    zend_get_gc_buffer_add_zval(gc, &ch->private_data);
}

extern "C" void swoole_curl_publish_roots(zend_get_gc_buffer *gc, zval **table, int *n) {
    zend_get_gc_buffer_use(gc, table, n);
}

extern "C" bool swoole_curl_release_clone(php_curl *ch) {
    return --*ch->clone == 0;
}

extern "C" void swoole_curl_free_lists(php_curl *ch) {
    zend_llist_clean(&ch->to_free->post);
    zend_llist_clean(&ch->to_free->stream);
#if PHP_VERSION_ID >= 80500
    zend_hash_destroy(&ch->to_free->slist);
#else
    zend_hash_destroy(ch->to_free->slist);
    efree(ch->to_free->slist);
#endif
    efree(ch->to_free);
    efree(ch->clone);
}

extern "C" void swoole_curl_release_values(php_curl *ch) {
    zval_ptr_dtor(&ch->postfields);
    zval_ptr_dtor(&ch->private_data);
    if (ch->share) {
        OBJ_RELEASE(&ch->share->std);
    }
}

extern "C" void swoole_curl_bind_easy(php_curl *ch, CURL *easy) {
    ch->cp = easy;
    ch->handlers.write->method = PHP_CURL_STDOUT;
    ch->handlers.read->method = PHP_CURL_DIRECT;
    ch->handlers.write_header->method = PHP_CURL_IGNORE;
}

extern "C" CURL *swoole_curl_easy(php_curl *ch) {
    return ch->cp;
}

extern "C" int swoole_curl_error_number(php_curl *ch) {
    return ch->err.no;
}

extern "C" const char *swoole_curl_error_buffer(php_curl *ch) {
    ch->err.str[CURL_ERROR_SIZE] = 0;
    return ch->err.str;
}

extern "C" bool swoole_curl_in_callback(php_curl *ch) {
    return ch->in_callback;
}

extern "C" void swoole_curl_take_private(php_curl *ch, zval *previous) {
    ZVAL_COPY_VALUE(previous, &ch->private_data);
    ZVAL_UNDEF(&ch->private_data);
}

extern "C" int swoole_curl_easy_upkeep(php_curl *ch) {
#if LIBCURL_VERSION_NUM >= 0x073e00
    return ch->err.no = curl_easy_upkeep(ch->cp);
#else
    /* The PHP function is registered only with libcurl 7.62 or newer. */
    return CURLE_UNKNOWN_OPTION;
#endif
}

/* libcurl returns a C string for escape and a counted binary buffer for unescape. */
extern "C" bool swoole_curl_url_bytes(php_curl *ch, const zend_string *input, bool decode, iovec *result) {
    int length = static_cast<int>(ZSTR_LEN(input));
    int decoded = 0;
    char *bytes;
    if (decode) {
        bytes = curl_easy_unescape(ch->cp, ZSTR_VAL(input), length, &decoded);
    } else {
        bytes = curl_easy_escape(ch->cp, ZSTR_VAL(input), length);
    }
    if (!bytes) {
        return false;
    }
    *result = {bytes, decode ? static_cast<size_t>(decoded) : strlen(bytes)};
    return true;
}

extern "C" void swoole_curl_free_bytes(iovec buffer) {
    curl_free(buffer.iov_base);
}
#endif
