/* Swoole's coroutine socket streams (crates/phpruntime): php_stream macros,
 * bitfields and globals, and the stream calls that can run PHP code. */
#ifndef SWOOLE_RS_ZEND_SHIM_STREAM_H
#define SWOOLE_RS_ZEND_SHIM_STREAM_H
#include "php.h"
#include "php_network.h"

/* php_stream_alloc_rel(ops, abstract, persistent_id, "r+"). */
php_stream *swr_zend_stream_alloc(const php_stream_ops *ops, void *abstract, const char *persistent_id);
/* php_stream_close. */
void swr_zend_stream_close(php_stream *stream);
/* stream->eof. */
bool swr_zend_stream_eof(const php_stream *stream);
void swr_zend_stream_set_eof(php_stream *stream);
/* stream->has_buffered_data (PHP 8.3+, else false). */
bool swr_zend_stream_buffered(const php_stream *stream);
/* stream->flags |= PHP_STREAM_FLAG_AVOID_BLOCKING. */
void swr_zend_stream_avoid_blocking(php_stream *stream);
/* php_stream_notify_progress_increment(PHP_STREAM_CONTEXT(stream), bytes, 0)
 * inside zend_try: 1 if the notifier bailed out. */
int swr_zend_stream_progress(php_stream *stream, size_t bytes);
/* php_stream_context_get_option(PHP_STREAM_CONTEXT(stream), wrapper, name),
 * null without a context. */
zval *swr_zend_stream_option(php_stream *stream, const char *wrapper, const char *name);
/* php_stream_context_set_option(PHP_STREAM_CONTEXT(stream), wrapper, name, value). */
void swr_zend_stream_set_option(php_stream *stream, const char *wrapper, const char *name, zval *value);
/* PHP_STREAM_CONTEXT(stream). */
php_stream_context *swr_zend_stream_context(php_stream *stream);
/* The context's options array; null without a context or an array. */
zend_array *swr_zend_stream_context_options(php_stream_context *context);
/* client->ctx = server->ctx, counted. */
void swr_zend_stream_share_context(php_stream *client, const php_stream *server);
/* php_stream_auto_cleanup(stream), then stream->res. */
zend_resource *swr_zend_stream_expose(php_stream *stream);
/* (double) time->tv_sec and (double) time->tv_usec. */
void swr_zend_stream_timeval(const struct timeval *time, double *seconds, double *micros);
/* EG(active). */
bool swr_zend_stream_vm_active(void);
/* php_stream_from_zval_no_verify: null if not a stream. */
php_stream *swr_zend_stream_from_zval(zval *value);
/* stream->writepos - stream->readpos. */
zend_off_t swr_zend_stream_buffered_bytes(const php_stream *stream);
/* ZVAL_ARR(value, array). */
void swr_zend_stream_set_array(zval *value, zend_array *array);
/* FG(default_socket_timeout). */
zend_long swr_zend_stream_default_timeout(void);
/* ZSTR_EMPTY_ALLOC(). */
zend_string *swr_zend_stream_empty_string(void);
/* A transport parameter's op, and its want_addr (bit 0), want_textaddr (1),
 * want_errortext (2) and how (bits 3-4). */
int swr_zend_stream_xport_op(const php_stream_xport_param *param);
unsigned swr_zend_stream_xport_flags(const php_stream_xport_param *param);
/* A crypto parameter's op. */
int swr_zend_stream_crypto_op(const php_stream_xport_crypto_param *param);
#endif
