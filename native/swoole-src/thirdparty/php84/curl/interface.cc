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


#include <stdio.h>
#include <string.h>

#ifdef PHP_WIN32
#include <winsock2.h>
#include <sys/types.h>
#endif

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

#endif
