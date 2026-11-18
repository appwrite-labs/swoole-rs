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

#define CAAL(s, v) add_assoc_long_ex(return_value, s, sizeof(s) - 1, (zend_long) v);
#define CAAD(s, v) add_assoc_double_ex(return_value, s, sizeof(s) - 1, (double) v);
#define CAAS(s, v) add_assoc_string_ex(return_value, s, sizeof(s) - 1, (char *) (v ? v : ""));
#define CAASTR(s, v) add_assoc_str_ex(return_value, s, sizeof(s) - 1, v ? zend_string_copy(v) : ZSTR_EMPTY_ALLOC());
#define CAAZ(s, v) add_assoc_zval_ex(return_value, s, sizeof(s) - 1, (zval *) v);

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

void swoole_curl_verify_handlers(php_curl *ch, bool reporterror) /* {{{ */
{
    php_stream *stream;

    ZEND_ASSERT(ch);

    if (!Z_ISUNDEF(ch->handlers.std_err)) {
        stream = (php_stream *) zend_fetch_resource2_ex(
            &ch->handlers.std_err, NULL, php_file_le_stream(), php_file_le_pstream());
        if (stream == NULL) {
            if (reporterror) {
                php_error_docref(NULL, E_WARNING, "CURLOPT_STDERR resource has gone away, resetting to stderr");
            }
            zval_ptr_dtor(&ch->handlers.std_err);
            ZVAL_UNDEF(&ch->handlers.std_err);

            curl_easy_setopt(ch->cp, CURLOPT_STDERR, stderr);
        }
    }
    if (ch->handlers.read && !Z_ISUNDEF(ch->handlers.read->stream)) {
        stream = (php_stream *) zend_fetch_resource2_ex(
            &ch->handlers.read->stream, NULL, php_file_le_stream(), php_file_le_pstream());
        if (stream == NULL) {
            if (reporterror) {
                php_error_docref(NULL, E_WARNING, "CURLOPT_INFILE resource has gone away, resetting to default");
            }
            zval_ptr_dtor(&ch->handlers.read->stream);
            ZVAL_UNDEF(&ch->handlers.read->stream);
            ch->handlers.read->res = NULL;
            ch->handlers.read->fp = 0;

            curl_easy_setopt(ch->cp, CURLOPT_INFILE, (void *) ch);
        }
    }
    if (ch->handlers.write_header && !Z_ISUNDEF(ch->handlers.write_header->stream)) {
        stream = (php_stream *) zend_fetch_resource2_ex(
            &ch->handlers.write_header->stream, NULL, php_file_le_stream(), php_file_le_pstream());
        if (stream == NULL) {
            if (reporterror) {
                php_error_docref(NULL, E_WARNING, "CURLOPT_WRITEHEADER resource has gone away, resetting to default");
            }
            zval_ptr_dtor(&ch->handlers.write_header->stream);
            ZVAL_UNDEF(&ch->handlers.write_header->stream);
            ch->handlers.write_header->fp = 0;

            ch->handlers.write_header->method = PHP_CURL_IGNORE;
            curl_easy_setopt(ch->cp, CURLOPT_WRITEHEADER, (void *) ch);
        }
    }
    if (ch->handlers.write && !Z_ISUNDEF(ch->handlers.write->stream)) {
        stream = (php_stream *) zend_fetch_resource2_ex(
            &ch->handlers.write->stream, NULL, php_file_le_stream(), php_file_le_pstream());
        if (stream == NULL) {
            if (reporterror) {
                php_error_docref(NULL, E_WARNING, "CURLOPT_FILE resource has gone away, resetting to default");
            }
            zval_ptr_dtor(&ch->handlers.write->stream);
            ZVAL_UNDEF(&ch->handlers.write->stream);
            ch->handlers.write->fp = 0;

            ch->handlers.write->method = PHP_CURL_STDOUT;
            curl_easy_setopt(ch->cp, CURLOPT_FILE, (void *) ch);
        }
    }
    return;
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
    curl_multi_register_class(nullptr);

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

/* {{{ curl_write
 */
static size_t fn_write(char *data, size_t size, size_t nmemb, void *ctx) {
    php_curl *ch = (php_curl *) ctx;
    php_curl_write *t = ch->handlers.write;
    size_t length = size * nmemb;

#if PHP_CURL_DEBUG
    fprintf(stderr, "curl_write() called\n");
    fprintf(stderr, "data = %s, size = %d, nmemb = %d, ctx = %x\n", data, size, nmemb, ctx);
#endif

    switch (t->method) {
    case PHP_CURL_STDOUT:
        PHPWRITE(data, length);
        break;
    case PHP_CURL_FILE:
        return fwrite(data, size, nmemb, t->fp);
    case PHP_CURL_RETURN:
        if (length > 0) {
            smart_str_appendl(&t->buf, data, (int) length);
        }
        break;
    case PHP_CURL_USER: {
        zval argv[2];
        zval retval;
        int error;
        zend_fcall_info fci;

        GC_ADDREF(&ch->std);
        ZVAL_OBJ(&argv[0], &ch->std);
        ZVAL_STRINGL(&argv[1], data, length);

        fci.size = sizeof(fci);
        fci.object = NULL;
        ZVAL_COPY_VALUE(&fci.function_name, &t->func_name);
        fci.retval = &retval;
        fci.param_count = 2;
        fci.params = argv;
        fci.named_params = NULL;
        ch->in_callback = 1;
        error = zend_call_function(&fci, &t->fci_cache);
        ch->in_callback = 0;
        if (error == FAILURE) {
            php_error_docref(NULL, E_WARNING, "Could not call the CURLOPT_WRITEFUNCTION");
            length = -1;
        } else if (!Z_ISUNDEF(retval)) {
            swoole_curl_verify_handlers(ch, /* reporterror */ true);
#if PHP_VERSION_ID >= 80300
            length = swoole_curl_get_long(&retval);
#else
            length = zval_get_long(&retval);
#endif
        }

        zval_ptr_dtor(&argv[0]);
        zval_ptr_dtor(&argv[1]);
        break;
    }
    }

    return length;
}
/* }}} */

/* {{{ curl_fnmatch
 */
static int fn_fnmatch(void *ctx, const char *pattern, const char *string) {
    php_curl *ch = (php_curl *) ctx;
    php_curl_callback *t = ch->handlers.fnmatch;
    int rval = CURL_FNMATCHFUNC_FAIL;
    zval argv[3];
    zval retval;
    zend_result error;
    zend_fcall_info fci;

    GC_ADDREF(&ch->std);
    ZVAL_OBJ(&argv[0], &ch->std);
    ZVAL_STRING(&argv[1], pattern);
    ZVAL_STRING(&argv[2], string);

    fci.size = sizeof(fci);
    ZVAL_COPY_VALUE(&fci.function_name, &t->func_name);
    fci.object = NULL;
    fci.retval = &retval;
    fci.param_count = 3;
    fci.params = argv;
    fci.named_params = NULL;

    ch->in_callback = 1;
    error = zend_call_function(&fci, &t->fci_cache);
    ch->in_callback = 0;
    if (error == FAILURE) {
        php_error_docref(NULL, E_WARNING, "Cannot call the CURLOPT_FNMATCH_FUNCTION");
    } else if (!Z_ISUNDEF(retval)) {
        swoole_curl_verify_handlers(ch, /* reporterror */ true);
#if PHP_VERSION_ID >= 80300
        rval = swoole_curl_get_long(&retval);
#else
        rval = zval_get_long(&retval);
#endif
    }
    zval_ptr_dtor(&argv[0]);
    zval_ptr_dtor(&argv[1]);
    zval_ptr_dtor(&argv[2]);
    return rval;
}
/* }}} */

/* {{{ curl_progress
 */
static int fn_progress(void *clientp, double dltotal, double dlnow, double ultotal, double ulnow) {
    php_curl *ch = (php_curl *) clientp;
    php_curl_callback *t = ch->handlers.progress;
    int rval = 0;

#if PHP_CURL_DEBUG
    fprintf(stderr, "curl_progress() called\n");
    fprintf(stderr,
            "clientp = %x, dltotal = %f, dlnow = %f, ultotal = %f, ulnow = %f\n",
            clientp,
            dltotal,
            dlnow,
            ultotal,
            ulnow);
#endif

    zval argv[5];
    zval retval;
    zend_result error;
    zend_fcall_info fci;

    GC_ADDREF(&ch->std);
    ZVAL_OBJ(&argv[0], &ch->std);
    ZVAL_LONG(&argv[1], (zend_long) dltotal);
    ZVAL_LONG(&argv[2], (zend_long) dlnow);
    ZVAL_LONG(&argv[3], (zend_long) ultotal);
    ZVAL_LONG(&argv[4], (zend_long) ulnow);

    fci.size = sizeof(fci);
    ZVAL_COPY_VALUE(&fci.function_name, &t->func_name);
    fci.object = NULL;
    fci.retval = &retval;
    fci.param_count = 5;
    fci.params = argv;
    fci.named_params = NULL;

    ch->in_callback = 1;
    error = zend_call_function(&fci, &t->fci_cache);
    ch->in_callback = 0;
    if (error == FAILURE) {
        php_error_docref(NULL, E_WARNING, "Cannot call the CURLOPT_PROGRESSFUNCTION");
    } else if (!Z_ISUNDEF(retval)) {
        swoole_curl_verify_handlers(ch, /* reporterror */ true);
#if PHP_VERSION_ID >= 80300
        if (0 != swoole_curl_get_long(&retval)) {
#else
        if (0 != zval_get_long(&retval)) {
#endif
            rval = 1;
        }
    }
    zval_ptr_dtor(&argv[0]);
    return rval;
}
/* }}} */

#if LIBCURL_VERSION_NUM >= 0x072000 && PHP_VERSION_ID >= 80200
/* {{{ curl_xferinfo */
static size_t fn_xferinfo(void *clientp, curl_off_t dltotal, curl_off_t dlnow, curl_off_t ultotal, curl_off_t ulnow) {
    php_curl *ch = (php_curl *) clientp;
    php_curl_callback *t = ch->handlers.xferinfo;
    int rval = 0;

#if PHP_CURL_DEBUG
    fprintf(stderr, "curl_xferinfo() called\n");
    fprintf(stderr,
            "clientp = %x, dltotal = %ld, dlnow = %ld, ultotal = %ld, ulnow = %ld\n",
            clientp,
            dltotal,
            dlnow,
            ultotal,
            ulnow);
#endif

    zval argv[5];
    zval retval;
    zend_result error;
    zend_fcall_info fci;

    GC_ADDREF(&ch->std);
    ZVAL_OBJ(&argv[0], &ch->std);
    ZVAL_LONG(&argv[1], dltotal);
    ZVAL_LONG(&argv[2], dlnow);
    ZVAL_LONG(&argv[3], ultotal);
    ZVAL_LONG(&argv[4], ulnow);

    fci.size = sizeof(fci);
    ZVAL_COPY_VALUE(&fci.function_name, &t->func_name);
    fci.object = NULL;
    fci.retval = &retval;
    fci.param_count = 5;
    fci.params = argv;
    fci.named_params = NULL;

    ch->in_callback = 1;
    error = zend_call_function(&fci, &t->fci_cache);
    ch->in_callback = 0;
    if (error == FAILURE) {
        php_error_docref(NULL, E_WARNING, "Cannot call the CURLOPT_XFERINFOFUNCTION");
    } else if (!Z_ISUNDEF(retval)) {
        swoole_curl_verify_handlers(ch, /* reporterror */ true);
#if PHP_VERSION_ID >= 80300
        if (0 != swoole_curl_get_long(&retval)) {
#else
        if (0 != zval_get_long(&retval)) {
#endif
            rval = 1;
        }
    }
    zval_ptr_dtor(&argv[0]);
    return rval;
}
/* }}} */
#endif

#if LIBCURL_VERSION_NUM >= 0x075400 && PHP_VERSION_ID >= 80300
static int fn_ssh_hostkeyfunction(void *clientp, int keytype, const char *key, size_t keylen) {
    php_curl *ch = (php_curl *) clientp;
    php_curl_callback *t = ch->handlers.sshhostkey;
    int rval = CURLKHMATCH_MISMATCH; /* cancel connection in case of an exception */

#if PHP_CURL_DEBUG
    fprintf(stderr, "curl_ssh_hostkeyfunction() called\n");
    fprintf(stderr, "clientp = %x, keytype = %d, key = %s, keylen = %zu\n", clientp, keytype, key, keylen);
#endif

    zval argv[4];
    zval retval;
    zend_result error;
    zend_fcall_info fci;

    GC_ADDREF(&ch->std);
    ZVAL_OBJ(&argv[0], &ch->std);
    ZVAL_LONG(&argv[1], keytype);
    ZVAL_STRINGL(&argv[2], key, keylen);
    ZVAL_LONG(&argv[3], keylen);

    fci.size = sizeof(fci);
    ZVAL_COPY_VALUE(&fci.function_name, &t->func_name);
    fci.object = NULL;
    fci.retval = &retval;
    fci.param_count = 4;
    fci.params = argv;
    fci.named_params = NULL;

    ch->in_callback = 1;
    error = zend_call_function(&fci, &t->fci_cache);
    ch->in_callback = 0;
    if (error == FAILURE) {
        php_error_docref(NULL, E_WARNING, "Cannot call the CURLOPT_SSH_HOSTKEYFUNCTION");
    } else if (!Z_ISUNDEF(retval)) {
        swoole_curl_verify_handlers(ch, /* reporterror */ true);
        if (Z_TYPE(retval) == IS_LONG) {
            zend_long retval_long = Z_LVAL(retval);
            if (retval_long == CURLKHMATCH_OK || retval_long == CURLKHMATCH_MISMATCH) {
                rval = retval_long;
            } else {
                zend_throw_error(NULL,
                                 "The CURLOPT_SSH_HOSTKEYFUNCTION callback must return either CURLKHMATCH_OK or "
                                 "CURLKHMATCH_MISMATCH");
            }
        } else {
            zend_throw_error(
                NULL,
                "The CURLOPT_SSH_HOSTKEYFUNCTION callback must return either CURLKHMATCH_OK or CURLKHMATCH_MISMATCH");
#if PHP_VERSION_ID >= 80300
            zval_ptr_dtor(&retval);
#endif
        }
    }
    zval_ptr_dtor(&argv[0]);
    zval_ptr_dtor(&argv[2]);
    return rval;
}
#endif

/* {{{ curl_read
 */
static size_t fn_read(char *data, size_t size, size_t nmemb, void *ctx) {
    php_curl *ch = (php_curl *) ctx;
    php_curl_read *t = ch->handlers.read;
    int length = 0;

    switch (t->method) {
    case PHP_CURL_DIRECT:
        if (t->fp) {
            length = fread(data, size, nmemb, t->fp);
        }
        break;
    case PHP_CURL_USER: {
        zval argv[3];
        zval retval;
        zend_result error;
        zend_fcall_info fci;

        GC_ADDREF(&ch->std);
        ZVAL_OBJ(&argv[0], &ch->std);
        if (t->res) {
            GC_ADDREF(t->res);
            ZVAL_RES(&argv[1], t->res);
        } else {
            ZVAL_NULL(&argv[1]);
        }
        ZVAL_LONG(&argv[2], (int) size * nmemb);

        fci.size = sizeof(fci);
        ZVAL_COPY_VALUE(&fci.function_name, &t->func_name);
        fci.object = NULL;
        fci.retval = &retval;
        fci.param_count = 3;
        fci.params = argv;
        fci.named_params = NULL;

        ch->in_callback = 1;
        error = zend_call_function(&fci, &t->fci_cache);
        ch->in_callback = 0;
        if (error == FAILURE) {
            php_error_docref(NULL, E_WARNING, "Cannot call the CURLOPT_READFUNCTION");
            length = CURL_READFUNC_ABORT;
        } else if (!Z_ISUNDEF(retval)) {
            swoole_curl_verify_handlers(ch, /* reporterror */ true);
            if (Z_TYPE(retval) == IS_STRING) {
                length = MIN((int) (size * nmemb), Z_STRLEN(retval));
                memcpy(data, Z_STRVAL(retval), length);
            } else if (Z_TYPE(retval) == IS_LONG) {
                length = Z_LVAL_P(&retval);
            }
            zval_ptr_dtor(&retval);
        }

        zval_ptr_dtor(&argv[0]);
        zval_ptr_dtor(&argv[1]);
        break;
    }
    }

    return length;
}
/* }}} */

/* {{{ curl_write_header
 */
static size_t fn_write_header(char *data, size_t size, size_t nmemb, void *ctx) {
    php_curl *ch = (php_curl *) ctx;
    php_curl_write *t = ch->handlers.write_header;
    size_t length = size * nmemb;

    switch (t->method) {
    case PHP_CURL_STDOUT:
        /* Handle special case write when we're returning the entire transfer
         */
        if (ch->handlers.write->method == PHP_CURL_RETURN && length > 0) {
            smart_str_appendl(&ch->handlers.write->buf, data, (int) length);
        } else {
            PHPWRITE(data, length);
        }
        break;
    case PHP_CURL_FILE:
        return fwrite(data, size, nmemb, t->fp);
    case PHP_CURL_USER: {
        zval argv[2];
        zval retval;
        zend_result error;
        zend_fcall_info fci;

        GC_ADDREF(&ch->std);
        ZVAL_OBJ(&argv[0], &ch->std);

        ZVAL_STRINGL(&argv[1], data, length);

        fci.size = sizeof(fci);
        ZVAL_COPY_VALUE(&fci.function_name, &t->func_name);
        fci.object = NULL;
        fci.retval = &retval;
        fci.param_count = 2;
        fci.params = argv;
        fci.named_params = NULL;

        ch->in_callback = 1;
        error = zend_call_function(&fci, &t->fci_cache);
        ch->in_callback = 0;
        if (error == FAILURE) {
            php_error_docref(NULL, E_WARNING, "Could not call the CURLOPT_HEADERFUNCTION");
            length = -1;
        } else if (!Z_ISUNDEF(retval)) {
            swoole_curl_verify_handlers(ch, /* reporterror */ true);
#if PHP_VERSION_ID >= 80300
            length = swoole_curl_get_long(&retval);
#else
            length = zval_get_long(&retval);
#endif
        }
        zval_ptr_dtor(&argv[0]);
        zval_ptr_dtor(&argv[1]);
        break;
    }

    case PHP_CURL_IGNORE:
        return length;

    default:
        return -1;
    }

    return length;
}
/* }}} */

static int curl_debug(CURL *cp, curl_infotype type, char *buf, size_t buf_len, void *ctx) /* {{{ */
{
    php_curl *ch = (php_curl *) ctx;

    if (type == CURLINFO_HEADER_OUT) {
        if (ch->header.str) {
            zend_string_release_ex(ch->header.str, 0);
        }
        ch->header.str = zend_string_init(buf, buf_len, 0);
    }

    return 0;
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

/* {{{ create_certinfo
 */
static void create_certinfo(struct curl_certinfo *ci, zval *listcode) {
    int i;

    if (ci) {
        zval certhash;

        for (i = 0; i < ci->num_of_certs; i++) {
            struct curl_slist *slist;

            array_init(&certhash);
            for (slist = ci->certinfo[i]; slist; slist = slist->next) {
                int len;
                char s[64];
                char *tmp;
                strncpy(s, slist->data, sizeof(s));
                s[sizeof(s) - 1] = '\0';
                tmp = (char *) memchr(s, ':', sizeof(s));
                if (tmp) {
                    *tmp = '\0';
                    len = strlen(s);
                    add_assoc_string(&certhash, s, &slist->data[len + 1]);
                } else {
                    php_error_docref(NULL, E_WARNING, "Could not extract hash key from certificate info");
                }
            }
            add_next_index_zval(listcode, &certhash);
        }
    }
}
/* }}} */

/* {{{ _php_curl_set_default_options()
   Set default options for a handle */
static void _php_curl_set_default_options(php_curl *ch) {
    const char *cainfo;

    curl_easy_setopt(ch->cp, CURLOPT_NOPROGRESS, 1L);
    curl_easy_setopt(ch->cp, CURLOPT_VERBOSE, 0L);
    curl_easy_setopt(ch->cp, CURLOPT_ERRORBUFFER, ch->err.str);
    curl_easy_setopt(ch->cp, CURLOPT_WRITEFUNCTION, fn_write);
    curl_easy_setopt(ch->cp, CURLOPT_FILE, (void *) ch);
    curl_easy_setopt(ch->cp, CURLOPT_READFUNCTION, fn_read);
    curl_easy_setopt(ch->cp, CURLOPT_INFILE, (void *) ch);
    curl_easy_setopt(ch->cp, CURLOPT_HEADERFUNCTION, fn_write_header);
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
zend_long swoole_curl_get_long(zval *zv) {
    if (EXPECTED(Z_TYPE_P(zv) == IS_LONG)) {
        return Z_LVAL_P(zv);
    } else {
        zend_long ret = zval_get_long(zv);
        zval_ptr_dtor(zv);
        return ret;
    }
}
#endif

/* The options crates/curl/src/setopt.rs leaves to C++: callables and CURLINFO_HEADER_OUT, and
 * the error for an unknown option. */
extern "C" zend_result swoole_curl_setopt_native(php_curl *ch, zend_long option, zval *zvalue, bool is_array_config) {
    CURLcode error = CURLE_OK;

    switch (option) {

    case CURLOPT_HEADERFUNCTION:
        if (!Z_ISUNDEF(ch->handlers.write_header->func_name)) {
            zval_ptr_dtor(&ch->handlers.write_header->func_name);
            ch->handlers.write_header->fci_cache = empty_fcall_info_cache;
        }
        ZVAL_COPY(&ch->handlers.write_header->func_name, zvalue);
        ch->handlers.write_header->method = PHP_CURL_USER;
        break;

    case CURLOPT_PROGRESSFUNCTION:
        curl_easy_setopt(ch->cp, CURLOPT_PROGRESSFUNCTION, fn_progress);
        curl_easy_setopt(ch->cp, CURLOPT_PROGRESSDATA, ch);
        if (ch->handlers.progress == NULL) {
            ch->handlers.progress = (php_curl_callback *) ecalloc(1, sizeof(php_curl_callback));
        } else if (!Z_ISUNDEF(ch->handlers.progress->func_name)) {
            zval_ptr_dtor(&ch->handlers.progress->func_name);
            ch->handlers.progress->fci_cache = empty_fcall_info_cache;
        }
        ZVAL_COPY(&ch->handlers.progress->func_name, zvalue);
        break;

#if LIBCURL_VERSION_NUM >= 0x075400 && PHP_VERSION_ID >= 80300
    case CURLOPT_SSH_HOSTKEYFUNCTION:
        curl_easy_setopt(ch->cp, CURLOPT_SSH_HOSTKEYFUNCTION, fn_ssh_hostkeyfunction);
        curl_easy_setopt(ch->cp, CURLOPT_SSH_HOSTKEYDATA, ch);
        if (ch->handlers.sshhostkey == NULL) {
            ch->handlers.sshhostkey = (php_curl_callback *) ecalloc(1, sizeof(php_curl_callback));
        } else if (!Z_ISUNDEF(ch->handlers.sshhostkey->func_name)) {
            zval_ptr_dtor(&ch->handlers.sshhostkey->func_name);
            ch->handlers.sshhostkey->fci_cache = empty_fcall_info_cache;
        }
        ZVAL_COPY(&ch->handlers.sshhostkey->func_name, zvalue);
        break;
#endif

    case CURLOPT_READFUNCTION:
        if (!Z_ISUNDEF(ch->handlers.read->func_name)) {
            zval_ptr_dtor(&ch->handlers.read->func_name);
            ch->handlers.read->fci_cache = empty_fcall_info_cache;
        }
        ZVAL_COPY(&ch->handlers.read->func_name, zvalue);
        ch->handlers.read->method = PHP_CURL_USER;
        break;

    case CURLOPT_WRITEFUNCTION:
        if (!Z_ISUNDEF(ch->handlers.write->func_name)) {
            zval_ptr_dtor(&ch->handlers.write->func_name);
            ch->handlers.write->fci_cache = empty_fcall_info_cache;
        }
        ZVAL_COPY(&ch->handlers.write->func_name, zvalue);
        ch->handlers.write->method = PHP_CURL_USER;
        break;

#if PHP_VERSION_ID >= 80200
    case CURLOPT_XFERINFOFUNCTION:
        curl_easy_setopt(ch->cp, CURLOPT_XFERINFOFUNCTION, fn_xferinfo);
        curl_easy_setopt(ch->cp, CURLOPT_XFERINFODATA, ch);
        if (ch->handlers.xferinfo == NULL) {
            ch->handlers.xferinfo = (php_curl_callback *) ecalloc(1, sizeof(php_curl_callback));
        } else if (!Z_ISUNDEF(ch->handlers.xferinfo->func_name)) {
            zval_ptr_dtor(&ch->handlers.xferinfo->func_name);
            ch->handlers.xferinfo->fci_cache = empty_fcall_info_cache;
        }
        ZVAL_COPY(&ch->handlers.xferinfo->func_name, zvalue);
        break;
#endif

    case CURLINFO_HEADER_OUT:
        if (zend_is_true(zvalue)) {
            curl_easy_setopt(ch->cp, CURLOPT_DEBUGFUNCTION, curl_debug);
            curl_easy_setopt(ch->cp, CURLOPT_DEBUGDATA, (void *) ch);
            curl_easy_setopt(ch->cp, CURLOPT_VERBOSE, 1L);
        } else {
            curl_easy_setopt(ch->cp, CURLOPT_DEBUGFUNCTION, NULL);
            curl_easy_setopt(ch->cp, CURLOPT_DEBUGDATA, NULL);
            curl_easy_setopt(ch->cp, CURLOPT_VERBOSE, 0L);
        }
        break;

    case CURLOPT_FNMATCH_FUNCTION:
        curl_easy_setopt(ch->cp, CURLOPT_FNMATCH_FUNCTION, fn_fnmatch);
        curl_easy_setopt(ch->cp, CURLOPT_FNMATCH_DATA, ch);
        if (ch->handlers.fnmatch == NULL) {
            ch->handlers.fnmatch = (php_curl_callback *) ecalloc(1, sizeof(php_curl_callback));
        } else if (!Z_ISUNDEF(ch->handlers.fnmatch->func_name)) {
            zval_ptr_dtor(&ch->handlers.fnmatch->func_name);
            ch->handlers.fnmatch->fci_cache = empty_fcall_info_cache;
        }
        ZVAL_COPY(&ch->handlers.fnmatch->func_name, zvalue);
        break;

    default:
        if (is_array_config) {
            zend_argument_value_error(2, "must contain only valid cURL options");
        } else {
            zend_argument_value_error(2, "is not a valid cURL option");
        }
        error = CURLE_UNKNOWN_OPTION;
        break;
    }

    SAVE_CURL_ERROR(ch, error);
    if (error != CURLE_OK) {
        return FAILURE;
    } else {
        return SUCCESS;
    }
}
/* }}} */

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

/* {{{ proto bool curl_exec(resource ch)
   Perform a cURL session */
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

    swoole_curl_verify_handlers(ch, /* reporterror */ true);
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

/* {{{ proto mixed curl_getinfo(resource ch [, int option])
   Get information regarding a specific transfer */
PHP_FUNCTION(swoole_native_curl_getinfo) {
    zval *zid;
    php_curl *ch;
    zend_long option;
    bool option_is_null = 1;

    ZEND_PARSE_PARAMETERS_START(1, 2)
    Z_PARAM_OBJECT_OF_CLASS(zid, swoole_coroutine_curl_handle_ce)
    Z_PARAM_OPTIONAL
    Z_PARAM_LONG_OR_NULL(option, option_is_null)

    ZEND_PARSE_PARAMETERS_END();

    if ((ch = swoole_curl_get_handle(zid, false)) == NULL) {
        RETURN_FALSE;
    }

    if (option_is_null) {
        char *s_code;
        /* libcurl expects long datatype. So far no cases are known where
           it would be an issue. Using zend_long would truncate a 64-bit
           var on Win64, so the exact long datatype fits everywhere, as
           long as there's no 32-bit int overflow. */
        long l_code;
        double d_code;
        struct curl_certinfo *ci = NULL;
        zval listcode;
#if LIBCURL_VERSION_NUM >= 0x073d00 /* 7.61.0 */
        curl_off_t co;
#endif

        array_init(return_value);

        if (curl_easy_getinfo(ch->cp, CURLINFO_EFFECTIVE_URL, &s_code) == CURLE_OK) {
            CAAS("url", s_code);
        }
        if (curl_easy_getinfo(ch->cp, CURLINFO_CONTENT_TYPE, &s_code) == CURLE_OK) {
            if (s_code != NULL) {
                CAAS("content_type", s_code);
            } else {
                zval retnull;
                ZVAL_NULL(&retnull);
                CAAZ("content_type", &retnull);
            }
        }
        if (curl_easy_getinfo(ch->cp, CURLINFO_HTTP_CODE, &l_code) == CURLE_OK) {
            CAAL("http_code", l_code);
        }
        if (curl_easy_getinfo(ch->cp, CURLINFO_HEADER_SIZE, &l_code) == CURLE_OK) {
            CAAL("header_size", l_code);
        }
        if (curl_easy_getinfo(ch->cp, CURLINFO_REQUEST_SIZE, &l_code) == CURLE_OK) {
            CAAL("request_size", l_code);
        }
        if (curl_easy_getinfo(ch->cp, CURLINFO_FILETIME, &l_code) == CURLE_OK) {
            CAAL("filetime", l_code);
        }
        if (curl_easy_getinfo(ch->cp, CURLINFO_SSL_VERIFYRESULT, &l_code) == CURLE_OK) {
            CAAL("ssl_verify_result", l_code);
        }
        if (curl_easy_getinfo(ch->cp, CURLINFO_REDIRECT_COUNT, &l_code) == CURLE_OK) {
            CAAL("redirect_count", l_code);
        }
        if (curl_easy_getinfo(ch->cp, CURLINFO_TOTAL_TIME, &d_code) == CURLE_OK) {
            CAAD("total_time", d_code);
        }
        if (curl_easy_getinfo(ch->cp, CURLINFO_NAMELOOKUP_TIME, &d_code) == CURLE_OK) {
            CAAD("namelookup_time", d_code);
        }
        if (curl_easy_getinfo(ch->cp, CURLINFO_CONNECT_TIME, &d_code) == CURLE_OK) {
            CAAD("connect_time", d_code);
        }
        if (curl_easy_getinfo(ch->cp, CURLINFO_PRETRANSFER_TIME, &d_code) == CURLE_OK) {
            CAAD("pretransfer_time", d_code);
        }
        if (curl_easy_getinfo(ch->cp, CURLINFO_SIZE_UPLOAD, &d_code) == CURLE_OK) {
            CAAD("size_upload", d_code);
        }
        if (curl_easy_getinfo(ch->cp, CURLINFO_SIZE_DOWNLOAD, &d_code) == CURLE_OK) {
            CAAD("size_download", d_code);
        }
        if (curl_easy_getinfo(ch->cp, CURLINFO_SPEED_DOWNLOAD, &d_code) == CURLE_OK) {
            CAAD("speed_download", d_code);
        }
        if (curl_easy_getinfo(ch->cp, CURLINFO_SPEED_UPLOAD, &d_code) == CURLE_OK) {
            CAAD("speed_upload", d_code);
        }
        if (curl_easy_getinfo(ch->cp, CURLINFO_CONTENT_LENGTH_DOWNLOAD, &d_code) == CURLE_OK) {
            CAAD("download_content_length", d_code);
        }
        if (curl_easy_getinfo(ch->cp, CURLINFO_CONTENT_LENGTH_UPLOAD, &d_code) == CURLE_OK) {
            CAAD("upload_content_length", d_code);
        }
        if (curl_easy_getinfo(ch->cp, CURLINFO_STARTTRANSFER_TIME, &d_code) == CURLE_OK) {
            CAAD("starttransfer_time", d_code);
        }
        if (curl_easy_getinfo(ch->cp, CURLINFO_REDIRECT_TIME, &d_code) == CURLE_OK) {
            CAAD("redirect_time", d_code);
        }
        if (curl_easy_getinfo(ch->cp, CURLINFO_REDIRECT_URL, &s_code) == CURLE_OK) {
            CAAS("redirect_url", s_code);
        }
        if (curl_easy_getinfo(ch->cp, CURLINFO_PRIMARY_IP, &s_code) == CURLE_OK) {
            CAAS("primary_ip", s_code);
        }
        if (curl_easy_getinfo(ch->cp, CURLINFO_CERTINFO, &ci) == CURLE_OK) {
            array_init(&listcode);
            create_certinfo(ci, &listcode);
            CAAZ("certinfo", &listcode);
        }
        if (curl_easy_getinfo(ch->cp, CURLINFO_PRIMARY_PORT, &l_code) == CURLE_OK) {
            CAAL("primary_port", l_code);
        }
        if (curl_easy_getinfo(ch->cp, CURLINFO_LOCAL_IP, &s_code) == CURLE_OK) {
            CAAS("local_ip", s_code);
        }
        if (curl_easy_getinfo(ch->cp, CURLINFO_LOCAL_PORT, &l_code) == CURLE_OK) {
            CAAL("local_port", l_code);
        }
        if (curl_easy_getinfo(ch->cp, CURLINFO_HTTP_VERSION, &l_code) == CURLE_OK) {
            CAAL("http_version", l_code);
        }
        if (curl_easy_getinfo(ch->cp, CURLINFO_PROTOCOL, &l_code) == CURLE_OK) {
            CAAL("protocol", l_code);
        }
        if (curl_easy_getinfo(ch->cp, CURLINFO_PROXY_SSL_VERIFYRESULT, &l_code) == CURLE_OK) {
            CAAL("ssl_verifyresult", l_code);
        }
        if (curl_easy_getinfo(ch->cp, CURLINFO_SCHEME, &s_code) == CURLE_OK) {
            CAAS("scheme", s_code);
        }
#if LIBCURL_VERSION_NUM >= 0x073d00 /* Available since 7.61.0 */
        if (curl_easy_getinfo(ch->cp, CURLINFO_APPCONNECT_TIME_T, &co) == CURLE_OK) {
            CAAL("appconnect_time_us", co);
        }
        if (curl_easy_getinfo(ch->cp, CURLINFO_CONNECT_TIME_T, &co) == CURLE_OK) {
            CAAL("connect_time_us", co);
        }
        if (curl_easy_getinfo(ch->cp, CURLINFO_NAMELOOKUP_TIME_T, &co) == CURLE_OK) {
            CAAL("namelookup_time_us", co);
        }
        if (curl_easy_getinfo(ch->cp, CURLINFO_PRETRANSFER_TIME_T, &co) == CURLE_OK) {
            CAAL("pretransfer_time_us", co);
        }
        if (curl_easy_getinfo(ch->cp, CURLINFO_REDIRECT_TIME_T, &co) == CURLE_OK) {
            CAAL("redirect_time_us", co);
        }
        if (curl_easy_getinfo(ch->cp, CURLINFO_STARTTRANSFER_TIME_T, &co) == CURLE_OK) {
            CAAL("starttransfer_time_us", co);
        }
        if (curl_easy_getinfo(ch->cp, CURLINFO_TOTAL_TIME_T, &co) == CURLE_OK) {
            CAAL("total_time_us", co);
        }
#endif
        if (ch->header.str) {
            CAASTR("request_header", ch->header.str);
        }
#if LIBCURL_VERSION_NUM >= 0x074800 /* Available since 7.72.0 */
        if (curl_easy_getinfo(ch->cp, CURLINFO_EFFECTIVE_METHOD, &s_code) == CURLE_OK) {
            CAAS("effective_method", s_code);
        }
#endif
#if LIBCURL_VERSION_NUM >= 0x075400 && PHP_VERSION_ID >= 80300
        if (curl_easy_getinfo(ch->cp, CURLINFO_CAPATH, &s_code) == CURLE_OK) {
            CAAS("capath", s_code);
        }
        if (curl_easy_getinfo(ch->cp, CURLINFO_CAINFO, &s_code) == CURLE_OK) {
            CAAS("cainfo", s_code);
        }
#endif
    } else {
        switch (option) {
        case CURLINFO_HEADER_OUT:
            if (ch->header.str) {
                RETURN_STR_COPY(ch->header.str);
            } else {
                RETURN_FALSE;
            }
        case CURLINFO_CERTINFO: {
            struct curl_certinfo *ci = NULL;

            array_init(return_value);

            if (curl_easy_getinfo(ch->cp, CURLINFO_CERTINFO, &ci) == CURLE_OK) {
                create_certinfo(ci, return_value);
            } else {
                RETURN_FALSE;
            }
            break;
        }
        case CURLINFO_PRIVATE: {
            if (!Z_ISUNDEF(ch->private_data)) {
                RETURN_COPY(&ch->private_data);
            } else {
                RETURN_FALSE;
            }
            break;
        }
        default: {
            int type = CURLINFO_TYPEMASK & option;
            switch (type) {
            case CURLINFO_STRING: {
                char *s_code = NULL;

                if (curl_easy_getinfo(ch->cp, (CURLINFO) option, &s_code) == CURLE_OK && s_code) {
                    RETURN_STRING(s_code);
                } else {
                    RETURN_FALSE;
                }
                break;
            }
            case CURLINFO_LONG: {
                zend_long code = 0;

                if (curl_easy_getinfo(ch->cp, (CURLINFO) option, &code) == CURLE_OK) {
                    RETURN_LONG(code);
                } else {
                    RETURN_FALSE;
                }
                break;
            }
            case CURLINFO_DOUBLE: {
                double code = 0.0;

                if (curl_easy_getinfo(ch->cp, (CURLINFO) option, &code) == CURLE_OK) {
                    RETURN_DOUBLE(code);
                } else {
                    RETURN_FALSE;
                }
                break;
            }
            case CURLINFO_SLIST: {
                struct curl_slist *slist;
                if (curl_easy_getinfo(ch->cp, (CURLINFO) option, &slist) == CURLE_OK) {
                    struct curl_slist *current = slist;
                    array_init(return_value);
                    while (current) {
                        add_next_index_string(return_value, current->data);
                        current = current->next;
                    }
                    curl_slist_free_all(slist);
                } else {
                    RETURN_FALSE;
                }
                break;
            }
            case CURLINFO_OFF_T: {
                curl_off_t c_off;
                if (curl_easy_getinfo(ch->cp, (CURLINFO) option, &c_off) == CURLE_OK) {
                    RETURN_LONG((long) c_off);
                } else {
                    RETURN_FALSE;
                }
                break;
            }
            default:
                RETURN_FALSE;
            }
        }
        }
    }
}
/* }}} */

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
