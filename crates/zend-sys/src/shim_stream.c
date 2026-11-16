#include "shim.h"
/* Swoole's coroutine socket streams (crates/phpruntime). Stays C (docs/zend.md,
 * What stays C): bailout (notifiers, frees and factories run PHP), globals
 * (FG(default_socket_timeout), EG(active)), version (has_buffered_data from 8.3,
 * the STREAMS_DC debug ABI, timeval's field widths) and allocator
 * (php_stream_alloc: on a debug PHP its _rel form needs the caller's STREAMS_DC). */
#include "shim_stream.h"
#include "ext/standard/file.h"

void swr_zend_stream_close(php_stream *stream) {
    php_stream_close(stream);
}

bool swr_zend_stream_buffered(const php_stream *stream) {
#if PHP_VERSION_ID >= 80300
    return stream->has_buffered_data;
#else
    (void) stream;
    return false;
#endif
}

int swr_zend_stream_progress(php_stream *stream, size_t bytes) {
    volatile int bailed = 0;
    SWR_ZEND_TRY {
        php_stream_notify_progress_increment(PHP_STREAM_CONTEXT(stream), bytes, 0);
    }
    SWR_ZEND_CATCH {
        bailed = 1;
    }
    SWR_ZEND_END_TRY();
    return bailed;
}

zval *swr_zend_stream_option(const php_stream *stream, const char *wrapper, const char *name) {
    php_stream_context *context = PHP_STREAM_CONTEXT(stream);
    if (!context || Z_TYPE(context->options) != IS_ARRAY) {
        return NULL;
    }
    zval *options = zend_hash_str_find(Z_ARRVAL(context->options), wrapper, strlen(wrapper));
    if (!options || Z_TYPE_P(options) != IS_ARRAY) {
        return NULL;
    }
    return zend_hash_str_find(Z_ARRVAL_P(options), name, strlen(name));
}

void swr_zend_stream_set_option(php_stream *stream, const char *wrapper, const char *name, zval *value) {
    php_stream_context_set_option(PHP_STREAM_CONTEXT(stream), wrapper, name, value);
}

