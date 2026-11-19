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
   |         Tianfeng Han  <rango@swoole.com>                             |
   +----------------------------------------------------------------------+
*/

#include "swoole_rs_phphost.h"

#if defined(SW_USE_CURL) && PHP_VERSION_ID < 80400
#include "swoole_rs_curl.h"

using namespace swoole;

SW_EXTERN_C_BEGIN
#include "curl_arginfo.h"

#include <stdio.h>
#include <string.h>

#include <curl/curl.h>
#include <curl/easy.h>

/* As of curl 7.11.1 this is no longer defined inside curl.h */
#ifndef HttpPost
#define HttpPost curl_httppost
#endif

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
#if PHP_VERSION_ID >= 80300
    if (zend_char_has_nul_byte(str, len)) {
#else
    if (strlen(str) != len) {
#endif
        zend_value_error("%s(): cURL option must not contain any null bytes", get_active_function_name());
        return FAILURE;
    }

    CURLcode error = curl_easy_setopt(ch->cp, (CURLoption) option, str);
    SAVE_CURL_ERROR(ch, error);

    return error == CURLE_OK ? SUCCESS : FAILURE;
}
#if PHP_VERSION_ID >= 80300
static zend_result php_curl_option_url(php_curl *ch, const zend_string *url) /* {{{ */
#else
static zend_result php_curl_option_url(php_curl *ch, const char *url, const size_t len) /* {{{ */
#endif
{
    /* Disable file:// if open_basedir are used */
    if (PG(open_basedir) && *PG(open_basedir)) {
        curl_easy_setopt(ch->cp, CURLOPT_PROTOCOLS, CURLPROTO_ALL & ~CURLPROTO_FILE);
    }

#if PHP_VERSION_ID >= 80300
    return php_curl_option_str(ch, CURLOPT_URL, ZSTR_VAL(url), ZSTR_LEN(url));
#else
    return php_curl_option_str(ch, CURLOPT_URL, url, len);
#endif
}
/* }}} */

/* CurlHandle class */

php_curl *swoole_curl_init_handle_into_zval(zval *curl);

SW_EXTERN_C_END

void swoole_native_curl_minit(int module_number) {
    if (!SWOOLE_G(cli)) {
        return;
    }

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

    swoole_curl_register_handle();
    curl_multi_register_class();

    zend_unregister_functions(swoole_native_curl_functions, -1, CG(function_table));
    zend_register_functions(NULL, swoole_native_curl_functions, NULL, MODULE_PERSISTENT);

    SW_INIT_CLASS_ENTRY_EX(swoole_native_curl_exception,
                           "Swoole\\Coroutine\\Curl\\Exception",
                           "Co\\Coroutine\\Curl\\Exception",
                           nullptr,
                           swoole_exception);
}

/* CurlHandle class */

void swoole_native_curl_mshutdown() {}

/* {{{ curl_write_nothing
 * Used as a work around. See _php_curl_close_ex
 */
static size_t fn_write_nothing(char *data, size_t size, size_t nmemb, void *ctx) {
    return size * nmemb;
}
/* }}} */

/* {{{ curl_free_post
 */
static void curl_free_post(void **post) {
    curl_mime_free((curl_mime *) *post);
}
/* }}} */

/* {{{ curl_free_slist
 */
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

/* {{{ alloc_curl_handle
 */
void swoole_curl_init_handle(php_curl *ch) {
    ch->to_free = (struct _php_curl_free *) ecalloc(1, sizeof(struct _php_curl_free));
    ch->handlers.write = (php_curl_write *) ecalloc(1, sizeof(php_curl_write));
    ch->handlers.write_header = (php_curl_write *) ecalloc(1, sizeof(php_curl_write));
    ch->handlers.read = (php_curl_read *) ecalloc(1, sizeof(php_curl_read));
    ch->handlers.progress = NULL;
#if PHP_VERSION_ID >= 80200
    ch->handlers.xferinfo = NULL;
#endif
    ch->handlers.fnmatch = NULL;
#if LIBCURL_VERSION_NUM >= 0x075400 && PHP_VERSION_ID >= 80300
    ch->handlers.sshhostkey = NULL;
#endif
    ch->clone = (uint32_t *) emalloc(sizeof(uint32_t));
    *ch->clone = 1;

    memset(&ch->err, 0, sizeof(struct _php_curl_error));

    zend_llist_init(&ch->to_free->post, sizeof(struct HttpPost *), (llist_dtor_func_t) curl_free_post, 0);
    zend_llist_init(&ch->to_free->stream, sizeof(void *), (llist_dtor_func_t) swoole_curl_free_cb_arg, 0);

    ch->to_free->slist = (HashTable *) emalloc(sizeof(HashTable));
    zend_hash_init(ch->to_free->slist, 4, NULL, curl_free_slist, 0);
    ZVAL_UNDEF(&ch->postfields);
}
/* }}} */

