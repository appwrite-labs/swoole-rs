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

extern "C" {
zend_class_entry *swoole_coroutine_curl_multi_handle_ce;
static zend_object_handlers swoole_coroutine_curl_multi_handle_handlers;
extern const size_t swoole_curl_multi_size = sizeof(php_curlm);
extern const size_t swoole_curl_multi_offset = XtOffsetOf(php_curlm, std);
zend_object *swoole_curl_multi_create_object(zend_class_entry *ce);
zend_function *swoole_curl_multi_get_constructor(zend_object *object);
void swoole_curl_multi_free_obj(zend_object *object);
HashTable *swoole_curl_multi_get_gc(zend_object *object, zval **table, int *n);
}

extern "C" const zend_object_handlers *swoole_curl_multi_handlers(zend_class_entry *ce) {
#if PHP_VERSION_ID >= 80300 && PHP_VERSION_ID < 80400
    return ce->default_object_handlers;
#else
    return &swoole_coroutine_curl_multi_handle_handlers;
#endif
}

void curl_multi_register_class(void) {
    swoole_coroutine_curl_multi_handle_ce = curl_multi_ce;
    swoole_coroutine_curl_multi_handle_ce->create_object = swoole_curl_multi_create_object;

    memcpy(&swoole_coroutine_curl_multi_handle_handlers, &std_object_handlers, sizeof(zend_object_handlers));
    swoole_coroutine_curl_multi_handle_handlers.offset = XtOffsetOf(php_curlm, std);
    swoole_coroutine_curl_multi_handle_handlers.free_obj = swoole_curl_multi_free_obj;
    swoole_coroutine_curl_multi_handle_handlers.get_gc = swoole_curl_multi_get_gc;
    swoole_coroutine_curl_multi_handle_handlers.get_constructor = swoole_curl_multi_get_constructor;
    swoole_coroutine_curl_multi_handle_handlers.clone_obj = NULL;
    swoole_coroutine_curl_multi_handle_handlers.cast_object = swoole_curl_cast_object;
    swoole_coroutine_curl_multi_handle_handlers.compare = [](zval *o1, zval *o2) { return ZEND_UNCOMPARABLE; };

    zend_declare_property_bool(swoole_coroutine_curl_multi_handle_ce, ZEND_STRL("in_coroutine"), 0, ZEND_ACC_PUBLIC);
}

// Compatibility with libcurl builds that still require OpenSSL's pre-1.1 ZTS locks.
#if PHP_VERSION_ID < 80500 && defined(ZTS) && defined(HAVE_CURL_OLD_OPENSSL)
#ifdef HAVE_OPENSSL_CRYPTO_H
#include <openssl/crypto.h>
static MUTEX_T *curl_locks;
static ZEND_ATTRIBUTE_UNUSED void curl_ssl_lock(int mode, int index, const char *, int) {
    if (mode & CRYPTO_LOCK) {
        tsrm_mutex_lock(curl_locks[index]);
    } else {
        tsrm_mutex_unlock(curl_locks[index]);
    }
}
static ZEND_ATTRIBUTE_UNUSED unsigned long curl_ssl_thread_id() {
    return (unsigned long) tsrm_thread_id();
}
#else
#warning "libcurl uses old OpenSSL but openssl/crypto.h is unavailable for ZTS locking"
#endif
#endif

extern "C" bool swoole_curl_startup(const zend_function_entry *entries) {
#if PHP_VERSION_ID < 80500 && defined(ZTS) && defined(HAVE_CURL_OLD_OPENSSL) && defined(HAVE_OPENSSL_CRYPTO_H)
    if (!CRYPTO_get_id_callback()) {
        int count = CRYPTO_num_locks();
        curl_locks = static_cast<MUTEX_T *>(malloc(count * sizeof(*curl_locks)));
        if (!curl_locks) {
            return false;
        }
        for (int i = 0; i < count; ++i) {
            curl_locks[i] = tsrm_mutex_alloc();
        }
        CRYPTO_set_id_callback(curl_ssl_thread_id);
        CRYPTO_set_locking_callback(curl_ssl_lock);
    }
#endif
    swoole_curl_register_handle();
    curl_multi_register_class();
    zend_unregister_functions(entries, -1, CG(function_table));
    zend_register_functions(nullptr, entries, nullptr, MODULE_PERSISTENT);
    return true;
}

extern "C" php_curlm *swoole_curl_multi_object(zend_object *object) {
    if (object->ce != swoole_coroutine_curl_multi_handle_ce) {
        return nullptr;
    }
    return reinterpret_cast<php_curlm *>(reinterpret_cast<char *>(object) - XtOffsetOf(php_curlm, std));
}

static void release_multi_easy(void *value) {
    zval_ptr_dtor(static_cast<zval *>(value));
}

extern "C" void swoole_curl_multi_attach(php_curlm *mh, swr_v1_curl_multi *engine) {
    mh->multi = engine;
    zend_llist_init(&mh->easyh, sizeof(zval), release_multi_easy, 0);
    zend_update_property_bool(nullptr, &mh->std, ZEND_STRL("in_coroutine"), true);
}

extern "C" bool swoole_curl_multi_is_in_coroutine(php_curlm *mh) {
    zval rv;
    zval *zv = zend_read_property_ex(nullptr, &mh->std, SW_ZSTR_KNOWN(SW_ZEND_STR_IN_COROUTINE), 1, &rv);
    return zval_is_true(zv);
}

extern "C" swr_v1_curl_multi *swoole_curl_multi_engine(php_curlm *mh) {
    // Only our attach routine pairs this list destructor with a Rust engine.
    return mh->easyh.dtor == release_multi_easy ? mh->multi : nullptr;
}

