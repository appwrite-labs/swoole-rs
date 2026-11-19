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
#include "zend_object_handlers.h"

#if defined(SW_USE_CURL) && PHP_VERSION_ID < 80400
#include "swoole_rs_curl.h"

SW_EXTERN_C_BEGIN
#include "curl_arginfo.h"

#include <stdio.h>
#include <string.h>

#include <curl/curl.h>
#include <curl/easy.h>

#define SAVE_CURLM_ERROR(__handle, __err) (__handle)->err.no = (int) __err;

extern "C" bool swoole_curl_multi_is_in_coroutine(php_curlm *mh);

/* CurlMultiHandle class */

static inline php_curlm *curl_multi_from_obj(zend_object *obj) {
    return (php_curlm *) ((char *) (obj) -XtOffsetOf(php_curlm, std));
}
#define Z_CURL_MULTI_P(zv) curl_multi_from_obj(Z_OBJ_P(zv))

static void _php_curl_multi_free(php_curlm *mh);

SW_EXTERN_C_END

/* {{{ Close a set of cURL handles */
PHP_FUNCTION(swoole_native_curl_multi_close) {
    php_curlm *mh;
    zval *z_mh;

    zend_llist_position pos;
    zval *pz_ch;

    ZEND_PARSE_PARAMETERS_START(1, 1)
    Z_PARAM_OBJECT_OF_CLASS(z_mh, swoole_coroutine_curl_multi_handle_ce)
    ZEND_PARSE_PARAMETERS_END();

    mh = Z_CURL_MULTI_P(z_mh);

    for (pz_ch = (zval *) zend_llist_get_first_ex(&mh->easyh, &pos); pz_ch;
         pz_ch = (zval *) zend_llist_get_next_ex(&mh->easyh, &pos)) {
        php_curl *ch = Z_CURL_P(pz_ch);
        if (!ch) {
            continue;
        }
        swoole_curl_verify_handlers(ch, /* reporterror */ false);
        auto handle = swr_v1_curl_get_handle(ch->cp);
        if (handle) {
            swr_v1_curl_multi_remove_handle(mh->multi, handle);
        } else {
            curl_multi_remove_handle(mh->multi, ch->cp);
        }
    }
    zend_llist_clean(&mh->easyh);
}
/* }}} */