/* {{{ _php_curl_set_default_options()
   Set default options for a handle */
static void _php_curl_set_default_options(php_curl *ch) {
    const char *cainfo;

    curl_easy_setopt(ch->cp, CURLOPT_NOPROGRESS, 1L);
    curl_easy_setopt(ch->cp, CURLOPT_VERBOSE, 0L);
    curl_easy_setopt(ch->cp, CURLOPT_ERRORBUFFER, ch->err.str);
    curl_easy_setopt(ch->cp, CURLOPT_WRITEFUNCTION, swoole_curl_on_write);
    curl_easy_setopt(ch->cp, CURLOPT_FILE, (void *) ch);
    curl_easy_setopt(ch->cp, CURLOPT_READFUNCTION, swoole_curl_on_read);
    curl_easy_setopt(ch->cp, CURLOPT_INFILE, (void *) ch);
    curl_easy_setopt(ch->cp, CURLOPT_HEADERFUNCTION, swoole_curl_on_header);
    curl_easy_setopt(ch->cp, CURLOPT_WRITEHEADER, (void *) ch);
#ifndef ZTS
    curl_easy_setopt(ch->cp, CURLOPT_DNS_USE_GLOBAL_CACHE, 1L);
#endif
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
#if PHP_VERSION_ID >= 80300
    return php_curl_option_url(ch, url) == SUCCESS;
#else
    return php_curl_option_url(ch, ZSTR_VAL(url), ZSTR_LEN(url)) == SUCCESS;
#endif
}

#if PHP_VERSION_ID >= 80300
static void _php_copy_callback(php_curl *ch,
                               php_curl_callback **new_callback,
                               php_curl_callback *source_callback,
                               CURLoption option) {
    if (source_callback) {
        *new_callback = (php_curl_callback *) ecalloc(1, sizeof(php_curl_callback));
        if (!Z_ISUNDEF(source_callback->func_name)) {
            ZVAL_COPY(&(*new_callback)->func_name, &source_callback->func_name);
        }
        curl_easy_setopt(ch->cp, (CURLoption) option, (void *) ch);
    }
}
#endif

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

    if (!Z_ISUNDEF(source->handlers.write->func_name)) {
        ZVAL_COPY(&ch->handlers.write->func_name, &source->handlers.write->func_name);
    }
    if (!Z_ISUNDEF(source->handlers.read->func_name)) {
        ZVAL_COPY(&ch->handlers.read->func_name, &source->handlers.read->func_name);
    }
    if (!Z_ISUNDEF(source->handlers.write_header->func_name)) {
        ZVAL_COPY(&ch->handlers.write_header->func_name, &source->handlers.write_header->func_name);
    }

    curl_easy_setopt(ch->cp, CURLOPT_ERRORBUFFER, ch->err.str);
    curl_easy_setopt(ch->cp, CURLOPT_FILE, (void *) ch);
    curl_easy_setopt(ch->cp, CURLOPT_INFILE, (void *) ch);
    curl_easy_setopt(ch->cp, CURLOPT_WRITEHEADER, (void *) ch);
    curl_easy_setopt(ch->cp, CURLOPT_DEBUGDATA, (void *) ch);

#if PHP_VERSION_ID >= 80300
    _php_copy_callback(ch, &ch->handlers.progress, source->handlers.progress, CURLOPT_PROGRESSDATA);
    _php_copy_callback(ch, &ch->handlers.xferinfo, source->handlers.xferinfo, CURLOPT_XFERINFODATA);
    _php_copy_callback(ch, &ch->handlers.fnmatch, source->handlers.fnmatch, CURLOPT_FNMATCH_DATA);
#if LIBCURL_VERSION_NUM >= 0x075400
    _php_copy_callback(ch, &ch->handlers.sshhostkey, source->handlers.sshhostkey, CURLOPT_SSH_HOSTKEYDATA);
#endif
#else
    if (source->handlers.progress) {
        ch->handlers.progress = (php_curl_callback *) ecalloc(1, sizeof(php_curl_callback));
        if (!Z_ISUNDEF(source->handlers.progress->func_name)) {
            ZVAL_COPY(&ch->handlers.progress->func_name, &source->handlers.progress->func_name);
        }
        curl_easy_setopt(ch->cp, CURLOPT_PROGRESSDATA, (void *) ch);
    }

