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

#if defined(SW_USE_CURL) && PHP_VERSION_ID >= 80400
#include "swoole_rs_curl.h"
using namespace swoole;

SW_EXTERN_C_BEGIN
#define ZEND_INCLUDE_FULL_WINDOWS_HEADERS
#include "curl_arginfo.h"

#include <stdio.h>
#include <string.h>

#ifdef PHP_WIN32
#include <winsock2.h>
#include <sys/types.h>
#endif

#include <curl/curl.h>
#include <curl/easy.h>

/* As of curl 7.11.1 this is no longer defined inside curl.h */
#ifndef HttpPost
#define HttpPost curl_httppost
#endif

#if PHP_VERSION_ID < 80500
/* {{{ cruft for thread safe SSL crypto locks */
#if defined(ZTS) && defined(HAVE_CURL_OLD_OPENSSL)
#if defined(HAVE_OPENSSL_CRYPTO_H)
#define PHP_CURL_NEED_OPENSSL_TSL
#include <openssl/crypto.h>
#else
#warning "libcurl was compiled with OpenSSL support, but configure could not find " \
	"openssl/crypto.h; thus no SSL crypto locking callbacks will be set, which may " \
	"cause random crashes on SSL requests"
#endif
#endif /* ZTS && HAVE_CURL_OLD_OPENSSL */
/* }}} */
#endif

#include "zend_smart_str.h"
#include "ext/standard/info.h"
#include "ext/standard/file.h"
#include "ext/standard/url.h"
#include "curl_private.h"

#ifdef __GNUC__
/* don't complain about deprecated CURLOPT_* we're exposing to PHP; we
   need to keep using those to avoid breaking PHP API compatibiltiy */
#pragma GCC diagnostic ignored "-Wdeprecated-declarations"
#endif

#if PHP_VERSION_ID < 80500
#ifdef PHP_CURL_NEED_OPENSSL_TSL /* {{{ */
static MUTEX_T *php_curl_openssl_tsl = NULL;

/* Locking callbacks are no longer used since OpenSSL 1.1. Mark the functions as unused to
 * avoid warnings due to this. */
static ZEND_ATTRIBUTE_UNUSED void php_curl_ssl_lock(int mode, int n, const char *file, int line) {
    if (mode & CRYPTO_LOCK) {
        tsrm_mutex_lock(php_curl_openssl_tsl[n]);
    } else {
        tsrm_mutex_unlock(php_curl_openssl_tsl[n]);
    }
}

static ZEND_ATTRIBUTE_UNUSED unsigned long php_curl_ssl_id(void) {
    return (unsigned long) tsrm_thread_id();
}
#endif
/* }}} */
#endif

static zend_class_entry *swoole_native_curl_exception_ce;
static zend_object_handlers swoole_native_curl_exception_handlers;


#if defined(PHP_WIN32) || defined(__GNUC__)
#define php_curl_ret(__ret)                                                                                            \
    RETVAL_FALSE;                                                                                                      \
    return __ret;
#else
#define php_curl_ret(__ret)                                                                                            \
    RETVAL_FALSE;                                                                                                      \
    return;
#endif

static zend_result php_curl_option_str(php_curl *ch, zend_long option, const char *str, const size_t len) {
    if (zend_char_has_nul_byte(str, len)) {
        zend_value_error("%s(): cURL option must not contain any null bytes", get_active_function_name());
        return FAILURE;
    }

    CURLcode error = curl_easy_setopt(ch->cp, (CURLoption) option, str);
    SAVE_CURL_ERROR(ch, error);

    return error == CURLE_OK ? SUCCESS : FAILURE;
}