static int _php_server_push_callback(
    CURL *parent_ch, CURL *easy, size_t num_headers, struct curl_pushheaders *push_headers, void *userp) /* {{{ */
{
    php_curl *ch;
    php_curl *parent;
    php_curlm *mh = (php_curlm *) userp;
    size_t rval = CURL_PUSH_DENY;
    php_curl_callback *t = mh->handlers.server_push;
    zval *pz_parent_ch = NULL;
    zval pz_ch;
    zval headers;
    zval retval;
    char *header;
    zend_result error;

    zend_fcall_info fci = empty_fcall_info;

    pz_parent_ch = swoole_curl_multi_find_easy(mh, parent_ch);
    if (pz_parent_ch == NULL) {
        return rval;
    }

    if (UNEXPECTED(zend_fcall_info_init(&t->func_name, 0, &fci, &t->fci_cache, NULL, NULL) == FAILURE)) {
        php_error_docref(NULL, E_WARNING, "Cannot call the CURLMOPT_PUSHFUNCTION");
        return rval;
    }

    parent = Z_CURL_P(pz_parent_ch);

    ch = swoole_curl_init_handle_into_zval(&pz_ch);
    ch->cp = easy;
    swoole_setup_easy_copy_handlers(ch, parent);

    swr_v1_curl_push_handle(easy, parent->cp);

    size_t i;
    array_init(&headers);
    for (i = 0; i < num_headers; i++) {
        header = curl_pushheader_bynum(push_headers, i);
        add_next_index_string(&headers, header);
    }

#if PHP_VERSION_ID >= 80300
    ZEND_ASSERT(pz_parent_ch);
    zval call_args[3] = {*pz_parent_ch, pz_ch, headers};

    fci.param_count = 3;
    fci.params = call_args;
    fci.retval = &retval;
#else
    zend_fcall_info_argn(&fci, 3, pz_parent_ch, &pz_ch, &headers);

    fci.retval = &retval;
#endif

    error = zend_call_function(&fci, &t->fci_cache);
#if PHP_VERSION_ID < 80300
    zend_fcall_info_args_clear(&fci, 1);
#endif
    zval_ptr_dtor_nogc(&headers);

    if (error == FAILURE) {
        php_error_docref(NULL, E_WARNING, "Cannot call the CURLMOPT_PUSHFUNCTION");
    } else if (!Z_ISUNDEF(retval)) {
#if PHP_VERSION_ID >= 80300
        if (CURL_PUSH_DENY != swoole_curl_get_long(&retval)) {
#else
        if (CURL_PUSH_DENY != zval_get_long(&retval)) {
#endif
            rval = CURL_PUSH_OK;
            zend_llist_add_element(&mh->easyh, &pz_ch);
        } else {
            /* libcurl will free this easy handle, avoid double free */
            ch->cp = NULL;
        }
    }

    return rval;
}
/* }}} */

static bool _php_curl_multi_setopt(php_curlm *mh, zend_long option, zval *zvalue, zval *return_value) /* {{{ */
{
    CURLMcode error = CURLM_OK;

    switch (option) {
    case CURLMOPT_PIPELINING:
    case CURLMOPT_MAXCONNECTS:
    case CURLMOPT_CHUNK_LENGTH_PENALTY_SIZE:
    case CURLMOPT_CONTENT_LENGTH_PENALTY_SIZE:
    case CURLMOPT_MAX_HOST_CONNECTIONS:
    case CURLMOPT_MAX_PIPELINE_LENGTH:
    case CURLMOPT_MAX_TOTAL_CONNECTIONS:
#if LIBCURL_VERSION_NUM >= 0x074300 && PHP_VERSION_ID >= 80200
    case CURLMOPT_MAX_CONCURRENT_STREAMS:
#endif
    {
        zend_long lval = zval_get_long(zvalue);

        if (option == CURLMOPT_PIPELINING && (lval & 1)) {
#if LIBCURL_VERSION_NUM >= 0x073e00 /* 7.62.0 */
            php_error_docref(NULL, E_WARNING, "CURLPIPE_HTTP1 is no longer supported");
#else
            php_error_docref(NULL, E_DEPRECATED, "CURLPIPE_HTTP1 is deprecated");
#endif
        }
        error = curl_multi_setopt((CURLM *) swr_v1_curl_multi_handle(mh->multi), (CURLMoption) option, lval);
        break;
    }
    case CURLMOPT_PUSHFUNCTION: {
        if (mh->handlers.server_push == NULL) {
            mh->handlers.server_push = (php_curl_callback *) ecalloc(1, sizeof(php_curl_callback));
        } else if (!Z_ISUNDEF(mh->handlers.server_push->func_name)) {
            zval_ptr_dtor(&mh->handlers.server_push->func_name);
            mh->handlers.server_push->fci_cache = empty_fcall_info_cache;
        }

        ZVAL_COPY(&mh->handlers.server_push->func_name, zvalue);
#if PHP_VERSION_ID >= 80200
        error = curl_multi_setopt((CURLM *) swr_v1_curl_multi_handle(mh->multi), CURLMOPT_PUSHFUNCTION, _php_server_push_callback);
#else
        error = curl_multi_setopt((CURLM *) swr_v1_curl_multi_handle(mh->multi), (CURLMoption) option, _php_server_push_callback);
#endif
        if (error != CURLM_OK) {
            return false;
        }
        error = curl_multi_setopt((CURLM *) swr_v1_curl_multi_handle(mh->multi), CURLMOPT_PUSHDATA, mh);
        break;
    }
    default:
        zend_argument_value_error(2, "is not a valid cURL multi option");
        error = CURLM_UNKNOWN_OPTION;
        break;
    }

    SAVE_CURLM_ERROR(mh, error);

    return error == CURLM_OK;
}
/* }}} */

/* {{{ Set an option for the curl multi handle */
PHP_FUNCTION(swoole_native_curl_multi_setopt) {
    zval *z_mh, *zvalue;
    zend_long options;
    php_curlm *mh;

    ZEND_PARSE_PARAMETERS_START(3, 3)
    Z_PARAM_OBJECT_OF_CLASS(z_mh, swoole_coroutine_curl_multi_handle_ce)
    Z_PARAM_LONG(options)
    Z_PARAM_ZVAL(zvalue)
    ZEND_PARSE_PARAMETERS_END();

    mh = Z_CURL_MULTI_P(z_mh);
    if (!(swoole_curl_multi_is_in_coroutine(mh))) {
        swoole_fatal_error(SW_ERROR_WRONG_OPERATION,
                           "The given object is not a valid coroutine CurlMultiHandle object");
        RETURN_FALSE;
    }
    if (_php_curl_multi_setopt(mh, (CURLMoption) options, zvalue, return_value)) {
        RETURN_TRUE;
    } else {
        RETURN_FALSE;
    }
}
/* }}} */

/* CurlMultiHandle class */

extern "C" void swoole_curl_multi_free_obj(zend_object *object) {
    php_curlm *mh = (php_curlm *) curl_multi_from_obj(object);
    if (!mh->multi) {
        /* Can happen if constructor throws. */
        zend_object_std_dtor(&mh->std);
        return;
    }
    _php_curl_multi_free(mh);
    zend_object_std_dtor(&mh->std);
}

static void _php_curl_multi_free(php_curlm *mh) {
    bool is_in_coroutine = swoole_curl_multi_is_in_coroutine(mh);
    for (zend_llist_element *element = mh->easyh.head; element; element = element->next) {
        zval *z_ch = (zval *) element->data;
        php_curl *ch;
        if (OBJ_FLAGS(Z_OBJ_P(z_ch)) & IS_OBJ_FREE_CALLED) {
            continue;
        }
        if ((ch = swoole_curl_get_handle(z_ch, true, false))) {
            swoole_curl_verify_handlers(ch, /* reporterror */ false);
            auto handle = swr_v1_curl_get_handle(ch->cp);
            if (is_in_coroutine && handle) {
                swr_v1_curl_multi_remove_handle(mh->multi, handle);
            } else {
                curl_multi_remove_handle(mh->multi, ch->cp);
            }
        }
    }
    if (mh->multi) {
        if (is_in_coroutine) {
            swr_v1_curl_multi_free(mh->multi);
        } else {
            curl_multi_cleanup(mh->multi);
        }
        mh->multi = nullptr;
    }
    zend_llist_clean(&mh->easyh);
    if (mh->handlers.server_push) {
        zval_ptr_dtor(&mh->handlers.server_push->func_name);
        efree(mh->handlers.server_push);
    }
}

#endif
