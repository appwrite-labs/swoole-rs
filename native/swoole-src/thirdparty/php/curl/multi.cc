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

extern "C" int swoole_curl_multi_on_push(
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

#endif
