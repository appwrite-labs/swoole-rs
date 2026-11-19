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
}

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
