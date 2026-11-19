#include "shim.h"
/* Swoole's coroutine socket streams (crates/phpruntime). Stays C (docs/zend.md,
 * What stays C): bailout (notifiers, frees and factories run PHP), globals
 * (FG(default_socket_timeout), EG(active)), version (has_buffered_data from 8.3,
 * the STREAMS_DC debug ABI, timeval's field widths) and allocator
 * (php_stream_alloc: on a debug PHP its _rel form needs the caller's STREAMS_DC). */
#include "shim_stream.h"
#include "ext/standard/file.h"

/* version: has_buffered_data first appeared in PHP 8.3. */
bool swr_zend_stream_buffered(const php_stream *stream) {
#if PHP_VERSION_ID >= 80300
    return stream->has_buffered_data;
#else
    (void) stream;
    return false;
#endif
}

/* bailout: a PHP progress notifier can exit nonlocally. */
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

/* budget: PHP_STREAM_CONTEXT follows a raw resource to its untyped data. */
php_stream_context *swr_zend_stream_context(const php_stream *stream) {
    return PHP_STREAM_CONTEXT(stream);
}

/* budget: StreamContext holds a raw context pointer to the options zval. */
zend_array *swr_zend_stream_context_options(php_stream_context *context) {
    return context && Z_TYPE(context->options) == IS_ARRAY ? Z_ARRVAL(context->options) : NULL;
}

/* budget: GC_ADDREF writes through the server's raw resource pointer. */
void swr_zend_stream_share_context(php_stream *client, const php_stream *server) {
    client->ctx = server->ctx;
    if (server->ctx) {
        GC_ADDREF(server->ctx);
    }
}

/* budget: both imported and fresh handles carry raw stream pointers. */
void swr_zend_stream_to_zval(php_stream *stream, zval *value) {
    php_stream_to_zval(stream, value);
}

/* globals: read EG(active) for the current PHP thread. */
bool swr_zend_stream_vm_active(void) {
    return EG(active);
}

/* budget: Value exposes the checked stream as a raw pointer; reading its positions needs unsafe. */
bool swr_zend_stream_zval_buffered(zval *value) {
    php_stream *stream;
    php_stream_from_zval_no_verify(stream, value);
    return stream && stream->writepos > stream->readpos;
}

/* globals: read the current thread's FG timeout. */
zend_long swr_zend_stream_default_timeout(void) {
    return FG(default_socket_timeout);
}

/* bailout: the stream close callback can run PHP. */
int swr_zend_try_stream_free(php_stream *stream) {
    volatile int bailed = 0;
    SWR_ZEND_TRY {
        php_stream_free(stream, PHP_STREAM_FREE_KEEP_RSRC | PHP_STREAM_FREE_CLOSE |
                        (stream->is_persistent ? PHP_STREAM_FREE_CLOSE_PERSISTENT : 0));
    }
    SWR_ZEND_CATCH { bailed = 1; } SWR_ZEND_END_TRY();
    return bailed;
}

/* bailout: user stream wrappers can exit during a read. */
zend_long swr_zend_try_stream_zval_read(zval *value, char *buffer, size_t size) {
    php_stream *stream;
    php_stream_from_zval_no_verify(stream, value);
    if (!stream) {
        return -1;
    }
    volatile zend_long count = -2;
    SWR_ZEND_TRY {
        ssize_t got = php_stream_read(stream, buffer, size);
        count = got < 0 ? -1 : got;
    }
    SWR_ZEND_CATCH {} SWR_ZEND_END_TRY();
    return count;
}

/* Set at MINIT, before any PHP thread starts. */
static swr_zend_stream_factory_body factory_body;

/* budget: installs the function pointer read by the STREAMS_DC factory. */
void swr_zend_stream_factory_set(swr_zend_stream_factory_body body) {
    factory_body = body;
}

/* version: forward the build's debug caller arguments to the original factory. */
php_stream *swr_zend_stream_factory(const char *proto, size_t protolen, const char *name,
    size_t namelen, const char *persistent, int options, int flags, struct timeval *timeout,
    php_stream_context *context STREAMS_DC) {
    bool bailed = false;
    php_stream_transport_factory original = NULL;
    php_stream *stream = factory_body(proto, protolen, name, persistent, context, &bailed, &original);
    if (bailed) {
        zend_bailout();
    }
    if (original) {
        stream = original(proto, protolen, name, namelen, persistent, options, flags, timeout,
            context STREAMS_CC);
    }
    return stream;
}

/* Rust's "socket/coroutine" operations (crates/phpruntime/src/stream.rs), over a
 * stream of the table below. Each argument arrives typed: a buffer Zend lends as
 * one iovec, a parameter only for the option that says what it points to. */
