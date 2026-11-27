#ifndef SWOOLE_RS_CURL_H
#define SWOOLE_RS_CURL_H
/* Native ABI for the Rust coroutine cURL engine. */

#ifdef SW_USE_CURL
#include <stdbool.h>
#include <curl/curl.h>
#include <curl/multi.h>
#if LIBCURL_VERSION_NUM < 0x073800
#error "require cURL version 7.56.0 or later"
#endif
#ifdef __cplusplus
extern "C" {
#include "swoole_rs_curl_layout.h"
#endif
/* A live easy handle (a CURL *), as Rust's Easy takes it. */
typedef void *swr_v1_curl_easy;
#ifdef __cplusplus
void swoole_curl_register_handle(void);
void swoole_curl_verify_handlers(php_curl *ch, bool reporterror);
zend_result swoole_curl_cast_object(zend_object *obj, zval *result, int type);
zend_object *swoole_curl_create_object(zend_class_entry *ce);
void swoole_curl_free_obj(zend_object *object);
HashTable *swoole_curl_get_gc(zend_object *object, zval **table, int *n);
zend_object *swoole_curl_clone_obj(zend_object *object);
zend_function *swoole_curl_get_constructor(zend_object *object);
/* Whether the current coroutine may use the php_curl whose easy handle is cp:
 * false after the error of a missing coroutine handle or of a multi bound to
 * another coroutine. */
bool swr_v1_curl_check_handle(swr_v1_curl_easy cp, bool exclusive, bool required);
/* libcurl's callbacks, Rust's (crates/curl/src/callbacks.rs): each has the php_curl as its data. */
size_t swoole_curl_on_write(char *data, size_t size, size_t nmemb, void *ctx);
size_t swoole_curl_on_header(char *data, size_t size, size_t nmemb, void *ctx);
size_t swoole_curl_on_read(char *data, size_t size, size_t nmemb, void *ctx);
int swoole_curl_on_progress(void *ctx, double dltotal, double dlnow, double ultotal, double ulnow);
int swoole_curl_on_xferinfo(void *ctx, curl_off_t dltotal, curl_off_t dlnow, curl_off_t ultotal, curl_off_t ulnow);
int swoole_curl_on_fnmatch(void *ctx, const char *pattern, const char *string);
int swoole_curl_on_debug(CURL *cp, curl_infotype type, char *data, size_t size, void *ctx);
int swoole_curl_on_prereq(void *ctx, char *primary_ip, char *local_ip, int primary_port, int local_port);
int swoole_curl_on_ssh_hostkey(void *ctx, int keytype, const char *key, size_t keylen);
#endif
struct swr_v1_curl_multi;
/* Rust's: the easy handle's multi while it is in one, and the multi curl_exec
 * runs it in. */
struct swr_v1_curl_handle {
    CURL *cp;
    void *multi;      /* Rust-owned (a Weak), which C only null-checks */
    void *easy_multi; /* Rust-owned (an Rc), opaque to C */
};
/* A multi the caller gives up (or null), freed by swr_v1_curl_multi_free. */
typedef struct swr_v1_curl_multi *swr_v1_curl_released;
void swr_v1_curl_multi_free(swr_v1_curl_released multi);
struct swr_v1_curl_handle *swr_v1_curl_get_handle(swr_v1_curl_easy cp);
struct swr_v1_curl_handle *swr_v1_curl_create_handle(swr_v1_curl_easy cp);
/* The handle leaves the multi it is in, while that multi lives. */
void swr_v1_curl_handle_leave_multi(const struct swr_v1_curl_handle *handle);
/* No transfer of cp runs, and nothing uses its handle afterwards. */
void swr_v1_curl_destroy_handle(swr_v1_curl_easy cp);
/* Curl's FILE* may be a PHP stream cookie, so fread/fwrite must not run in an AIO thread. */
void swoole_curl_easy_reset(CURL *curl);
#define curl_easy_reset swoole_curl_easy_reset

/* The current coroutine, or null after the fatal error of a multi another
 * coroutine waits on. */
void *swoole_curl_check_bound_co(const struct swr_v1_curl_multi *multi);
CURLcode swoole_curl_easy_perform(CURL *cp);

#ifdef __cplusplus
}
#endif
#endif
#endif