static zend_result php_curl_option_url(php_curl *ch, const zend_string *url) /* {{{ */
{
    /* Disable file:// if open_basedir are used */
    if (PG(open_basedir) && *PG(open_basedir)) {
        curl_easy_setopt(ch->cp, CURLOPT_PROTOCOLS, CURLPROTO_ALL & ~CURLPROTO_FILE);
    }

#ifdef PHP_WIN32
    if (zend_string_starts_with_literal_ci(url, "file://") && '/' != ZSTR_VAL(url)[sizeof("file://") - 1] &&
        ZSTR_LEN(url) < MAXPATHLEN - 2) {
        char _tmp[MAXPATHLEN] = {0};

        memmove(_tmp, "file:///", sizeof("file:///") - 1);
        memmove(_tmp + sizeof("file:///") - 1,
                ZSTR_VAL(url) + sizeof("file://") - 1,
                ZSTR_LEN(url) - sizeof("file://") + 1);

        return php_curl_option_str(ch, CURLOPT_URL, _tmp, ZSTR_LEN(url) + 1);
    }
#endif

    return php_curl_option_str(ch, CURLOPT_URL, ZSTR_VAL(url), ZSTR_LEN(url));
}
/* }}} */

/* CurlHandle class */

SW_EXTERN_C_END

void swoole_native_curl_minit(int module_number) {
    if (!SWOOLE_G(cli)) {
        return;
    }

#if PHP_VERSION_ID < 80500
#ifdef PHP_CURL_NEED_OPENSSL_TSL
    if (CRYPTO_get_id_callback() == nullptr) {
        int i, c = CRYPTO_num_locks();

        php_curl_openssl_tsl = (MUTEX_T *) malloc(c * sizeof(MUTEX_T));
        if (!php_curl_openssl_tsl) {
            return;
        }

        for (i = 0; i < c; ++i) {
            php_curl_openssl_tsl[i] = tsrm_mutex_alloc();
        }

        CRYPTO_set_id_callback(php_curl_ssl_id);
        CRYPTO_set_locking_callback(php_curl_ssl_lock);
    }
#endif
#endif

    swoole_curl_register_handle();
    curl_multi_register_class(nullptr);

    zend_unregister_functions(swoole_native_curl_functions, -1, CG(function_table));
    zend_register_functions(NULL, swoole_native_curl_functions, NULL, MODULE_PERSISTENT);

    SW_INIT_CLASS_ENTRY_EX(swoole_native_curl_exception,
                           "Swoole\\Coroutine\\Curl\\Exception",
                           "Co\\Coroutine\\Curl\\Exception",
                           nullptr,
                           swoole_exception);
}
/* }}} */

/* CurlHandle class */

void swoole_native_curl_mshutdown() {}

/* {{{ curl_free_post */
static void curl_free_post(void **post) {
    curl_mime_free((curl_mime *) *post);
}
/* }}} */

/* {{{ curl_free_slist */
static void curl_free_slist(zval *el) {
    curl_slist_free_all(((struct curl_slist *) Z_PTR_P(el)));
}
/* }}} */

php_curl *swoole_curl_init_handle_into_zval(zval *curl) {
    php_curl *ch;

    object_init_ex(curl, swoole_coroutine_curl_handle_ce);
    ch = Z_CURL_P(curl);

    swoole_curl_init_handle(ch);

    return ch;
}

