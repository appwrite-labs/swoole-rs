#ifndef SWOOLE_RS_CURL_H
#define SWOOLE_RS_CURL_H
#include <stdbool.h>
#ifdef __cplusplus
extern "C" {
#endif
/* The coroutine cURL multi engine (Rust, crates/curl, --enable-swoole-curl).
 * php_swoole_curl.h keeps swoole::curl::Multi and Handle as names over these;
 * C++ lends the reactor, timer and coroutine operations. */

#ifdef SW_USE_CURL
struct swr_v1_curl_multi;
struct swr_v1_curl_handle;
struct swr_v1_curl_ops {
    bool (*event_available)(void);
    /* Install the cURL fd type's reactor handlers unless they are. */
    void (*install_handlers)(void);
    /* A network::Socket for fd, of the cURL fd type, out of the reactor,
     * carrying object. */
    void *(*socket_new)(int fd, void *object);
    /* socket->events; socket->removed into removed unless it is null. */
    int (*socket_events)(void *socket, bool *removed);
    int (*event_add)(void *socket, int events);
    int (*event_set)(void *socket, int events);
    int (*event_del)(void *socket);
    /* Leave the reactor silently if in it, then free it without closing the fd. */
    void (*socket_free)(void *socket);
    bool (*timer_available)(void);
    /* A timer whose callback is swr_v1_curl_multi_on_timer(multi). */
    void *(*timer_add)(long ms, void *multi);
    void (*timer_del)(void *timer);
    void *(*current)(void);
    /* co->yield_ex(timeout), then whether it was canceled. */
    bool (*yield_)(void *co, double timeout);
    void (*resume)(void *co);
    /* swoole_event_defer of swr_v1_curl_multi_deferred(multi). */
    void (*defer)(void *multi);
    void (*set_canceled)(void);
    void (*warn_message)(void);
};
struct swr_v1_curl_multi *swr_v1_curl_multi_new(const struct swr_v1_curl_ops *ops);
void swr_v1_curl_multi_free(struct swr_v1_curl_multi *multi);
void *swr_v1_curl_multi_handle(struct swr_v1_curl_multi *multi);
int swr_v1_curl_multi_running(struct swr_v1_curl_multi *multi);
/* The coroutine waiting on the multi, or null. */
void *swr_v1_curl_multi_co(struct swr_v1_curl_multi *multi);
int swr_v1_curl_multi_add_handle(struct swr_v1_curl_multi *multi, struct swr_v1_curl_handle *handle);
int swr_v1_curl_multi_remove_handle(struct swr_v1_curl_multi *multi, struct swr_v1_curl_handle *handle);
/* After check_bound_co. */
int swr_v1_curl_multi_perform(struct swr_v1_curl_multi *multi);
long swr_v1_curl_multi_select(struct swr_v1_curl_multi *multi, void *co, double timeout);
/* A CURLcode, or -1 when it must wait outside a coroutine or while another
 * one waits: the caller raises check_bound_co's fatal error. */
int swr_v1_curl_multi_exec(struct swr_v1_curl_multi *multi, struct swr_v1_curl_handle *handle);
void swr_v1_curl_multi_on_timer(struct swr_v1_curl_multi *multi);
void swr_v1_curl_multi_deferred(struct swr_v1_curl_multi *multi);
/* A reactor event (CURL_CSELECT_* bits) on the socket whose object is object. */
void swr_v1_curl_socket_event(void *object, int bitmask);
struct swr_v1_curl_handle *swr_v1_curl_get_handle(void *cp);
struct swr_v1_curl_handle *swr_v1_curl_create_handle(void *cp);
void swr_v1_curl_destroy_handle(void *cp);
#endif
#ifdef __cplusplus
}
#endif
#endif
