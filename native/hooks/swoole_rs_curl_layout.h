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
   |         Wez Furlong <wez@thebrainroom.com>                           |
   +----------------------------------------------------------------------+
*/

#ifndef SWOOLE_RS_CURL_LAYOUT_H
#define SWOOLE_RS_CURL_LAYOUT_H

/* ext/curl installs its class declarations, but not its object layouts.
 * These prefixes must match the PHP and libcurl headers used to build it. */
#include "ext/curl/php_curl.h"
#include "zend_smart_str.h"

#define PHP_CURL_STDOUT 0
#define PHP_CURL_FILE 1
#define PHP_CURL_DIRECT 3
#define PHP_CURL_IGNORE 7

#if PHP_VERSION_ID < 80400
typedef struct {
    zval func_name;
    zend_fcall_info_cache fci_cache;
} php_curl_callback;
using php_curl_callback_slot = php_curl_callback *;
#else
using php_curl_callback_slot = zend_fcall_info_cache;
#endif

typedef struct {
#if PHP_VERSION_ID >= 80400
    zend_fcall_info_cache fcc;
#else
    zval func_name;
    zend_fcall_info_cache fci_cache;
#endif
    FILE *fp;
    smart_str buf;
    int method;
    zval stream;
} php_curl_write;

typedef struct {
#if PHP_VERSION_ID >= 80400
    zend_fcall_info_cache fcc;
#else
    zval func_name;
    zend_fcall_info_cache fci_cache;
#endif
    FILE *fp;
    zend_resource *res;
    int method;
    zval stream;
} php_curl_read;

typedef struct {
    php_curl_write *write;
    php_curl_write *write_header;
    php_curl_read *read;
    zval std_err;
    php_curl_callback_slot progress;
    php_curl_callback_slot xferinfo;
    php_curl_callback_slot fnmatch;
#if PHP_VERSION_ID >= 80400
    php_curl_callback_slot debug;
#if LIBCURL_VERSION_NUM >= 0x075000 /* Available since 7.80.0 */
    php_curl_callback_slot prereq;
#endif
#endif
#if PHP_VERSION_ID >= 80300 && LIBCURL_VERSION_NUM >= 0x075400
    php_curl_callback_slot sshhostkey;
#endif
} php_curl_handlers;

struct _php_curl_error {
    char str[CURL_ERROR_SIZE + 1];
    int no;
};

struct _php_curl_send_headers {
    zend_string *str;
};

struct _php_curl_free {
    zend_llist post;
    zend_llist stream;
#if PHP_VERSION_ID >= 80500
    HashTable slist;
#else
    HashTable *slist;
#endif
};

typedef struct {
    CURL *cp;
    php_curl_handlers handlers;
    struct _php_curl_free *to_free;
    struct _php_curl_send_headers header;
    struct _php_curl_error err;
    bool in_callback;
    uint32_t *clone;
    zval postfields;
    /* For CURLOPT_PRIVATE */
    zval private_data;
    /* CurlShareHandle object set using CURLOPT_SHARE. */
    struct _php_curlsh *share;
    zend_object std;
} php_curl;

typedef struct {
    php_curl_callback_slot server_push;
} php_curlm_handlers;

struct swr_v1_curl_multi;

typedef struct {
    struct swr_v1_curl_multi *multi;
    zend_llist easyh;
    php_curlm_handlers handlers;
    struct {
        int no;
    } err;
    zend_object std;
} php_curlm;

typedef struct _php_curlsh {
    CURLSH *share;
    struct {
        int no;
    } err;
    zend_object std;
} php_curlsh;

static inline php_curl *curl_from_obj(zend_object *obj) {
    return (php_curl *) ((char *) (obj) -XtOffsetOf(php_curl, std));
}

#define Z_CURL_P(zv) curl_from_obj(Z_OBJ_P(zv))

static inline php_curlsh *curl_share_from_obj(zend_object *obj) {
    return (php_curlsh *) ((char *) (obj) -XtOffsetOf(php_curlsh, std));
}

#define Z_CURL_SHARE_P(zv) curl_share_from_obj(Z_OBJ_P(zv))

#endif