void swoole_curl_init_handle(php_curl *ch) {
    ch->to_free = (struct _php_curl_free *) ecalloc(1, sizeof(struct _php_curl_free));
    ch->handlers.write = (php_curl_write *) ecalloc(1, sizeof(php_curl_write));
    ch->handlers.write_header = (php_curl_write *) ecalloc(1, sizeof(php_curl_write));
    ch->handlers.read = (php_curl_read *) ecalloc(1, sizeof(php_curl_read));
    ch->handlers.progress = empty_fcall_info_cache;
    ch->handlers.xferinfo = empty_fcall_info_cache;
    ch->handlers.fnmatch = empty_fcall_info_cache;
    ch->handlers.debug = empty_fcall_info_cache;
#if LIBCURL_VERSION_NUM >= 0x075000 /* Available since 7.80.0 */
    ch->handlers.prereq = empty_fcall_info_cache;
#endif
#if LIBCURL_VERSION_NUM >= 0x075400 /* Available since 7.84.0 */
    ch->handlers.sshhostkey = empty_fcall_info_cache;
#endif
    ch->clone = (uint32_t *) emalloc(sizeof(uint32_t));
    *ch->clone = 1;

    memset(&ch->err, 0, sizeof(struct _php_curl_error));

    zend_llist_init(&ch->to_free->post, sizeof(struct HttpPost *), (llist_dtor_func_t) curl_free_post, 0);
    zend_llist_init(&ch->to_free->stream, sizeof(void *), (llist_dtor_func_t) swoole_curl_free_cb_arg, 0);
#if PHP_VERSION_ID >= 80500
    zend_hash_init(&ch->to_free->slist, 4, NULL, curl_free_slist, 0);
#else
    ch->to_free->slist = (HashTable *) emalloc(sizeof(HashTable));
    zend_hash_init(ch->to_free->slist, 4, NULL, curl_free_slist, 0);
#endif
    ZVAL_UNDEF(&ch->postfields);
}

/* }}} */

/* {{{ _php_curl_set_default_options()
   Set default options for a handle */
static void _php_curl_set_default_options(php_curl *ch) {
    char *cainfo;

    curl_easy_setopt(ch->cp, CURLOPT_NOPROGRESS, 1L);
    curl_easy_setopt(ch->cp, CURLOPT_VERBOSE, 0L);
    curl_easy_setopt(ch->cp, CURLOPT_ERRORBUFFER, ch->err.str);
    curl_easy_setopt(ch->cp, CURLOPT_WRITEFUNCTION, swoole_curl_on_write);
    curl_easy_setopt(ch->cp, CURLOPT_FILE, (void *) ch);
    curl_easy_setopt(ch->cp, CURLOPT_READFUNCTION, swoole_curl_on_read);
    curl_easy_setopt(ch->cp, CURLOPT_INFILE, (void *) ch);
    curl_easy_setopt(ch->cp, CURLOPT_HEADERFUNCTION, swoole_curl_on_header);
    curl_easy_setopt(ch->cp, CURLOPT_WRITEHEADER, (void *) ch);
    curl_easy_setopt(ch->cp, CURLOPT_DNS_CACHE_TIMEOUT, 120L);
    curl_easy_setopt(ch->cp, CURLOPT_MAXREDIRS, 20L); /* prevent infinite redirects */

    cainfo = INI_STR("openssl.cafile");
    if (!(cainfo && cainfo[0] != '\0')) {
        cainfo = INI_STR("curl.cainfo");
    }
    if (cainfo && cainfo[0] != '\0') {
        curl_easy_setopt(ch->cp, CURLOPT_CAINFO, cainfo);
    }

#ifdef ZTS
    curl_easy_setopt(ch->cp, CURLOPT_NOSIGNAL, 1L);
#endif
}
/* }}} */

extern "C" void swoole_curl_default_options(php_curl *ch) {
    _php_curl_set_default_options(ch);
}

extern "C" bool swoole_curl_set_url(php_curl *ch, const zend_string *url) {
    return php_curl_option_url(ch, url) == SUCCESS;
}

static void php_curl_copy_fcc_with_option(php_curl *ch,
                                          CURLoption option,
                                          zend_fcall_info_cache *target_fcc,
                                          zend_fcall_info_cache *source_fcc) {
    if (ZEND_FCC_INITIALIZED(*source_fcc)) {
        zend_fcc_dup(target_fcc, source_fcc);
        curl_easy_setopt(ch->cp, (CURLoption) option, (void *) ch);
    }
}

