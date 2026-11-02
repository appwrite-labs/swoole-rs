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
    zend_try {
        php_stream_notify_progress_increment(PHP_STREAM_CONTEXT(stream), bytes, 0);
    }
    zend_catch {
        bailed = 1;
    }
    zend_end_try();
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

bool swr_zend_stream_vm_active(void) {
    return EG(active);
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
