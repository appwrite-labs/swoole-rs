/* Swoole\Event (Rust, crates/phpevents): C++ lends the reactor, socket and
 * zend::Callable operations; Rust registers the class and the swoole_event_*
 * functions, owns the fd registry and each fd's callbacks, and its
 * zif_swoole_event_* handlers are the method table's (wait, dispatch, exit and
 * rshutdown, which run the loop, stay C++). The C++ reactor and defer
 * trampolines call the swr_v1_phpevents_* bodies below and re-raise a bailout
 * they report. */
#ifndef SWOOLE_RS_PHPEVENTS_H
#define SWOOLE_RS_PHPEVENTS_H
#include <stddef.h>
#include <stdint.h>
#ifdef __cplusplus
extern "C" {
#endif
struct _zval_struct;
struct swr_v1_phpevents_invocation {
    void *callable;
    struct _zval_struct *argv;
    uint32_t argc;
    uint8_t ok;
};
struct swr_v1_phpevents_ops {
    void (*check_reactor)(void);
    void (*init_reactor)(void);
    uint8_t (*has_reactor)(void);
    int32_t (*fd)(struct _zval_struct *zsocket);
    void *(*callable)(struct _zval_struct *zfn, const char *name, uint8_t allow_null);
    void (*callable_free)(void *callable);
    void (*defer_callable_free)(void *callable);
    void (*call)(void *invocation);
    void *(*socket_make)(int32_t fd, void *object);
    int32_t (*socket_add)(void *socket, int64_t events);
    int32_t (*socket_write)(void *socket, const char *data, size_t length);
    int32_t (*socket_set)(void *socket, int64_t events);
    int32_t (*socket_del)(void *socket);
    void (*socket_free)(void *socket, uint8_t detach_fd);
    void *(*socket_object)(void *socket);
    void (*socket_detach)(void *socket);
    int32_t (*socket_events)(void *socket, uint8_t *removed);
    void (*defer_object)(void *object);
    void (*defer)(void *callable);
    void *(*cycle_get)(uint8_t before, uint8_t *armed);
    void (*cycle_set)(uint8_t before, void *callable, uint8_t armed);
    void (*fatal_warning)(const char *message);
    void (*warning)(const char *message);
};
void swr_v1_phpevents_minit(const struct swr_v1_phpevents_ops *ops);
/* Reactor read (0) or write (1) handler of a user socket: SW_OK, SW_ERR, or 1
 * when the callback bailed out. */
int32_t swr_v1_phpevents_ready(void *socket, uint8_t write);
/* Free an fd's callbacks and socket zval: 1 if a destructor bailed out. */
uint8_t swr_v1_phpevents_free(void *object);
/* A deferred (defer) or idle/future (cycle) callable: 1 if it bailed out. */
uint8_t swr_v1_phpevents_deferred(void *callable);
uint8_t swr_v1_phpevents_cycle(void *callable);
#ifdef __cplusplus
}
#endif
#endif
