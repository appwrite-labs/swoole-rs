/* Event policy and callback ownership live in Rust. These static leaves lend
 * the native reactor/socket and PHP SDK effects. C++ trampolines re-raise
 * reported Zend bailouts only after the Rust call and its cleanup return. */
#ifndef SWOOLE_RS_PHPEVENTS_H
#define SWOOLE_RS_PHPEVENTS_H
#include <stddef.h>
#include <stdint.h>
#ifdef __cplusplus
extern "C" {
#endif
struct _zval_struct;
struct swr_v1_phpevents_driver {
    uint8_t (*cli)(void);
    uint8_t (*shutdown_begun)(void);
    uint8_t (*server)(void);
    uint8_t (*fatal)(void);
    uint8_t (*coroutine)(void);
    uint8_t (*bailed)(void);
    int32_t (*create)(int32_t);
    uint8_t (*installed)(void);
    void (*install)(void *);
    void (*trace)(void *);
    void (*shutdown)(void *);
    uint8_t (*user_condition)(void);
    uint8_t (*condition_installed)(void);
    void (*condition_set)(void *);
    void (*if_exit)(void *);
    void (*wait)(void *);
    void (*free)(void *);
    void (*timers)(void *);
    void (*running)(uint8_t);
    void (*once)(uint8_t);
    void (*report)(int32_t, const char *);
    void (*system_error)(int32_t, int32_t, const char *);
};
struct _zend_string;
struct _zend_class_entry;
struct swr_v1_phpevents_descriptors {
    struct _zend_string *(*property_name)(uint8_t);
    struct _zend_class_entry *(*socket_class)(void);
    int32_t (*socket_fd)(struct _zval_struct *, uint8_t *);
};
struct swr_v1_phpevents_ops {
    void (*error_dispatch)(void *);
    struct swr_v1_phpevents_driver driver;
    struct swr_v1_phpevents_descriptors descriptors;
    uint8_t (*has_reactor)(void);
    void (*defer_callable_free)(void *);
    uint8_t (*enabled_coroutine)(void);
    int64_t (*create_coroutine)(void *, uint32_t, struct _zval_struct *);
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
struct swr_v1_phpevents_error_event {
    void *event;
    void *socket;
    int32_t fd;
    int32_t reported_fd;
    int32_t events;
    int32_t result;
};
uint8_t swr_v1_phpevents_error(struct swr_v1_phpevents_error_event *);
int32_t swr_v1_phpevents_init(void);
int32_t swr_v1_phpevents_fd(struct _zval_struct *);
void swr_v1_phpevents_wait(void);
void swr_v1_phpevents_shutdown(void);
void swr_v1_phpevents_minit(const struct swr_v1_phpevents_ops *ops);
/* Reactor read (0) or write (1) handler of a user socket: SW_OK, SW_ERR, or 1
 * when the callback bailed out. */
int32_t swr_v1_phpevents_ready(void *socket, uint8_t write);
/* Free an fd's callbacks and socket zval: 1 if a destructor bailed out. */
uint8_t swr_v1_phpevents_free(void *object);
/* A deferred (defer) or idle/future (cycle) callable: 1 if it bailed out. */
uint8_t swr_v1_phpevents_callable_free(void *);
uint8_t swr_v1_phpevents_deferred(void *callable);
uint8_t swr_v1_phpevents_cycle(void *callable);
#ifdef __cplusplus
}
#endif
#endif
