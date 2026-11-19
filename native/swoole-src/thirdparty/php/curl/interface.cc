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


#include <stdio.h>
#include <string.h>

#include <curl/curl.h>
#include <curl/easy.h>

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

SW_EXTERN_C_END


/* {{{ curl_write_nothing
 * Used as a work around. See _php_curl_close_ex
 */
static size_t fn_write_nothing(char *data, size_t size, size_t nmemb, void *ctx) {
    return size * nmemb;
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

#endif
