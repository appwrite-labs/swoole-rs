/* PHP Swoole\Redis\Server (crates/phpredis, which also defines its MINIT and
 * RSHUTDOWN hooks and swoole_redis_server_ce): onReceive on a Redis port parses
 * the request and runs its handler (SW_OK/SW_ERR, or SWR_PHPREDIS_BAILED: the
 * caller re-raises the bailout). */
#ifndef SWOOLE_RS_PHPREDIS_H
#define SWOOLE_RS_PHPREDIS_H
#include <stdbool.h>
#include <stddef.h>
#include <stdint.h>
#ifdef __cplusplus
extern "C" {
#endif
struct _zend_class_entry;
struct _zval_struct;
struct _zend_object;
#define SWR_PHPREDIS_BAILED (-2)
int swr_v1_phpredis_receive(void *server, void *request);
/* What it asks of the Server, the request and Zend (ext-src/swoole_server.cc).
 * swr_redis_host_call is 1 called, 0 failed, -1 bailed out; swr_redis_host_callable
 * is false if creating the callable bailed out. */
void swr_redis_host_recv_data(void *server, void *request, struct _zval_struct *data);
int64_t swr_redis_host_fd(const void *request);
bool swr_redis_host_send(void *server, int64_t fd, const char *data, size_t length);
void swr_redis_host_close(void *server, int64_t fd);
int swr_redis_host_call(void *server, void *callable, struct _zval_struct *args, struct _zval_struct *retval);
bool swr_redis_host_callable(struct _zval_struct *callback, void **callable);
void swr_redis_host_callable_free(void *callable);
/* zend_update_property; false if the user error handler bailed out. */
bool swr_redis_host_update_property(struct _zend_class_entry *scope, struct _zend_object *object, const char *name,
                                    size_t length, struct _zval_struct *value);
void swr_redis_host_erange(void);
void swr_redis_host_error(int level, const char *message);
void swr_redis_host_fatal_error(int level, const char *message);
#ifdef __cplusplus
}
#endif
#endif