#if PHP_VERSION_ID >= 80200
    if (source->handlers.xferinfo) {
        ch->handlers.xferinfo = (php_curl_callback *) ecalloc(1, sizeof(php_curl_callback));
        if (!Z_ISUNDEF(source->handlers.xferinfo->func_name)) {
            ZVAL_COPY(&ch->handlers.xferinfo->func_name, &source->handlers.xferinfo->func_name);
        }
        curl_easy_setopt(ch->cp, CURLOPT_XFERINFODATA, (void *) ch);
    }
#endif

    if (source->handlers.fnmatch) {
        ch->handlers.fnmatch = (php_curl_callback *) ecalloc(1, sizeof(php_curl_callback));
        if (!Z_ISUNDEF(source->handlers.fnmatch->func_name)) {
            ZVAL_COPY(&ch->handlers.fnmatch->func_name, &source->handlers.fnmatch->func_name);
        }
        curl_easy_setopt(ch->cp, CURLOPT_FNMATCH_DATA, (void *) ch);
    }
#endif

    ZVAL_COPY(&ch->private_data, &source->private_data);

    efree(ch->to_free->slist);
    efree(ch->to_free);
    ch->to_free = source->to_free;
    efree(ch->clone);
    ch->clone = source->clone;

    /* Keep track of cloned copies to avoid invoking curl destructors for every clone */
    (*source->clone)++;
}

#if PHP_VERSION_ID >= 80300
static void _php_curl_free_callback(php_curl_callback *callback) {
    if (callback) {
        zval_ptr_dtor(&callback->func_name);
        efree(callback);
    }
}
#endif

extern "C" bool swoole_curl_prepare_free(php_curl *ch) {
    swoole_curl_verify_handlers(ch, false);
    curl_easy_setopt(ch->cp, CURLOPT_HEADERFUNCTION, fn_write_nothing);
    curl_easy_setopt(ch->cp, CURLOPT_WRITEFUNCTION, fn_write_nothing);
    return true;
}

extern "C" void swoole_curl_free_callbacks(php_curl *ch) {
    smart_str_free(&ch->handlers.write->buf);
    zval_ptr_dtor(&ch->handlers.write->func_name);
    zval_ptr_dtor(&ch->handlers.read->func_name);
    zval_ptr_dtor(&ch->handlers.write_header->func_name);
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

#if PHP_VERSION_ID >= 80300
    _php_curl_free_callback(ch->handlers.progress);
    _php_curl_free_callback(ch->handlers.xferinfo);
    _php_curl_free_callback(ch->handlers.fnmatch);
#if LIBCURL_VERSION_NUM >= 0x075400
    _php_curl_free_callback(ch->handlers.sshhostkey);
#endif
#else
    if (ch->handlers.progress) {
        zval_ptr_dtor(&ch->handlers.progress->func_name);
        efree(ch->handlers.progress);
    }

#if PHP_VERSION_ID >= 80200
    if (ch->handlers.xferinfo) {
        zval_ptr_dtor(&ch->handlers.xferinfo->func_name);
        efree(ch->handlers.xferinfo);
    }
#endif

    if (ch->handlers.fnmatch) {
        zval_ptr_dtor(&ch->handlers.fnmatch->func_name);
        efree(ch->handlers.fnmatch);
    }

#if LIBCURL_VERSION_NUM >= 0x075400 && php_version_id >= 80300
    if (ch->handlers.sshhostkey) {
        zval_ptr_dtor(&ch->handlers.sshhostkey->func_name);
        efree(ch->handlers.sshhostkey);
    }
#endif
#endif
}

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

    if (ch->handlers.progress) {
        zval_ptr_dtor(&ch->handlers.progress->func_name);
        efree(ch->handlers.progress);
        ch->handlers.progress = NULL;
    }

#if LIBCURL_VERSION_NUM >= 0x072000 && PHP_VERSION_ID >= 80200
    if (ch->handlers.xferinfo) {
        zval_ptr_dtor(&ch->handlers.xferinfo->func_name);
        efree(ch->handlers.xferinfo);
        ch->handlers.xferinfo = NULL;
    }
#endif

    if (ch->handlers.fnmatch) {
        zval_ptr_dtor(&ch->handlers.fnmatch->func_name);
        efree(ch->handlers.fnmatch);
        ch->handlers.fnmatch = NULL;
    }

#if LIBCURL_VERSION_NUM >= 0x075400 && PHP_VERSION_ID >= 80300
    if (ch->handlers.sshhostkey) {
        zval_ptr_dtor(&ch->handlers.sshhostkey->func_name);
        efree(ch->handlers.sshhostkey);
        ch->handlers.sshhostkey = NULL;
    }
#endif
}
/* }}} */

extern "C" void swoole_curl_reset_options(php_curl *ch) {
    _php_curl_reset_handlers(ch);
    _php_curl_set_default_options(ch);
}

#endif
