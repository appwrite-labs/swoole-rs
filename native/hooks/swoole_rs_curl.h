#ifndef SWOOLE_RS_CURL_H
#define SWOOLE_RS_CURL_H
/* The coroutine cURL multi engine (Rust, crates/curl, --enable-swoole-curl),
 * which the bundled php_curl (thirdparty/php{,84}/curl) calls directly. */

#ifdef SW_USE_CURL
#include <stdbool.h>
#include <curl/curl.h>
#include <curl/multi.h>
#if LIBCURL_VERSION_NUM < 0x073800
#error "require cURL version 7.56.0 or later"
#endif
#ifdef __cplusplus
extern "C" {
#if PHP_VERSION_ID >= 80400
#include "thirdparty/php84/curl/curl_private.h"
#else
#include "thirdparty/php/curl/curl_private.h"
#endif
/* The handle ch on behalf of the current coroutine, or null after the error of
 * a missing coroutine handle or of a multi bound to another coroutine. */
php_curl *swoole_curl_get_handle(zval *zid, bool exclusive = true, bool required = true);
#endif
struct swr_v1_curl_multi;
/* Rust's: the easy handle's multi while it is in one, and the multi curl_exec
 * runs it in. */
struct swr_v1_curl_handle {
    CURL *cp;
    struct swr_v1_curl_multi *multi;
    struct swr_v1_curl_multi *easy_multi;
};
void swr_v1_curl_multi_free(struct swr_v1_curl_multi *multi);
void *swr_v1_curl_multi_handle(const struct swr_v1_curl_multi *multi);
int swr_v1_curl_multi_running(const struct swr_v1_curl_multi *multi);
int swr_v1_curl_multi_add_handle(const struct swr_v1_curl_multi *multi, struct swr_v1_curl_handle *handle);
int swr_v1_curl_multi_remove_handle(const struct swr_v1_curl_multi *multi, struct swr_v1_curl_handle *handle);
/* After swoole_curl_check_bound_co. */
int swr_v1_curl_multi_perform(const struct swr_v1_curl_multi *multi);
long swr_v1_curl_multi_select(const struct swr_v1_curl_multi *multi, void *co, double timeout);
/* A live easy handle (a CURL *), as Rust's Easy takes it. */
typedef void *swr_v1_curl_easy;
struct swr_v1_curl_handle *swr_v1_curl_get_handle(swr_v1_curl_easy cp);
struct swr_v1_curl_handle *swr_v1_curl_create_handle(swr_v1_curl_easy cp);
/* No transfer of cp runs, and nothing uses its handle afterwards. */
void swr_v1_curl_destroy_handle(swr_v1_curl_easy cp);
/* Curl's FILE* may be a PHP stream cookie, so fread/fwrite must not run in an AIO thread. */
void swoole_curl_easy_reset(CURL *curl);
#define curl_easy_reset swoole_curl_easy_reset

struct swr_v1_curl_multi *swoole_curl_multi_new(void);
/* The current coroutine, or null after the fatal error of a multi another
 * coroutine waits on. */
void *swoole_curl_check_bound_co(const struct swr_v1_curl_multi *multi);
CURLcode swoole_curl_easy_perform(CURL *cp);

/* The bundled php_curl's. */
void swoole_native_curl_minit(int module_number);
void swoole_native_curl_mshutdown(void);
#ifdef PHP_FUNCTION
PHP_FUNCTION(swoole_native_curl_close);
PHP_FUNCTION(swoole_native_curl_copy_handle);
PHP_FUNCTION(swoole_native_curl_errno);
PHP_FUNCTION(swoole_native_curl_error);
PHP_FUNCTION(swoole_native_curl_exec);
PHP_FUNCTION(swoole_native_curl_getinfo);
PHP_FUNCTION(swoole_native_curl_init);
PHP_FUNCTION(swoole_native_curl_setopt);
PHP_FUNCTION(swoole_native_curl_setopt_array);
PHP_FUNCTION(swoole_native_curl_reset);
PHP_FUNCTION(swoole_native_curl_escape);
PHP_FUNCTION(swoole_native_curl_unescape);
PHP_FUNCTION(swoole_native_curl_pause);
PHP_FUNCTION(swoole_native_curl_multi_add_handle);
PHP_FUNCTION(swoole_native_curl_multi_close);
PHP_FUNCTION(swoole_native_curl_multi_errno);
PHP_FUNCTION(swoole_native_curl_multi_exec);
PHP_FUNCTION(swoole_native_curl_multi_select);
PHP_FUNCTION(swoole_native_curl_multi_remove_handle);
PHP_FUNCTION(swoole_native_curl_multi_setopt);
PHP_FUNCTION(swoole_native_curl_multi_getcontent);
PHP_FUNCTION(swoole_native_curl_multi_info_read);
PHP_FUNCTION(swoole_native_curl_multi_init);
#if LIBCURL_VERSION_NUM >= 0x073E00
PHP_FUNCTION(swoole_native_curl_upkeep);
#endif
#endif
#ifdef __cplusplus
}
#endif
#endif
#endif