php_stream_context *swr_zend_stream_context(const php_stream *stream) {
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

void swr_zend_stream_to_zval(php_stream *stream, zval *value) {
    php_stream_to_zval(stream, value);
}

bool swr_zend_stream_vm_active(void) {
    return EG(active);
}

php_stream *swr_zend_stream_from_zval(zval *value) {
    php_stream *stream;
    php_stream_from_zval_no_verify(stream, value);
    return stream;
}

bool swr_zend_stream_zval_buffered(zval *value) {
    php_stream *stream;
    php_stream_from_zval_no_verify(stream, value);
    return stream && stream->writepos > stream->readpos;
}

zend_long swr_zend_stream_default_timeout(void) {
    return FG(default_socket_timeout);
}

int swr_zend_try_stream_free(php_stream *stream, int flags) {
    volatile int bailed = 0;
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

/* Set at MINIT, before any PHP thread starts. */
static swr_zend_stream_factory_body factory_body;

void swr_zend_stream_factory_set(swr_zend_stream_factory_body body) {
    factory_body = body;
}

php_stream *swr_zend_stream_factory(const char *proto, size_t protolen, const char *name,
    size_t namelen, const char *persistent, int options, int flags, struct timeval *timeout,
    php_stream_context *context STREAMS_DC) {
    bool bailed = false;
    php_stream *stream = factory_body(proto, protolen, name, namelen, persistent, options, flags,
        timeout, context, &bailed);
    if (bailed) {
        zend_bailout();
    }
    return stream;
}

/* Rust's "socket/coroutine" operations (crates/phpruntime/src/stream.rs), over a
 * stream of the table below. Each argument arrives typed: a buffer Zend lends as
 * one iovec, a parameter only for the option that says what it points to. */
ssize_t swr_v1_phpruntime_socket_write(php_stream *stream, struct iovec data);
ssize_t swr_v1_phpruntime_socket_read(php_stream *stream, struct iovec out);
bool swr_v1_phpruntime_socket_close(php_stream *stream);
bool swr_v1_phpruntime_socket_cast(php_stream *stream, int castas, FILE **file, php_socket_t *fd);
bool swr_v1_phpruntime_socket_stat(php_stream *stream, zend_stat_t *sb);
bool swr_v1_phpruntime_socket_xport(php_stream *stream, php_stream_xport_param *param,
    const struct timeval *timeout, const struct iovec *name, const struct iovec *send,
    struct iovec *recv);
bool swr_v1_phpruntime_socket_option(php_stream *stream, int option, int value, zval *data,
    const struct timeval *timeout, php_stream_xport_crypto_param *crypto);

/* The `count` bytes Zend lends at `buf`, never null. */
static inline struct iovec lend(const void *buf, size_t count) {
    struct iovec lent = {(void *) (count ? buf : ""), count};
    return lent;
}

static ssize_t socket_write(php_stream *stream, const char *buf, size_t count) {
    return swr_v1_phpruntime_socket_write(stream, lend(buf, count));
}

static ssize_t socket_read(php_stream *stream, char *buf, size_t count) {
    return swr_v1_phpruntime_socket_read(stream, lend(buf, count));
}

static int socket_close(php_stream *stream, int close_handle) {
    (void) close_handle;
    return swr_v1_phpruntime_socket_close(stream) ? SUCCESS : FAILURE;
}

/* Nothing is buffered. */
static int socket_flush(php_stream *stream) {
    (void) stream;
    return 0;
}

static int socket_cast(php_stream *stream, int castas, void **ret) {
    bool stdio = castas == PHP_STREAM_AS_STDIO;
    return swr_v1_phpruntime_socket_cast(
               stream, castas, stdio ? (FILE **) ret : NULL, stdio ? NULL : (php_socket_t *) ret)
               ? SUCCESS
               : FAILURE;
}

static int socket_stat(php_stream *stream, php_stream_statbuf *ssb) {
    return swr_v1_phpruntime_socket_stat(stream, &ssb->sb) ? SUCCESS : FAILURE;
}

static int socket_set_option(php_stream *stream, int option, int value, void *ptrparam) {
    bool done;
    if (option == PHP_STREAM_OPTION_XPORT_API) {
        php_stream_xport_param *param = ptrparam;
        struct iovec name = lend(param->inputs.name, param->inputs.namelen);
        struct iovec buf = lend(param->inputs.buf, param->inputs.buflen);
        done = swr_v1_phpruntime_socket_xport(stream, param, param->inputs.timeout,
            param->inputs.name ? &name : NULL, param->op == STREAM_XPORT_OP_SEND ? &buf : NULL,
            param->op == STREAM_XPORT_OP_RECV ? &buf : NULL);
    } else {
        done = swr_v1_phpruntime_socket_option(stream, option, value,
            option == PHP_STREAM_OPTION_META_DATA_API ? ptrparam : NULL,
            option == PHP_STREAM_OPTION_READ_TIMEOUT ? ptrparam : NULL,
            option == PHP_STREAM_OPTION_CRYPTO_API ? ptrparam : NULL);
    }
    return done ? PHP_STREAM_OPTION_RETURN_OK : PHP_STREAM_OPTION_RETURN_ERR;
}

const php_stream_ops swoole_runtime_socket_ops = {
    socket_write, socket_read, socket_close, socket_flush, "socket/coroutine", NULL,
    socket_cast, socket_stat, socket_set_option,
};

int swr_zend_try_socket_stream_alloc(const char *persistent, php_stream **stream) {
    volatile int bailed = 0;
    SWR_ZEND_TRY { *stream = php_stream_alloc(&swoole_runtime_socket_ops, NULL, persistent, "r+"); }
    SWR_ZEND_CATCH { bailed = 1; } SWR_ZEND_END_TRY();
    return bailed;
}

void swr_zend_stream_populate_name(const struct sockaddr_storage *peer, socklen_t size,
    zend_string **textaddr, struct sockaddr **addr, socklen_t *addrlen) {
    php_network_populate_name_from_sockaddr((struct sockaddr *) peer,
        size < sizeof *peer ? size : (socklen_t) sizeof *peer, textaddr, addr, addrlen);
}
int swr_zend_try_stream_close(php_stream *stream) {
    volatile int bailed = 0;
    SWR_ZEND_TRY { php_stream_close(stream); }
    SWR_ZEND_CATCH { bailed = 1; } SWR_ZEND_END_TRY();
    return bailed;
}