extern "C" zval *swoole_curl_multi_find_easy(php_curlm *mh, CURL *easy) {
    for (zend_llist_element *item = mh->easyh.head; item; item = item->next) {
        auto *value = reinterpret_cast<zval *>(item->data);
        if (Z_TYPE_P(value) == IS_OBJECT && Z_CURL_P(value)->cp == easy) {
            return value;
        }
    }
    return nullptr;
}

extern "C" void swoole_curl_multi_keep(php_curlm *mh, php_curl *ch) {
    zval value;
    ZVAL_OBJ_COPY(&value, &ch->std);
    zend_llist_add_element(&mh->easyh, &value);
}

extern "C" void swoole_curl_multi_membership(php_curlm *mh, php_curl *ch, int error, bool add) {
    swoole_trace_log(SW_TRACE_CO_CURL, "multi=%p, cp=%p, handle=%p, error=%d",
                     mh->multi, ch->cp, swr_v1_curl_get_handle(ch->cp), error);
    mh->err.no = error;
#if PHP_VERSION_ID >= 80200
    if (error != CURLM_OK) {
        return;
    }
#endif
    if (add) {
        swoole_curl_multi_keep(mh, ch);
    } else {
        zval needle;
        ZVAL_OBJ(&needle, &ch->std);
        zend_llist_del_element(&mh->easyh, &needle, [](void *left, void *right) -> int {
            return Z_OBJ_P(static_cast<zval *>(left)) == Z_OBJ_P(static_cast<zval *>(right));
        });
    }
}

extern "C" void swoole_curl_multi_take_easy(php_curlm *mh, zval *handles, bool destroying) {
    if (!mh->easyh.head) {
        return;
    }
    array_init_size(handles, zend_llist_count(&mh->easyh));
    for (auto *item = mh->easyh.head; item; item = item->next) {
        auto *value = reinterpret_cast<zval *>(item->data);
        if (destroying && (OBJ_FLAGS(Z_OBJ_P(value)) & IS_OBJ_FREE_CALLED)) {
            continue;
        }
        Z_TRY_ADDREF_P(value);
        add_next_index_zval(handles, value);
    }
    zend_llist_clean(&mh->easyh);
}

extern "C" void swoole_curl_multi_detach_native(php_curlm *mh, php_curl *ch) {
    if (mh->easyh.dtor != release_multi_easy && mh->multi) {
        curl_multi_remove_handle(reinterpret_cast<CURLM *>(mh->multi), ch->cp);
    }
}

extern "C" void swoole_curl_multi_release_engine(php_curlm *mh) {
    auto *engine = mh->multi;
    mh->multi = nullptr;
    if (mh->easyh.dtor == release_multi_easy) {
        swr_v1_curl_multi_free(engine);
    } else if (engine) {
        curl_multi_cleanup(reinterpret_cast<CURLM *>(engine));
    }
}

extern "C" void swoole_curl_multi_release_push(php_curlm *mh) {
    auto &push = mh->handlers.server_push;
#if PHP_VERSION_ID >= 80400
    if (ZEND_FCC_INITIALIZED(push)) {
        zend_fcc_dtor(&push);
    }
#else
    if (push) {
        zval_ptr_dtor(&push->func_name);
        efree(push);
    }
#endif
}

extern "C" int swoole_curl_multi_set_number(CURLM *multi, int option, zend_long value) {
    auto id = static_cast<CURLMoption>(option);
    if (id == CURLMOPT_CONTENT_LENGTH_PENALTY_SIZE || id == CURLMOPT_CHUNK_LENGTH_PENALTY_SIZE) {
        return curl_multi_setopt(multi, id, static_cast<curl_off_t>(value));
    }
    return curl_multi_setopt(multi, id, static_cast<long>(value));
}

extern "C" int swoole_curl_multi_on_push(CURL *, CURL *, size_t, struct curl_pushheaders *, void *);

extern "C" bool swoole_curl_multi_set_push(php_curlm *mh, CURLM *multi, zval *value) {
    auto &push = mh->handlers.server_push;
#if PHP_VERSION_ID >= 80400
    if (ZEND_FCC_INITIALIZED(push)) {
        zend_fcc_dtor(&push);
    }
    char *reason = nullptr;
    if (!zend_is_callable_ex(value, nullptr, 0, nullptr, &push, &reason)) {
        if (!EG(exception)) {
            zend_argument_type_error(2, "must be a valid callback for option CURLMOPT_PUSHFUNCTION, %s", reason);
        }
        efree(reason);
        return false;
    }
    zend_fcc_addref(&push);
#else
    if (!push) {
        push = static_cast<php_curl_callback *>(ecalloc(1, sizeof(*push)));
    } else if (!Z_ISUNDEF(push->func_name)) {
        zval_ptr_dtor(&push->func_name);
        push->fci_cache = empty_fcall_info_cache;
    }
    ZVAL_COPY(&push->func_name, value);
#endif
    if (curl_multi_setopt(multi, CURLMOPT_PUSHFUNCTION, swoole_curl_multi_on_push) != CURLM_OK) {
        return false;
    }
    mh->err.no = curl_multi_setopt(multi, CURLMOPT_PUSHDATA, mh);
    return mh->err.no == CURLM_OK;
}

extern "C" bool swoole_curl_multi_empty(php_curlm *mh) {
    return zend_llist_count(&mh->easyh) == 0;
}

