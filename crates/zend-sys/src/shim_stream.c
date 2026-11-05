#include "shim.h"
/* Swoole's coroutine socket streams (crates/phpruntime). */
#include "shim_stream.h"
#include "ext/standard/file.h"

php_stream *swr_zend_stream_alloc(const php_stream_ops *ops, void *abstract, const char *persistent_id) {
    return php_stream_alloc_rel(ops, abstract, persistent_id, "r+");
}

void swr_zend_stream_close(php_stream *stream) {
    php_stream_close(stream);
}

bool swr_zend_stream_eof(const php_stream *stream) {
    return stream->eof;
}

void swr_zend_stream_set_eof(php_stream *stream) {
    stream->eof = 1;
}

bool swr_zend_stream_buffered(const php_stream *stream) {
#if PHP_VERSION_ID >= 80300
    return stream->has_buffered_data;
#else
    (void) stream;
    return false;
#endif
}

void swr_zend_stream_avoid_blocking(php_stream *stream) {
    stream->flags |= PHP_STREAM_FLAG_AVOID_BLOCKING;
}

int swr_zend_stream_progress(php_stream *stream, size_t bytes) {
    int bailed = 0;
    SWR_ZEND_TRY {
        php_stream_notify_progress_increment(PHP_STREAM_CONTEXT(stream), bytes, 0);
    }
    SWR_ZEND_CATCH {
        bailed = 1;
    }
    SWR_ZEND_END_TRY();
    return bailed;
}

zval *swr_zend_stream_option(php_stream *stream, const char *wrapper, const char *name) {
    php_stream_context *context = PHP_STREAM_CONTEXT(stream);
    return context ? php_stream_context_get_option(context, wrapper, name) : NULL;
}

void swr_zend_stream_set_option(php_stream *stream, const char *wrapper, const char *name, zval *value) {
    php_stream_context_set_option(PHP_STREAM_CONTEXT(stream), wrapper, name, value);
}

php_stream_context *swr_zend_stream_context(php_stream *stream) {
    return PHP_STREAM_CONTEXT(stream);
}

zend_array *swr_zend_stream_context_options(php_stream_context *context) {
    return context && Z_TYPE(context->options) == IS_ARRAY ? Z_ARRVAL(context->options) : NULL;
}

void swr_zend_stream_share_context(php_stream *client, const php_stream *server) {
    client->ctx = server->ctx;
    if (server->ctx) {
        GC_ADDREF(server->ctx);
    }
}

zend_resource *swr_zend_stream_expose(php_stream *stream) {
    php_stream_auto_cleanup(stream);
    return stream->res;
}

void swr_zend_stream_timeval(const struct timeval *time, double *seconds, double *micros) {
    *seconds = (double) time->tv_sec;
    *micros = (double) time->tv_usec;
}

bool swr_zend_stream_vm_active(void) {
    return EG(active);
}

php_stream *swr_zend_stream_from_zval(zval *value) {
    php_stream *stream;
    php_stream_from_zval_no_verify(stream, value);
    return stream;
}

zend_off_t swr_zend_stream_buffered_bytes(const php_stream *stream) {
    return stream->writepos - stream->readpos;
}

void swr_zend_stream_set_array(zval *value, zend_array *array) {
    ZVAL_ARR(value, array);
}

zend_long swr_zend_stream_default_timeout(void) {
    return FG(default_socket_timeout);
}

zend_string *swr_zend_stream_empty_string(void) {
    return ZSTR_EMPTY_ALLOC();
}

int swr_zend_stream_xport_op(const php_stream_xport_param *param) {
    return param->op;
}

unsigned swr_zend_stream_xport_flags(const php_stream_xport_param *param) {
    return param->want_addr | param->want_textaddr << 1 | param->want_errortext << 2 | param->how << 3;
}

int swr_zend_stream_crypto_op(const php_stream_xport_crypto_param *param) {
    return param->op;
}

int swr_zend_try_stream_free(php_stream *stream, int flags) {
    int bailed = 0;
    SWR_ZEND_TRY { php_stream_free(stream, flags); }
    SWR_ZEND_CATCH { bailed = 1; } SWR_ZEND_END_TRY();
    return bailed;
}

php_stream *swr_zend_try_stream_factory(void *factory, const char *proto, size_t protolen,
    const char *name, size_t namelen, const char *persistent, int options, int flags,
    struct timeval *timeout, php_stream_context *context, bool *bailed) {
    php_stream *stream = NULL;
    SWR_ZEND_TRY {
        stream = ((php_stream_transport_factory) factory)(proto, protolen, name, namelen,
            persistent, options, flags, timeout, context STREAMS_CC);
    } SWR_ZEND_CATCH { *bailed = true; } SWR_ZEND_END_TRY();
    return stream;
}

int swr_zend_try_stream_alloc(const php_stream_ops *ops, void *abstract, const char *persistent, php_stream **stream) {
    int bailed = 0;
    SWR_ZEND_TRY { *stream = php_stream_alloc_rel(ops, abstract, persistent, "r+"); }
    SWR_ZEND_CATCH { bailed = 1; } SWR_ZEND_END_TRY();
    return bailed;
}
int swr_zend_try_stream_close(php_stream *stream) {
    int bailed = 0;
    SWR_ZEND_TRY { php_stream_close(stream); }
    SWR_ZEND_CATCH { bailed = 1; } SWR_ZEND_END_TRY();
    return bailed;
}