void swoole_setup_easy_copy_handlers(php_curl *ch, php_curl *source) {
    if (!Z_ISUNDEF(source->handlers.write->stream)) {
        Z_ADDREF(source->handlers.write->stream);
    }
    ch->handlers.write->stream = source->handlers.write->stream;
    ch->handlers.write->method = source->handlers.write->method;
    if (!Z_ISUNDEF(source->handlers.read->stream)) {
        Z_ADDREF(source->handlers.read->stream);
    }
    ch->handlers.read->stream = source->handlers.read->stream;
    ch->handlers.read->method = source->handlers.read->method;
    ch->handlers.write_header->method = source->handlers.write_header->method;
    if (!Z_ISUNDEF(source->handlers.write_header->stream)) {
        Z_ADDREF(source->handlers.write_header->stream);
    }
    ch->handlers.write_header->stream = source->handlers.write_header->stream;

    ch->handlers.write->fp = source->handlers.write->fp;
    ch->handlers.write_header->fp = source->handlers.write_header->fp;
    ch->handlers.read->fp = source->handlers.read->fp;
    ch->handlers.read->res = source->handlers.read->res;

    if (ZEND_FCC_INITIALIZED(source->handlers.read->fcc)) {
        zend_fcc_dup(&ch->handlers.read->fcc, &source->handlers.read->fcc);
    }
    if (ZEND_FCC_INITIALIZED(source->handlers.write->fcc)) {
        zend_fcc_dup(&ch->handlers.write->fcc, &source->handlers.write->fcc);
    }
    if (ZEND_FCC_INITIALIZED(source->handlers.write_header->fcc)) {
        zend_fcc_dup(&ch->handlers.write_header->fcc, &source->handlers.write_header->fcc);
    }

    curl_easy_setopt(ch->cp, CURLOPT_ERRORBUFFER, ch->err.str);
    curl_easy_setopt(ch->cp, CURLOPT_FILE, (void *) ch);
    curl_easy_setopt(ch->cp, CURLOPT_INFILE, (void *) ch);
    curl_easy_setopt(ch->cp, CURLOPT_WRITEHEADER, (void *) ch);
    curl_easy_setopt(ch->cp, CURLOPT_DEBUGDATA, (void *) ch);

    php_curl_copy_fcc_with_option(ch, CURLOPT_PROGRESSDATA, &ch->handlers.progress, &source->handlers.progress);
    php_curl_copy_fcc_with_option(ch, CURLOPT_XFERINFODATA, &ch->handlers.xferinfo, &source->handlers.xferinfo);
    php_curl_copy_fcc_with_option(ch, CURLOPT_FNMATCH_DATA, &ch->handlers.fnmatch, &source->handlers.fnmatch);
    php_curl_copy_fcc_with_option(ch, CURLOPT_DEBUGDATA, &ch->handlers.debug, &source->handlers.debug);
#if LIBCURL_VERSION_NUM >= 0x075000 /* Available since 7.80.0 */
    php_curl_copy_fcc_with_option(ch, CURLOPT_PREREQDATA, &ch->handlers.prereq, &source->handlers.prereq);
#endif
#if LIBCURL_VERSION_NUM >= 0x075400 /* Available since 7.84.0 */
    php_curl_copy_fcc_with_option(ch, CURLOPT_SSH_HOSTKEYDATA, &ch->handlers.sshhostkey, &source->handlers.sshhostkey);
#endif

    ZVAL_COPY(&ch->private_data, &source->private_data);

#if PHP_VERSION_ID < 80500
    efree(ch->to_free->slist);
#endif
    efree(ch->to_free);
    ch->to_free = source->to_free;
    efree(ch->clone);
    ch->clone = source->clone;

    /* Keep track of cloned copies to avoid invoking curl destructors for every clone */
    (*source->clone)++;
}

zend_long swoole_curl_get_long(zval *zv)
{
    if (EXPECTED(Z_TYPE_P(zv) == IS_LONG)) {
        return Z_LVAL_P(zv);
    } else {
        zend_long ret = zval_get_long(zv);
        zval_ptr_dtor(zv);
        return ret;
    }
}

/* {{{ _php_curl_cleanup_handle(ch)
   Cleanup an execution phase */
