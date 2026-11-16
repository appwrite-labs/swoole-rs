/* Swoole's coroutine socket streams (crates/phpruntime): php_stream macros,
 * bitfields and globals, and the stream calls that can run PHP code. */
#ifndef SWOOLE_RS_ZEND_SHIM_STREAM_H
#define SWOOLE_RS_ZEND_SHIM_STREAM_H
#include "php.h"
#include "php_network.h"

/* php_stream_close. */
void swr_zend_stream_close(php_stream *stream);
/* stream->has_buffered_data (PHP 8.3+, else false). */
bool swr_zend_stream_buffered(const php_stream *stream);
/* php_stream_notify_progress_increment(PHP_STREAM_CONTEXT(stream), bytes, 0)
 * inside zend_try: 1 if the notifier bailed out. */
int swr_zend_stream_progress(php_stream *stream, size_t bytes);
/* php_stream_context_get_option(PHP_STREAM_CONTEXT(stream), wrapper, name):
 * null without a context, without options for `wrapper`, or where the
 * context's options or the wrapper's are no array. */
zval *swr_zend_stream_option(const php_stream *stream, const char *wrapper, const char *name);
/* php_stream_context_set_option(PHP_STREAM_CONTEXT(stream), wrapper, name, value). */
void swr_zend_stream_set_option(php_stream *stream, const char *wrapper, const char *name, zval *value);
/* PHP_STREAM_CONTEXT(stream). */
php_stream_context *swr_zend_stream_context(const php_stream *stream);
/* The context's options array; null without a context or an array. */
zend_array *swr_zend_stream_context_options(php_stream_context *context);
/* client->ctx = server->ctx, counted. */
void swr_zend_stream_share_context(php_stream *client, const php_stream *server);
/* php_stream_auto_cleanup(stream), then stream->res. */
zend_resource *swr_zend_stream_expose(php_stream *stream);
/* php_stream_to_zval: `value` holds the stream's resource, in place of what it
 * held, which is not released. */
void swr_zend_stream_to_zval(php_stream *stream, zval *value);
/* EG(active). */
bool swr_zend_stream_vm_active(void);
/* php_stream_from_zval_no_verify: null if not a stream. */
php_stream *swr_zend_stream_from_zval(zval *value);
/* Whether `value` is a stream resource (php_stream_from_zval_no_verify) whose
 * read buffer holds bytes not yet read (writepos > readpos). */
bool swr_zend_stream_zval_buffered(zval *value);

/* FG(default_socket_timeout). */
zend_long swr_zend_stream_default_timeout(void);
/* php_stream_free(stream, KEEP_RSRC | CLOSE, and CLOSE_PERSISTENT for a
 * persistent stream) inside zend_try: 1 if it bailed out. */
int swr_zend_try_stream_free(php_stream *stream);
/* Invoke one PHP transport factory with its native debug ABI, inside zend_try. */
php_stream *swr_zend_try_stream_factory(void *factory, const char *proto, size_t protolen,
    const char *name, size_t namelen, const char *persistent, int options, int flags,
    struct timeval *timeout, php_stream_context *context, bool *bailed);
/* The coroutine transport factory (crates/phpruntime) as a php_stream_transport_factory
 * with its native debug ABI: the body set once at MINIT, then the bailout it trapped. */
typedef php_stream *(*swr_zend_stream_factory_body)(const char *proto, size_t protolen,
    const char *name, size_t namelen, const char *persistent, int options, int flags,
    struct timeval *timeout, php_stream_context *context, bool *bailed);
void swr_zend_stream_factory_set(swr_zend_stream_factory_body body);
php_stream *swr_zend_stream_factory(const char *proto, size_t protolen, const char *name,
    size_t namelen, const char *persistent, int options, int flags, struct timeval *timeout,
    php_stream_context *context STREAMS_DC);
/* The "socket/coroutine" operations: C entries that type each argument for
 * crates/phpruntime's. */
extern const php_stream_ops swoole_runtime_socket_ops;
/* php_stream_alloc of those operations, with no abstract yet, inside zend_try:
 * 1 if it bailed out. */
int swr_zend_try_socket_stream_alloc(const char *persistent, php_stream **stream);
int swr_zend_try_stream_close(php_stream *);
/* php_network_populate_name_from_sockaddr of the first `size` bytes of `peer`,
 * which holds them. */
void swr_zend_stream_populate_name(const struct sockaddr *peer, socklen_t size,
    zend_string **textaddr, struct sockaddr **addr, socklen_t *addrlen);
#endif