ssize_t swr_v1_phpruntime_socket_write(php_stream *stream, struct iovec data);
ssize_t swr_v1_phpruntime_socket_read(php_stream *stream, struct iovec out);
int swr_v1_phpruntime_socket_close(php_stream *stream, int close_handle);
int swr_v1_phpruntime_socket_flush(php_stream *stream);
bool swr_v1_phpruntime_socket_cast(php_stream *stream, int castas, FILE **file, php_socket_t *fd);
_Static_assert(offsetof(php_stream_statbuf, sb) == 0, "socket_stat writes the leading stat field");
int swr_v1_phpruntime_socket_stat(php_stream *stream, php_stream_statbuf *ssb);
bool swr_v1_phpruntime_socket_xport(php_stream *stream, php_stream_xport_param *param,
    const struct timeval *timeout, const struct iovec *name, const struct iovec *send,
    struct iovec *recv, const struct iovec *to);
bool swr_v1_phpruntime_socket_option(php_stream *stream, int option, int value, zval *data,
    const struct timeval *timeout, php_stream_xport_crypto_param *crypto);

/* budget: pair a raw span and its count, supplying a non-null base for an empty Rust slice. */
static inline struct iovec lend(const void *buf, size_t count) {
    struct iovec lent = {(void *) (count ? buf : ""), count};
    return lent;
}

/* budget: join PHP's raw span into the IoSlice borrowed by the Rust writer. */
static ssize_t socket_write(php_stream *stream, const char *buf, size_t count) {
    return swr_v1_phpruntime_socket_write(stream, lend(buf, count));
}

/* budget: join the raw output pointer and capacity before borrowing them in Rust. */
static ssize_t socket_read(php_stream *stream, char *buf, size_t count) {
    return swr_v1_phpruntime_socket_read(stream, lend(buf, count));
}

/* budget: only castas determines the type of the untyped output slot. */
static int socket_cast(php_stream *stream, int castas, void **ret) {
    bool stdio = castas == PHP_STREAM_AS_STDIO;
    return swr_v1_phpruntime_socket_cast(
               stream, castas, stdio ? (FILE **) ret : NULL, stdio ? NULL : (php_socket_t *) ret)
               ? SUCCESS
               : FAILURE;
}

/* budget: option and operation tags select the types of raw union pointers. */
static int socket_set_option(php_stream *stream, int option, int value, void *ptrparam) {
    bool done;
    if (option == PHP_STREAM_OPTION_XPORT_API) {
        php_stream_xport_param *param = ptrparam;
        struct iovec name = lend(param->inputs.name, param->inputs.namelen);
        struct iovec buf = lend(param->inputs.buf, param->inputs.buflen);
        struct iovec to = lend(param->inputs.addr, param->inputs.addrlen);
        done = swr_v1_phpruntime_socket_xport(stream, param, param->inputs.timeout,
            param->inputs.name ? &name : NULL, param->op == STREAM_XPORT_OP_SEND ? &buf : NULL,
            param->op == STREAM_XPORT_OP_RECV ? &buf : NULL, param->inputs.addr ? &to : NULL);
    } else {
        done = swr_v1_phpruntime_socket_option(stream, option, value,
            option == PHP_STREAM_OPTION_META_DATA_API ? ptrparam : NULL,
            option == PHP_STREAM_OPTION_READ_TIMEOUT ? ptrparam : NULL,
            option == PHP_STREAM_OPTION_CRYPTO_API ? ptrparam : NULL);
    }
    return done ? PHP_STREAM_OPTION_RETURN_OK : PHP_STREAM_OPTION_RETURN_ERR;
}

#define SOCKET_OPS(label) {socket_write, socket_read, swr_v1_phpruntime_socket_close, \
                           swr_v1_phpruntime_socket_flush, label, NULL, socket_cast, \
                           swr_v1_phpruntime_socket_stat, socket_set_option}
const php_stream_ops swoole_runtime_socket_ops = SOCKET_OPS("socket/coroutine");
static const php_stream_ops unix_socket_ops = SOCKET_OPS("unix_socket");

/* allocator: php_stream_alloc expands to the build's debug allocation ABI. */
int swr_zend_try_socket_stream_alloc(const char *persistent, bool unix_stream, php_stream **stream) {
    volatile int bailed = 0;
    SWR_ZEND_TRY { *stream = php_stream_alloc(unix_stream ? &unix_socket_ops : &swoole_runtime_socket_ops, NULL, persistent, "r+"); }
    SWR_ZEND_CATCH { bailed = 1; } SWR_ZEND_END_TRY();
    return bailed;
}

/* bailout: trap user code invoked by the stream close. */
int swr_zend_try_stream_close(php_stream *stream) {
    volatile int bailed = 0;
    SWR_ZEND_TRY { php_stream_close(stream); }
    SWR_ZEND_CATCH { bailed = 1; } SWR_ZEND_END_TRY();
    return bailed;
}