void swoole_curl_cleanup_handle(php_curl *ch) {
    smart_str_free(&ch->handlers.write->buf);
    if (ch->header.str) {
        zend_string_release_ex(ch->header.str, 0);
        ch->header.str = NULL;
    }

    memset(ch->err.str, 0, CURL_ERROR_SIZE + 1);
    ch->err.no = 0;
}
/* }}} */

/* {{{ Perform a cURL session */
PHP_FUNCTION(swoole_native_curl_exec) {
    CURLcode error;
    zval *zid;
    php_curl *ch;

    ZEND_PARSE_PARAMETERS_START(1, 1)
    Z_PARAM_OBJECT_OF_CLASS(zid, swoole_coroutine_curl_handle_ce)
    ZEND_PARSE_PARAMETERS_END();

    if ((ch = swoole_curl_get_handle(zid)) == NULL) {
        RETURN_FALSE;
    }

    swoole_curl_verify_handlers(ch, 1);

    swoole_curl_cleanup_handle(ch);

    error = swoole_curl_easy_perform(ch->cp);

    SAVE_CURL_ERROR(ch, error);

    if (error != CURLE_OK) {
        smart_str_free(&ch->handlers.write->buf);
        RETURN_FALSE;
    }

    if (!Z_ISUNDEF(ch->handlers.std_err)) {
        php_stream *stream;
        stream = (php_stream *) zend_fetch_resource2_ex(
            &ch->handlers.std_err, NULL, php_file_le_stream(), php_file_le_pstream());
        if (stream) {
            php_stream_flush(stream);
        }
    }

    if (ch->handlers.write->method == PHP_CURL_RETURN && ch->handlers.write->buf.s) {
        smart_str_0(&ch->handlers.write->buf);
        RETURN_STR_COPY(ch->handlers.write->buf.s);
    }

    /* flush the file handle, so any remaining data is synched to disk */
    if (ch->handlers.write->method == PHP_CURL_FILE && ch->handlers.write->fp) {
        fflush(ch->handlers.write->fp);
    }
    if (ch->handlers.write_header->method == PHP_CURL_FILE && ch->handlers.write_header->fp) {
        fflush(ch->handlers.write_header->fp);
    }

    if (ch->handlers.write->method == PHP_CURL_RETURN) {
        RETURN_EMPTY_STRING();
    } else {
        RETURN_TRUE;
    }
}
/* }}} */

extern "C" bool swoole_curl_prepare_free(php_curl *ch) {
    swoole_curl_verify_handlers(ch, false);
    if (!swr_v1_curl_get_handle(ch->cp)) {
        swoole_curl_free_original(&ch->std);
        return false;
    }
    return true;
}

extern "C" void swoole_curl_free_callbacks(php_curl *ch) {
    smart_str_free(&ch->handlers.write->buf);
    if (ZEND_FCC_INITIALIZED(ch->handlers.write->fcc)) {
        zend_fcc_dtor(&ch->handlers.write->fcc);
    }
    if (ZEND_FCC_INITIALIZED(ch->handlers.write_header->fcc)) {
        zend_fcc_dtor(&ch->handlers.write_header->fcc);
    }
    if (ZEND_FCC_INITIALIZED(ch->handlers.read->fcc)) {
        zend_fcc_dtor(&ch->handlers.read->fcc);
    }
    zval_ptr_dtor(&ch->handlers.std_err);
    if (ch->header.str) {
        zend_string_release_ex(ch->header.str, 0);
    }

    zval_ptr_dtor(&ch->handlers.write_header->stream);
    zval_ptr_dtor(&ch->handlers.write->stream);
    zval_ptr_dtor(&ch->handlers.read->stream);

    efree(ch->handlers.write);
    efree(ch->handlers.write_header);
    efree(ch->handlers.read);

    if (ZEND_FCC_INITIALIZED(ch->handlers.progress)) {
        zend_fcc_dtor(&ch->handlers.progress);
    }
    if (ZEND_FCC_INITIALIZED(ch->handlers.xferinfo)) {
        zend_fcc_dtor(&ch->handlers.xferinfo);
    }
    if (ZEND_FCC_INITIALIZED(ch->handlers.fnmatch)) {
        zend_fcc_dtor(&ch->handlers.fnmatch);
    }
    if (ZEND_FCC_INITIALIZED(ch->handlers.debug)) {
        zend_fcc_dtor(&ch->handlers.debug);
    }
#if LIBCURL_VERSION_NUM >= 0x075000 /* Available since 7.80.0 */
    if (ZEND_FCC_INITIALIZED(ch->handlers.prereq)) {
        zend_fcc_dtor(&ch->handlers.prereq);
    }
#endif

#if LIBCURL_VERSION_NUM >= 0x075400 /* Available since 7.84.0 */
    if (ZEND_FCC_INITIALIZED(ch->handlers.sshhostkey)) {
        zend_fcc_dtor(&ch->handlers.sshhostkey);
    }
#endif
}
/* }}} */

