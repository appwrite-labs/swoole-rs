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

#if defined(SW_USE_CURL) && PHP_VERSION_ID >= 80400

#define ZEND_INCLUDE_FULL_WINDOWS_HEADERS
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

SW_EXTERN_C_END

static int _php_server_push_callback(
    CURL *parent_ch, CURL *easy, size_t num_headers, struct curl_pushheaders *push_headers, void *userp) /* {{{ */
{
    php_curl *ch;
    php_curl *parent;
    php_curlm *mh = (php_curlm *) userp;
    int rval = CURL_PUSH_DENY;
    zval *pz_parent_ch = NULL;
    zval pz_ch;
    zval headers;
    zval retval;

    pz_parent_ch = swoole_curl_multi_find_easy(mh, parent_ch);
    if (pz_parent_ch == NULL) {
        return rval;
    }

    parent = Z_CURL_P(pz_parent_ch);

    ch = swoole_curl_init_handle_into_zval(&pz_ch);
    ch->cp = easy;
    swoole_setup_easy_copy_handlers(ch, parent);

    swr_v1_curl_push_handle(easy, parent->cp);

#if PHP_VERSION_ID >= 80500
    array_init_size(&headers, num_headers);
    zend_hash_real_init_packed(Z_ARRVAL(headers));
    for (size_t i = 0; i < num_headers; i++) {
        char *header = curl_pushheader_bynum(push_headers, i);
        add_index_string(&headers, i, header);
    }
#else
    array_init(&headers);
    for (size_t i = 0; i < num_headers; i++) {
        char *header = curl_pushheader_bynum(push_headers, i);
        add_next_index_string(&headers, header);
    }
#endif

    ZEND_ASSERT(pz_parent_ch);
    zval call_args[3] = {*pz_parent_ch, pz_ch, headers};

    zend_call_known_fcc(&mh->handlers.server_push, &retval, /* param_count */ 3, call_args, /* named_params */ NULL);
    zval_ptr_dtor_nogc(&headers);

    if (!Z_ISUNDEF(retval)) {
        if (CURL_PUSH_DENY != swoole_curl_get_long(&retval)) {
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
#if LIBCURL_VERSION_NUM >= 0x074300 /* Available since 7.67.0 */
    case CURLMOPT_MAX_CONCURRENT_STREAMS:
#endif
    {
        zend_long lval = zval_get_long(zvalue);

        if (option == CURLMOPT_PIPELINING && (lval & 1)) {
#if LIBCURL_VERSION_NUM >= 0x073e00 /* Available since 7.62.0 */
            php_error_docref(NULL, E_WARNING, "CURLPIPE_HTTP1 is no longer supported");
#else
            php_error_docref(NULL, E_DEPRECATED, "CURLPIPE_HTTP1 is deprecated");
#endif
        }
        error = curl_multi_setopt((CURLM *) swr_v1_curl_multi_handle(mh->multi), (CURLMoption) option, lval);
        break;
    }
    case CURLMOPT_PUSHFUNCTION: {
        /* See php_curl_set_callable_handler */
        if (ZEND_FCC_INITIALIZED(mh->handlers.server_push)) {
            zend_fcc_dtor(&mh->handlers.server_push);
        }

        char *error_str = NULL;
        if (UNEXPECTED(!zend_is_callable_ex(zvalue,
                                            /* object */ NULL,
                                            /* check_flags */ 0,
                                            /* callable_name */ NULL,
                                            &mh->handlers.server_push,
                                            /* error */ &error_str))) {
            if (!EG(exception)) {
                zend_argument_type_error(2, "must be a valid callback for option CURLMOPT_PUSHFUNCTION, %s", error_str);
            }
            efree(error_str);
            return false;
        }
        zend_fcc_addref(&mh->handlers.server_push);

        error = curl_multi_setopt((CURLM *) swr_v1_curl_multi_handle(mh->multi), CURLMOPT_PUSHFUNCTION, _php_server_push_callback);
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

#endif
