/*
   +----------------------------------------------------------------------+
   | Copyright (c) The PHP Group                                          |
   +----------------------------------------------------------------------+
   | This source file is subject to version 3.01 of the PHP license,      |
   | that is bundled with this package in the file LICENSE, and is        |
   | available through the world-wide-web at the following url:           |
   | https://www.php.net/license/3_01.txt                                 |
   | If you did not receive a copy of the PHP license and are unable to   |
   | obtain it through the world-wide-web, please send a note to          |
   | license@php.net so we can mail you a copy immediately.               |
   +----------------------------------------------------------------------+
   | Author: Sterling Hughes <sterling@php.net>                           |
   +----------------------------------------------------------------------+
*/

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

/* curl_setopt's options of a number, a string or a blob (crates/curl/src/setopt.rs). Each sets the
 * option as the bundled copies did and saves the error, SAVE_CURL_ERROR: curl_easy_setopt is
 * variadic. The header's libcurl version gates the options the copies compiled in. */
extern "C" const unsigned swoole_curl_header_version = LIBCURL_VERSION_NUM;

extern "C" int swoole_curl_setopt_long(php_curl *ch, int option, long value) {
    return ch->err.no = curl_easy_setopt(ch->cp, (CURLoption) option, value);
}

extern "C" int swoole_curl_setopt_off_t(php_curl *ch, int option, curl_off_t value) {
    return ch->err.no = curl_easy_setopt(ch->cp, (CURLoption) option, value);
}

/* libcurl copies the string; null unsets the option. */
extern "C" int swoole_curl_setopt_string(php_curl *ch, int option, const zend_string *value) {
    return ch->err.no = curl_easy_setopt(ch->cp, (CURLoption) option, value ? ZSTR_VAL(value) : NULL);
}

extern "C" int swoole_curl_setopt_blob(php_curl *ch, int option, const zend_string *value) {
#if LIBCURL_VERSION_NUM >= 0x074700 /* Available since 7.71.0 */
    struct curl_blob blob;
    blob.data = (void *) ZSTR_VAL(value);
    blob.len = ZSTR_LEN(value);
    blob.flags = CURL_BLOB_COPY;
    return ch->err.no = curl_easy_setopt(ch->cp, (CURLoption) option, &blob);
#else
    return ch->err.no = CURLE_UNKNOWN_OPTION;
#endif
}

extern "C" void swoole_curl_save_error(php_curl *ch, int error) {
    ch->err.no = error;
}

/* curl_slist_append: false leaves the list as it was. */
extern "C" bool swoole_curl_list_append(struct curl_slist **list, const zend_string *value) {
    struct curl_slist *appended = curl_slist_append(*list, ZSTR_VAL(value));
    if (!appended) {
        return false;
    }
    *list = appended;
    return true;
}

extern "C" void swoole_curl_list_free(struct curl_slist *list) {
    curl_slist_free_all(list);
}

/* Sets a list option and keeps the list until the handle's clone family frees it. A handle with
 * no clone replaces (and frees) its previous list for the option; otherwise the old one is kept, as
 * a clone may still use it. */
extern "C" int swoole_curl_setopt_list(php_curl *ch, int option, struct curl_slist *list) {
    if (list) {
#if PHP_VERSION_ID >= 80500
        HashTable *lists = &ch->to_free->slist;
#else
        HashTable *lists = ch->to_free->slist;
#endif
        if (*ch->clone == 1) {
            zend_hash_index_update_ptr(lists, option, list);
        } else {
            zend_hash_next_index_insert_ptr(lists, list);
        }
    }
    return ch->err.no = curl_easy_setopt(ch->cp, (CURLoption) option, list);
}

extern "C" void swoole_curl_set_write_method(php_curl *ch, int method) {
    ch->handlers.write->method = method;
}

extern "C" zval *swoole_curl_private(php_curl *ch) {
    return &ch->private_data;
}

/* The stream resource's FILE * and whether it was opened for writing, or null after the error. */
extern "C" FILE *swoole_curl_stream_file(zval *value, bool *writable) {
    php_stream *what =
        (php_stream *) zend_fetch_resource2_ex(value, "File-Handle", php_file_le_stream(), php_file_le_pstream());
    FILE *fp = NULL;
    if (!what || php_stream_cast(what, PHP_STREAM_AS_STDIO, (void **) &fp, REPORT_ERRORS) == FAILURE) {
        return NULL;
    }
    *writable = what->mode[0] != 'r' || what->mode[1] == '+';
    return fp;
}

/* The stream a file-handle option keeps. */
extern "C" zval *swoole_curl_file_stream(php_curl *ch, int option) {
    switch (option) {
    case CURLOPT_FILE:
        return &ch->handlers.write->stream;
    case CURLOPT_WRITEHEADER:
        return &ch->handlers.write_header->stream;
    case CURLOPT_INFILE:
        return &ch->handlers.read->stream;
    default:
        return &ch->handlers.std_err;
    }
}

/* Points the option's handler at fp, or back at its default when fp is null. Only CURLOPT_STDERR
 * is a libcurl option; the others are read by the write, header and read callbacks. */
extern "C" int swoole_curl_set_file(php_curl *ch, int option, FILE *fp) {
    switch (option) {
    case CURLOPT_FILE:
        ch->handlers.write->fp = fp;
        ch->handlers.write->method = fp ? PHP_CURL_FILE : PHP_CURL_STDOUT;
        return ch->err.no = CURLE_OK;
    case CURLOPT_WRITEHEADER:
        ch->handlers.write_header->fp = fp;
        ch->handlers.write_header->method = fp ? PHP_CURL_FILE : PHP_CURL_IGNORE;
        return ch->err.no = CURLE_OK;
    case CURLOPT_INFILE:
        ch->handlers.read->fp = fp;
        ch->handlers.read->res = fp ? Z_RES(ch->handlers.read->stream) : NULL;
        return ch->err.no = CURLE_OK;
    default:
        return ch->err.no = curl_easy_setopt(ch->cp, CURLOPT_STDERR, fp);
    }
}

/* CURLOPT_SHARE: sets and keeps a CurlShareHandle (or, on 8.5, a persistent one); any other
 * value is ignored. */
extern "C" void swoole_curl_set_share(php_curl *ch, zval *value) {
    if (Z_TYPE_P(value) != IS_OBJECT) {
        return;
    }
#if PHP_VERSION_ID >= 80500
    if (Z_OBJCE_P(value) != curl_share_ce && Z_OBJCE_P(value) != curl_share_persistent_ce) {
        return;
    }
#else
    if (Z_OBJCE_P(value) != curl_share_ce) {
        return;
    }
#endif
    php_curlsh *sh = Z_CURL_SHARE_P(value);
    curl_easy_setopt(ch->cp, CURLOPT_SHARE, sh->share);
    if (ch->share) {
        OBJ_RELEASE(&ch->share->std);
    }
    GC_ADDREF(&sh->std);
    ch->share = sh;
}

/* PG(open_basedir) is set and not empty. */
extern "C" bool swoole_curl_open_basedir() {
    return PG(open_basedir) && *PG(open_basedir);
}

/* php_check_open_basedir, which warns about a path outside it. */
extern "C" bool swoole_curl_allowed_path(const zend_string *path) {
    return php_check_open_basedir(ZSTR_VAL(path)) == 0;
}
#endif