/* {{{ _php_curl_reset_handlers()
   Reset all handlers of a given php_curl */
static void _php_curl_reset_handlers(php_curl *ch) {
    if (!Z_ISUNDEF(ch->handlers.write->stream)) {
        zval_ptr_dtor(&ch->handlers.write->stream);
        ZVAL_UNDEF(&ch->handlers.write->stream);
    }
    ch->handlers.write->fp = NULL;
    ch->handlers.write->method = PHP_CURL_STDOUT;

    if (!Z_ISUNDEF(ch->handlers.write_header->stream)) {
        zval_ptr_dtor(&ch->handlers.write_header->stream);
        ZVAL_UNDEF(&ch->handlers.write_header->stream);
    }
    ch->handlers.write_header->fp = NULL;
    ch->handlers.write_header->method = PHP_CURL_IGNORE;

    if (!Z_ISUNDEF(ch->handlers.read->stream)) {
        zval_ptr_dtor(&ch->handlers.read->stream);
        ZVAL_UNDEF(&ch->handlers.read->stream);
    }
    ch->handlers.read->fp = NULL;
    ch->handlers.read->res = NULL;
    ch->handlers.read->method = PHP_CURL_DIRECT;

    if (!Z_ISUNDEF(ch->handlers.std_err)) {
        zval_ptr_dtor(&ch->handlers.std_err);
        ZVAL_UNDEF(&ch->handlers.std_err);
    }

    if (ZEND_FCC_INITIALIZED(ch->handlers.progress)) {
        zend_fcc_dtor(&ch->handlers.progress);
    }

    if (ZEND_FCC_INITIALIZED(ch->handlers.xferinfo)) {
        zend_fcc_dtor(&ch->handlers.xferinfo);
    }

    if (ZEND_FCC_INITIALIZED(ch->handlers.fnmatch)) {
        zend_fcc_dtor(&ch->handlers.fnmatch);
    }

    if (ZEND_FCC_INITIALIZED(ch->handlers.debug)) {
        zend_fcc_dtor(&ch->handlers.debug);
    }
#if LIBCURL_VERSION_NUM >= 0x075000 /* Available since 7.80.0 */
    if (ZEND_FCC_INITIALIZED(ch->handlers.prereq)) {
        zend_fcc_dtor(&ch->handlers.prereq);
    }
#endif

#if LIBCURL_VERSION_NUM >= 0x075400 /* Available since 7.84.0 */
    if (ZEND_FCC_INITIALIZED(ch->handlers.sshhostkey)) {
        zend_fcc_dtor(&ch->handlers.sshhostkey);
    }
#endif
}
/* }}} */

extern "C" void swoole_curl_reset_options(php_curl *ch) {
    _php_curl_reset_handlers(ch);
    _php_curl_set_default_options(ch);
}

#endif