extern "C" void swoole_curl_multi_verify(php_curlm *mh) {
    zend_llist_position pos;
    for (auto *item = zend_llist_get_first_ex(&mh->easyh, &pos); item; item = zend_llist_get_next_ex(&mh->easyh, &pos)) {
        swoole_curl_verify_handlers(Z_CURL_P(static_cast<zval *>(item)), true);
    }
}

extern "C" void swoole_curl_assign_long(zval *slot, zend_long number) {
    ZEND_TRY_ASSIGN_LONG(slot, number);
}

extern "C" void swoole_curl_multi_set_error(php_curlm *mh, int code) {
    mh->err.no = code;
}

extern "C" int swoole_curl_multi_error(php_curlm *mh) {
    return mh->err.no;
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

extern "C" void swoole_curl_multi_trace(zend_get_gc_buffer *gc, php_curlm *mh) {
    trace_callback(gc, mh->handlers.server_push);
    for (zend_llist_element *item = mh->easyh.head; item; item = item->next) {
        zend_get_gc_buffer_add_zval(gc, reinterpret_cast<zval *>(item->data));
    }
}

extern "C" void swoole_curl_publish_roots(zend_get_gc_buffer *gc, zval **table, int *n) {
    zend_get_gc_buffer_use(gc, table, n);
}

extern "C" php_curl *swoole_curl_new_handle(zval *result) {
    object_init_ex(result, swoole_coroutine_curl_handle_ce);
    return Z_CURL_P(result);
}

/* zend_object_alloc zeroes all php_curl fields before std, including the
 * callback slots, error buffer and undefined postfields zval. */
extern "C" void swoole_curl_alloc_handlers(php_curl *ch) {
    ch->handlers.write = (php_curl_write *) ecalloc(1, sizeof(php_curl_write));
    ch->handlers.write_header = (php_curl_write *) ecalloc(1, sizeof(php_curl_write));
    ch->handlers.read = (php_curl_read *) ecalloc(1, sizeof(php_curl_read));
}

/* A CURLFile upload's state for libcurl's read, seek and free callbacks: the file opens on the
 * first read when it couldn't be opened up front. */
struct mime_data_cb_arg {
    zend_string *filename;
    php_stream *stream;
};

static void free_upload(void *slot) {
    auto *cb_arg = static_cast<mime_data_cb_arg *>(*static_cast<void **>(slot));

    ZEND_ASSERT(cb_arg->stream == NULL);
    zend_string_release(cb_arg->filename);
    efree(cb_arg);
}

extern "C" void swoole_curl_alloc_lists(php_curl *ch) {
    ch->to_free = (_php_curl_free *) ecalloc(1, sizeof(_php_curl_free));
    zend_llist_init(&ch->to_free->post, sizeof(curl_mime *), [](void *slot) {
        curl_mime_free(*static_cast<curl_mime **>(slot));
    }, 0);
    zend_llist_init(&ch->to_free->stream, sizeof(void *), free_upload, 0);
#if PHP_VERSION_ID >= 80500
    HashTable *lists = &ch->to_free->slist;
#else
    HashTable *lists = ch->to_free->slist = (HashTable *) emalloc(sizeof(HashTable));
#endif
    zend_hash_init(lists, 4, nullptr, [](zval *entry) {
        curl_slist_free_all(static_cast<curl_slist *>(Z_PTR_P(entry)));
    }, 0);
    ch->clone = (uint32_t *) emalloc(sizeof(uint32_t));
    *ch->clone = 1;
}

extern "C" void swoole_curl_share_lists(php_curl *ch, php_curl *source) {
    ch->to_free = source->to_free;
    ch->clone = source->clone;
    ++*ch->clone;
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

extern "C" void swoole_curl_bind_io(php_curl *ch) {
    curl_easy_setopt(ch->cp, CURLOPT_ERRORBUFFER, ch->err.str);
    curl_easy_setopt(ch->cp, CURLOPT_WRITEFUNCTION, swoole_curl_on_write);
    curl_easy_setopt(ch->cp, CURLOPT_WRITEDATA, ch);
    curl_easy_setopt(ch->cp, CURLOPT_READFUNCTION, swoole_curl_on_read);
    curl_easy_setopt(ch->cp, CURLOPT_READDATA, ch);
    curl_easy_setopt(ch->cp, CURLOPT_HEADERFUNCTION, swoole_curl_on_header);
    curl_easy_setopt(ch->cp, CURLOPT_HEADERDATA, ch);
}

template <typename Stream>
static void copy_stream(Stream *copy, const Stream *source) {
    ZVAL_COPY(&copy->stream, &source->stream);
    copy->method = source->method;
    copy->fp = source->fp;
}

extern "C" void swoole_curl_copy_streams(php_curl *ch, php_curl *source) {
    copy_stream(ch->handlers.write, source->handlers.write);
    copy_stream(ch->handlers.read, source->handlers.read);
    copy_stream(ch->handlers.write_header, source->handlers.write_header);
    ch->handlers.read->res = source->handlers.read->res;
    curl_easy_setopt(ch->cp, CURLOPT_ERRORBUFFER, ch->err.str);
    curl_easy_setopt(ch->cp, CURLOPT_WRITEDATA, ch);
    curl_easy_setopt(ch->cp, CURLOPT_READDATA, ch);
    curl_easy_setopt(ch->cp, CURLOPT_HEADERDATA, ch);
    curl_easy_setopt(ch->cp, CURLOPT_DEBUGDATA, ch);
}

extern "C" bool swoole_curl_use_cafile(php_curl *ch, const char *directive) {
    const char *path = INI_STR(directive);
    if (!path || !*path) {
        return false;
    }
    curl_easy_setopt(ch->cp, CURLOPT_CAINFO, path);
    return true;
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

extern "C" void swoole_curl_set_in_callback(php_curl *ch, bool running) {
    ch->in_callback = running;
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
extern "C" const long swoole_curl_protocols_without_file = (long) (CURLPROTO_ALL & ~CURLPROTO_FILE);
extern "C" const unsigned swoole_curl_header_version = LIBCURL_VERSION_NUM;

extern "C" int swoole_curl_easy_setopt_long(CURL *easy, zend_long option, long value) {
    return curl_easy_setopt(easy, (CURLoption) option, value);
}

extern "C" int swoole_curl_setopt_long(php_curl *ch, int option, long value) {
    return ch->err.no = swoole_curl_easy_setopt_long(ch->cp, option, value);
}

extern "C" int swoole_curl_setopt_off_t(php_curl *ch, int option, curl_off_t value) {
    return ch->err.no = curl_easy_setopt(ch->cp, (CURLoption) option, value);
}

/* libcurl copies the string; null unsets the option. */
extern "C" int swoole_curl_setopt_string(php_curl *ch, int option, const zend_string *value) {
#if defined(PHP_WIN32) && PHP_VERSION_ID >= 80400
    constexpr size_t prefix_length = sizeof("file://") - 1;
    if (option == CURLOPT_URL && value && zend_string_starts_with_literal_ci(value, "file://") &&
        ZSTR_VAL(value)[prefix_length] != '/' && ZSTR_LEN(value) < MAXPATHLEN - 2) {
        char path[MAXPATHLEN] = "file:///";
        memcpy(path + prefix_length + 1, ZSTR_VAL(value) + prefix_length, ZSTR_LEN(value) - prefix_length);
        return ch->err.no = curl_easy_setopt(ch->cp, CURLOPT_URL, path);
    }
#endif
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

/* curl_easy_getinfo, which is variadic, for each result type: whether libcurl answered. */
extern "C" bool swoole_curl_info_string(php_curl *ch, int info, const char **value) {
    return curl_easy_getinfo(ch->cp, (CURLINFO) info, value) == CURLE_OK;
}

extern "C" bool swoole_curl_info_long(php_curl *ch, int info, int64_t *value) {
    long answer = 0;
    if (curl_easy_getinfo(ch->cp, (CURLINFO) info, &answer) != CURLE_OK) {
        return false;
    }
    *value = answer;
    return true;
}

extern "C" bool swoole_curl_info_double(php_curl *ch, int info, double *value) {
    return curl_easy_getinfo(ch->cp, (CURLINFO) info, value) == CURLE_OK;
}

extern "C" bool swoole_curl_info_off_t(php_curl *ch, int info, curl_off_t *value) {
    return curl_easy_getinfo(ch->cp, (CURLINFO) info, value) == CURLE_OK;
}

/* A list the caller frees with swoole_curl_list_free. */
extern "C" bool swoole_curl_info_list(php_curl *ch, int info, struct curl_slist **list) {
    return curl_easy_getinfo(ch->cp, (CURLINFO) info, list) == CURLE_OK;
}

/* The next string of a list libcurl owns, or null at its end. */
extern "C" const char *swoole_curl_list_next(const struct curl_slist **cursor) {
    const struct curl_slist *node = *cursor;
    if (!node) {
        return NULL;
    }
    *cursor = node->next;
    return node->data;
}

/* CURLINFO_CERTINFO: the number of certificates, or -1 when libcurl did not answer. */
extern "C" int swoole_curl_info_certs(php_curl *ch, const struct curl_certinfo **certs) {
    struct curl_certinfo *info = NULL;
    if (curl_easy_getinfo(ch->cp, CURLINFO_CERTINFO, &info) != CURLE_OK) {
        return -1;
    }
    *certs = info;
    return info ? info->num_of_certs : 0;
}

/* The fields of certificate `index`, which swoole_curl_info_certs counted. */
extern "C" const struct curl_slist *swoole_curl_cert_fields(const struct curl_certinfo *certs, int index) {
    return certs->certinfo[index];
}

/* The request header CURLINFO_HEADER_OUT recorded, or null. */
extern "C" const zend_string *swoole_curl_header_out(php_curl *ch) {
    return ch->header.str;
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

/* What the libcurl callbacks in crates/curl/src/callbacks.rs read and write of php_curl, whose
 * layout is ext/curl's own. */

/* Whether the value is a stream resource that has been closed under the handle. */
extern "C" bool swoole_curl_stream_gone(zval *value) {
    return !Z_ISUNDEF_P(value) && !zend_fetch_resource2_ex(value, NULL, php_file_le_stream(), php_file_le_pstream());
}

/* Back to the default once the option's stream is gone. */
extern "C" void swoole_curl_stream_reset(php_curl *ch, int option) {
    switch (option) {
    case CURLOPT_FILE:
        ch->handlers.write->fp = NULL;
        ch->handlers.write->method = PHP_CURL_STDOUT;
        break;
    case CURLOPT_WRITEHEADER:
        ch->handlers.write_header->fp = NULL;
        ch->handlers.write_header->method = PHP_CURL_IGNORE;
        break;
    case CURLOPT_INFILE:
        ch->handlers.read->res = NULL;
        ch->handlers.read->fp = NULL;
        break;
    default:
        curl_easy_setopt(ch->cp, CURLOPT_STDERR, stderr);
    }
}

/* The write, header or read slot of a callable option. */
extern "C" int swoole_curl_method(php_curl *ch, int option) {
    switch (option) {
    case CURLOPT_WRITEFUNCTION:
        return ch->handlers.write->method;
    case CURLOPT_HEADERFUNCTION:
        return ch->handlers.write_header->method;
    default:
        return ch->handlers.read->method;
    }
}

extern "C" void swoole_curl_set_method(php_curl *ch, int option, int method) {
    switch (option) {
    case CURLOPT_WRITEFUNCTION:
        ch->handlers.write->method = method;
        break;
    case CURLOPT_HEADERFUNCTION:
        ch->handlers.write_header->method = method;
        break;
    default:
        ch->handlers.read->method = method;
    }
}

extern "C" FILE *swoole_curl_file(php_curl *ch, int option) {
    switch (option) {
    case CURLOPT_WRITEFUNCTION:
        return ch->handlers.write->fp;
    case CURLOPT_HEADERFUNCTION:
        return ch->handlers.write_header->fp;
    default:
        return ch->handlers.read->fp;
    }
}

/* CURLOPT_INFILE's stream, as the read callable's second argument, or null. */
extern "C" void swoole_curl_read_stream(php_curl *ch, zval *value) {
    zend_resource *res = ch->handlers.read->res;
    if (res) {
        GC_ADDREF(res);
        ZVAL_RES(value, res);
    } else {
        ZVAL_NULL(value);
    }
}

extern "C" void swoole_curl_buffer_append(php_curl *ch, const char *data, size_t length) {
    smart_str_appendl(&ch->handlers.write->buf, data, (int) length);
}

/* curl_exec's result: the collected response, which the handle keeps until its next transfer. */
extern "C" bool swoole_curl_buffer_result(php_curl *ch, zval *result) {
    smart_str *buf = &ch->handlers.write->buf;
    if (!buf->s) {
        return false;
    }
    smart_str_0(buf);
    ZVAL_STR_COPY(result, buf->s);
    return true;
}

extern "C" void swoole_curl_buffer_free(php_curl *ch) {
    smart_str_free(&ch->handlers.write->buf);
}

/* _php_curl_cleanup_handle: the last transfer's response, request header and error. */
extern "C" void swoole_curl_cleanup_handle(php_curl *ch) {
    smart_str_free(&ch->handlers.write->buf);
    if (ch->header.str) {
        zend_string_release_ex(ch->header.str, 0);
        ch->header.str = NULL;
    }
    memset(ch->err.str, 0, CURL_ERROR_SIZE + 1);
    ch->err.no = 0;
}

extern "C" void swoole_curl_stream_flush(zval *value) {
    php_stream *stream = Z_ISUNDEF_P(value) ? NULL
        : (php_stream *) zend_fetch_resource2_ex(value, NULL, php_file_le_stream(), php_file_le_pstream());
    if (stream) {
        php_stream_flush(stream);
    }
}

/* The request header, for CURLINFO_HEADER_OUT. */
extern "C" void swoole_curl_record_header(php_curl *ch, const char *data, size_t length) {
    if (ch->header.str) {
        zend_string_release_ex(ch->header.str, 0);
    }
    ch->header.str = zend_string_init(data, length, 0);
}

/* The CurlHandle object, as a callable's first argument. */
extern "C" void swoole_curl_handle_value(php_curl *ch, zval *value) {
    ZVAL_OBJ_COPY(value, &ch->std);
}

extern "C" void swoole_curl_print(const char *data, size_t length) {
    PHPWRITE(data, length);
}

extern "C" size_t swoole_curl_fwrite(const char *data, size_t size, size_t nmemb, FILE *fp) {
    return fwrite(data, size, nmemb, fp);
}

extern "C" size_t swoole_curl_fread(char *data, size_t size, size_t nmemb, FILE *fp) {
    return fread(data, size, nmemb, fp);
}

extern "C" void swoole_curl_data_string(zval *value, const char *data, size_t length) {
    ZVAL_STRINGL(value, data, length);
}

/* The first length bytes of a read callable's string, into libcurl's buffer. */
extern "C" void swoole_curl_data_fill(char *data, const zend_string *from, size_t length) {
    memcpy(data, ZSTR_VAL(from), length);
}

/* A callable option's storage. From PHP 8.4 it is a cache resolved when the option is set,
 * which holds what it calls; before, the value as given beside the cache its first call
 * fills, in a record of its own for the options libcurl has a callback for. */
#if PHP_VERSION_ID >= 80400
static zend_fcall_info_cache *callable_slot(php_curl *ch, int option) {
    switch (option) {
    case CURLOPT_WRITEFUNCTION:
        return &ch->handlers.write->fcc;
    case CURLOPT_HEADERFUNCTION:
        return &ch->handlers.write_header->fcc;
    case CURLOPT_READFUNCTION:
        return &ch->handlers.read->fcc;
    case CURLOPT_PROGRESSFUNCTION:
        return &ch->handlers.progress;
    case CURLOPT_XFERINFOFUNCTION:
        return &ch->handlers.xferinfo;
    case CURLOPT_FNMATCH_FUNCTION:
        return &ch->handlers.fnmatch;
    case CURLOPT_DEBUGFUNCTION:
        return &ch->handlers.debug;
#if LIBCURL_VERSION_NUM >= 0x075000 /* Available since 7.80.0 */
    case CURLOPT_PREREQFUNCTION:
        return &ch->handlers.prereq;
#endif
#if LIBCURL_VERSION_NUM >= 0x075400 /* Available since 7.84.0 */
    case CURLOPT_SSH_HOSTKEYFUNCTION:
        return &ch->handlers.sshhostkey;
#endif
    default:
        return NULL;
    }
}
#else
static php_curl_callback **plain_callback_slot(php_curl *ch, int option) {
    switch (option) {
    case CURLOPT_PROGRESSFUNCTION:
        return &ch->handlers.progress;
    case CURLOPT_XFERINFOFUNCTION:
        return &ch->handlers.xferinfo;
    case CURLOPT_FNMATCH_FUNCTION:
        return &ch->handlers.fnmatch;
#if LIBCURL_VERSION_NUM >= 0x075400 && PHP_VERSION_ID >= 80300
    case CURLOPT_SSH_HOSTKEYFUNCTION:
        return &ch->handlers.sshhostkey;
#endif
    default:
        return nullptr;
    }
}

/* The option's value and cache: false for an option libcurl has a callback for that was
 * never set, unless create. */
static bool callable_slot(php_curl *ch, int option, bool create, zval **name, zend_fcall_info_cache **cache) {
    switch (option) {
    case CURLOPT_WRITEFUNCTION:
        *name = &ch->handlers.write->func_name;
        *cache = &ch->handlers.write->fci_cache;
        return true;
    case CURLOPT_HEADERFUNCTION:
        *name = &ch->handlers.write_header->func_name;
        *cache = &ch->handlers.write_header->fci_cache;
        return true;
    case CURLOPT_READFUNCTION:
        *name = &ch->handlers.read->func_name;
        *cache = &ch->handlers.read->fci_cache;
        return true;
    default:
        break;
    }
    auto **record = plain_callback_slot(ch, option);
    if (!record) {
        return false;
    }
    if (!*record) {
        if (!create) {
            return false;
        }
        *record = (php_curl_callback *) ecalloc(1, sizeof(php_curl_callback));
    }
    *name = &(*record)->func_name;
    *cache = &(*record)->fci_cache;
    return true;
}
#endif

/* Stream userdata is bound unconditionally above. Other callbacks bind their
 * userdata only when the source has a callback slot to copy. */
extern "C" void swoole_curl_copy_callback(php_curl *ch, php_curl *source, int option, int data_option) {
#if PHP_VERSION_ID >= 80400
    auto *from = callable_slot(source, option);
    if (!from || !ZEND_FCC_INITIALIZED(*from)) {
        return;
    }
    zend_fcc_dup(callable_slot(ch, option), from);
#else
    zval *from, *to;
    zend_fcall_info_cache *cache;
    if (!callable_slot(source, option, false, &from, &cache)) {
        return;
    }
    callable_slot(ch, option, true, &to, &cache);
    ZVAL_COPY(to, from);
#endif
    if (data_option) {
        curl_easy_setopt(ch->cp, (CURLoption) data_option, ch);
    }
}

extern "C" bool swoole_curl_has_callable(php_curl *ch, int option) {
#if PHP_VERSION_ID >= 80400
    zend_fcall_info_cache *fcc = callable_slot(ch, option);
    return fcc && ZEND_FCC_INITIALIZED(*fcc);
#else
    zval *name;
    zend_fcall_info_cache *cache;
    return callable_slot(ch, option, false, &name, &cache) && !Z_ISUNDEF_P(name);
#endif
}

/* Detach the callable before its last reference can run PHP code. */
extern "C" void swoole_curl_reset_callback(php_curl *ch, int option) {
#if PHP_VERSION_ID >= 80400
    auto *slot = callable_slot(ch, option);
    if (slot && ZEND_FCC_INITIALIZED(*slot)) {
        auto previous = *slot;
        *slot = empty_fcall_info_cache;
        zend_fcc_dtor(&previous);
    }
#else
    auto **slot = plain_callback_slot(ch, option);
    if (slot && *slot) {
        zval previous;
        ZVAL_COPY_VALUE(&previous, &(*slot)->func_name);
        efree(*slot);
        *slot = nullptr;
        zval_ptr_dtor(&previous);
    }
#endif
}

/* Releases the option's callable, then keeps value as it. From PHP 8.4 null keeps none, and
 * a value that is not callable keeps none and is false, with the reason in error. */
extern "C" bool swoole_curl_set_callable(php_curl *ch, int option, zval *value, zval *error) {
#if PHP_VERSION_ID >= 80400
    zend_fcall_info_cache *fcc = callable_slot(ch, option);
    if (!fcc) {
        return false;
    }
    if (ZEND_FCC_INITIALIZED(*fcc)) {
        zend_fcc_dtor(fcc);
    }
    if (Z_TYPE_P(value) == IS_NULL) {
        return true;
    }
    char *reason = NULL;
    if (!zend_is_callable_ex(value, NULL, 0, NULL, fcc, &reason)) {
        if (reason) {
            ZVAL_STRING(error, reason);
            efree(reason);
        }
        *fcc = empty_fcall_info_cache;
        return false;
    }
    zend_fcc_addref(fcc);
    return true;
#else
    zval *name, previous;
    zend_fcall_info_cache *cache;
    if (!callable_slot(ch, option, true, &name, &cache)) {
        return false;
    }
    ZVAL_COPY_VALUE(&previous, name);
    ZVAL_COPY(name, value);
    *cache = empty_fcall_info_cache;
    zval_ptr_dtor(&previous);
    return true;
#endif
}

/* The arguments of a call, as Rust lends them (swoole_rs_zend::Zvals). */
struct callable_args {
    zval *argv;
    uint32_t argc;
};

#if PHP_VERSION_ID >= 80400
static bool call_cache(zend_fcall_info_cache fcc, callable_args args, zval *retval) {
    zend_object *object = fcc.object, *closure = fcc.closure;
    if (object) {
        GC_ADDREF(object);
    }
    if (closure) {
        GC_ADDREF(closure);
    }
    zend_call_known_fcc(&fcc, retval, args.argc, args.argv, NULL);
    if (object) {
        OBJ_RELEASE(object);
    }
    if (closure) {
        OBJ_RELEASE(closure);
    }
    return true;
}
#endif

/* Calls the option's callable, which swoole_curl_has_callable saw, into retval (undefined
 * when it throws): false when PHP could not call it. The call holds what it calls, so the
 * PHP code it runs may set the option again. */
extern "C" bool swoole_curl_call(php_curl *ch, int option, struct callable_args args, zval *retval) {
#if PHP_VERSION_ID >= 80400
    zend_fcall_info_cache fcc = *callable_slot(ch, option);
    return call_cache(fcc, args, retval);
#else
    zval *name;
    zend_fcall_info_cache *cache;
    zend_fcall_info fci;
    callable_slot(ch, option, false, &name, &cache);
    fci.size = sizeof(fci);
    ZVAL_COPY(&fci.function_name, name);
    fci.object = NULL;
    fci.retval = retval;
    fci.param_count = args.argc;
    fci.params = args.argv;
    fci.named_params = NULL;
    bool called = zend_call_function(&fci, cache) != FAILURE;
    zval_ptr_dtor(&fci.function_name);
    return called;
#endif
}

extern "C" bool swoole_curl_call_push(php_curlm *mh, callable_args args, zval *retval) {
#if PHP_VERSION_ID >= 80400
    return call_cache(mh->handlers.server_push, args, retval);
#else
    zend_fcall_info fci;
    zend_fcall_info_cache cache;
    if (zend_fcall_info_init(&mh->handlers.server_push->func_name, 0, &fci, &cache, nullptr, nullptr) == FAILURE) {
        return false;
    }
    Z_TRY_ADDREF(fci.function_name);
    fci.params = args.argv;
    fci.param_count = args.argc;
    fci.retval = retval;
    bool called = zend_call_function(&fci, &cache) == SUCCESS;
    zval_ptr_dtor(&fci.function_name);
    return called;
#endif
}

extern "C" bool swoole_curl_has_clone_storage(php_curl *ch) {
    return ch->clone != nullptr;
}

/* libcurl's callback for a callable option it has one for, with the handle as its data; with
 * `installed` false, libcurl's own default for the option. */
extern "C" void swoole_curl_install_callback(php_curl *ch, int option, bool installed) {
    switch (option) {
    case CURLOPT_PROGRESSFUNCTION:
        curl_easy_setopt(ch->cp, CURLOPT_PROGRESSFUNCTION, installed ? swoole_curl_on_progress : nullptr);
        curl_easy_setopt(ch->cp, CURLOPT_PROGRESSDATA, ch);
        break;
    case CURLOPT_XFERINFOFUNCTION:
        curl_easy_setopt(ch->cp, CURLOPT_XFERINFOFUNCTION, installed ? swoole_curl_on_xferinfo : nullptr);
        curl_easy_setopt(ch->cp, CURLOPT_XFERINFODATA, ch);
        break;
    case CURLOPT_FNMATCH_FUNCTION:
        curl_easy_setopt(ch->cp, CURLOPT_FNMATCH_FUNCTION, installed ? swoole_curl_on_fnmatch : nullptr);
        curl_easy_setopt(ch->cp, CURLOPT_FNMATCH_DATA, ch);
        break;
    case CURLOPT_DEBUGFUNCTION:
        curl_easy_setopt(ch->cp, CURLOPT_DEBUGFUNCTION, installed ? swoole_curl_on_debug : nullptr);
        curl_easy_setopt(ch->cp, CURLOPT_DEBUGDATA, ch);
        break;
#if LIBCURL_VERSION_NUM >= 0x075000 /* Available since 7.80.0 */
    case CURLOPT_PREREQFUNCTION:
        curl_easy_setopt(ch->cp, CURLOPT_PREREQFUNCTION, installed ? swoole_curl_on_prereq : nullptr);
        curl_easy_setopt(ch->cp, CURLOPT_PREREQDATA, ch);
        break;
#endif
#if LIBCURL_VERSION_NUM >= 0x075400 /* Available since 7.84.0 */
    case CURLOPT_SSH_HOSTKEYFUNCTION:
        curl_easy_setopt(ch->cp, CURLOPT_SSH_HOSTKEYFUNCTION, installed ? swoole_curl_on_ssh_hostkey : nullptr);
        curl_easy_setopt(ch->cp, CURLOPT_SSH_HOSTKEYDATA, ch);
        break;
#endif
    }
}

/* CURLINFO_HEADER_OUT as an option: libcurl's debug callback records the request header. */
extern "C" void swoole_curl_set_header_out(php_curl *ch, bool record) {
    curl_easy_setopt(ch->cp, CURLOPT_DEBUGFUNCTION, record ? swoole_curl_on_debug : NULL);
    curl_easy_setopt(ch->cp, CURLOPT_DEBUGDATA, record ? ch : NULL);
    curl_easy_setopt(ch->cp, CURLOPT_VERBOSE, record ? 1L : 0L);
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

static size_t read_cb(char *buffer, size_t size, size_t nitems, void *arg) {
    struct mime_data_cb_arg *cb_arg = (struct mime_data_cb_arg *) arg;
    ssize_t numread;

    if (cb_arg->stream == NULL) {
        if (!(cb_arg->stream = php_stream_open_wrapper(ZSTR_VAL(cb_arg->filename), "rb", IGNORE_PATH, NULL))) {
            return CURL_READFUNC_ABORT;
        }
    }
    numread = php_stream_read(cb_arg->stream, buffer, nitems * size);
    if (numread < 0) {
        php_stream_close(cb_arg->stream);
        cb_arg->stream = NULL;
        return CURL_READFUNC_ABORT;
    }
    return numread;
}

static int seek_cb(void *arg, curl_off_t offset, int origin) {
    struct mime_data_cb_arg *cb_arg = (struct mime_data_cb_arg *) arg;

    if (cb_arg->stream == NULL) {
        return CURL_SEEKFUNC_CANTSEEK;
    }
    return php_stream_seek(cb_arg->stream, offset, origin) == SUCCESS ? CURL_SEEKFUNC_OK : CURL_SEEKFUNC_CANTSEEK;
}

static void free_cb(void *arg) {
    struct mime_data_cb_arg *cb_arg = (struct mime_data_cb_arg *) arg;

    if (cb_arg->stream != NULL) {
        php_stream_close(cb_arg->stream);
        cb_arg->stream = NULL;
    }
}

/* CURLOPT_POSTFIELDS (crates/curl/src/postfields.rs): PHP's CURLFile classes, the multipart
 * parts and the handle's copy of the fields a file upload needs again on curl_copy_handle. */
extern "C" zend_class_entry *swoole_curl_file_class() {
    return curl_CURLFile_class;
}

extern "C" zend_class_entry *swoole_curl_string_file_class() {
    return curl_CURLStringFile_class;
}

extern "C" curl_mime *swoole_curl_mime_new(php_curl *ch) {
    return curl_mime_init(ch->cp);
}

extern "C" void swoole_curl_mime_free(curl_mime *mime) {
    curl_mime_free(mime);
}

/* Each part's builder returns its CURLcode, or -1 when libcurl gave no part. */
extern "C" int swoole_curl_mime_field(curl_mime *mime, const zend_string *name, const zend_string *data) {
    curl_mimepart *part = curl_mime_addpart(mime);
    if (part == NULL) {
        return CURLE_OUT_OF_MEMORY;
    }
    CURLcode error = curl_mime_name(part, ZSTR_VAL(name));
    return error != CURLE_OK ? error : curl_mime_data(part, ZSTR_VAL(data), ZSTR_LEN(data));
}

extern "C" int swoole_curl_mime_string_file(curl_mime *mime,
                                            const zend_string *name,
                                            const zend_string *data,
                                            const zend_string *filename,
                                            const zend_string *type) {
    curl_mimepart *part = curl_mime_addpart(mime);
    if (part == NULL) {
        return -1;
    }
    CURLcode error = curl_mime_name(part, ZSTR_VAL(name));
    if (error == CURLE_OK) {
        error = curl_mime_data(part, ZSTR_VAL(data), ZSTR_LEN(data));
    }
    if (error == CURLE_OK) {
        error = curl_mime_filename(part, ZSTR_VAL(filename));
    }
    if (error == CURLE_OK) {
        error = curl_mime_type(part, ZSTR_VAL(type));
    }
    return error;
}

/* A CURLFile part: libcurl reads the file through the callbacks above, from a stream opened now
 * when it can be (and sized when it has no read filter), or on the first read. */
extern "C" int swoole_curl_mime_file(php_curl *ch,
                                     curl_mime *mime,
                                     const zend_string *name,
                                     zend_string *path,
                                     const zend_string *filename,
                                     const zend_string *type) {
    php_stream_statbuf ssb;
    size_t filesize = -1;
    curl_seek_callback seekfunc = seek_cb;
    php_stream *stream = php_stream_open_wrapper(ZSTR_VAL(path), "rb", STREAM_MUST_SEEK, NULL);
    if (stream) {
        if (!stream->readfilters.head && !php_stream_stat(stream, &ssb)) {
            filesize = ssb.sb.st_size;
        }
    } else {
        seekfunc = NULL;
    }
    curl_mimepart *part = curl_mime_addpart(mime);
    if (part == NULL) {
        if (stream) {
            php_stream_close(stream);
        }
        return -1;
    }
    struct mime_data_cb_arg *cb_arg = (struct mime_data_cb_arg *) emalloc(sizeof *cb_arg);
    cb_arg->filename = zend_string_copy(path);
    cb_arg->stream = stream;
    CURLcode error = curl_mime_name(part, ZSTR_VAL(name));
    if (error == CURLE_OK) {
        error = curl_mime_data_cb(part, filesize, read_cb, seekfunc, free_cb, cb_arg);
    }
    if (error == CURLE_OK) {
        error = curl_mime_filename(part, ZSTR_VAL(filename ? filename : path));
    }
    if (error == CURLE_OK) {
        error = curl_mime_type(part, type ? ZSTR_VAL(type) : "application/octet-stream");
    }
    zend_llist_add_element(&ch->to_free->stream, &cb_arg);
    return error;
}

/* Sets the multipart body; the handle frees it, and a handle with no clone frees the one before. */
extern "C" int swoole_curl_setopt_mime(php_curl *ch, curl_mime *mime) {
    if (*ch->clone == 1) {
        zend_llist_clean(&ch->to_free->post);
    }
    zend_llist_add_element(&ch->to_free->post, &mime);
    return ch->err.no = curl_easy_setopt(ch->cp, CURLOPT_MIMEPOST, mime);
}

/* An empty array: no multipart body (libcurl mishandles an empty one, curl/curl#6455). */
extern "C" int swoole_curl_set_empty_post(php_curl *ch) {
    curl_easy_setopt(ch->cp, CURLOPT_POSTFIELDS, "");
    return ch->err.no = curl_easy_setopt(ch->cp, CURLOPT_POSTFIELDSIZE, 0L);
}

/* The fields a CURLFile upload keeps, which curl_copy_handle builds again for the copy. */
extern "C" zval *swoole_curl_postfields(php_curl *ch) {
    return &ch->postfields;
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
