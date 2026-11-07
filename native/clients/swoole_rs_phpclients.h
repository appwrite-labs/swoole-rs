#ifndef SWOOLE_RS_PHPCLIENTS_H
#define SWOOLE_RS_PHPCLIENTS_H

#include <stdbool.h>
#include <stddef.h>
#include <stdint.h>
#include <sys/types.h>

#ifdef __cplusplus
extern "C" {
#endif

struct _zend_class_entry;
struct _zend_object;
struct _zval_struct;
struct _zend_string;
/* php_swoole_error(level, "%s", message), or with fatal php_swoole_fatal_error
 * (crates/phpclients). */
void swoole_rs_php_error(int level, bool fatal, const char *message);

/* PHP Swoole\Client and Swoole\Async\Client (crates/phpclients): MINIT
 * (php_swoole_client_minit and php_swoole_client_async_minit), and the async events
 * network::Client raises, which re-raise a PHP callback's bailout as their
 * last step. For SWOOLE_CLIENT_ON_ERROR, `length` is
 * swoole_get_last_error(). */
enum {
    SWOOLE_CLIENT_ON_CONNECT = 1,
    SWOOLE_CLIENT_ON_RECEIVE = 2,
    SWOOLE_CLIENT_ON_CLOSE = 3,
    SWOOLE_CLIENT_ON_ERROR = 4,
    SWOOLE_CLIENT_ON_BUFFER_FULL = 5,
    SWOOLE_CLIENT_ON_BUFFER_EMPTY = 6,
};
void swr_v1_phpclients_async_event(struct _zend_object *object, int kind, const char *data, size_t length);

/* Defined by native/clients/client.cc: network::Client, one call per operation. */
enum { SWOOLE_CLIENT_ERROR = 0, SWOOLE_CLIENT_ERROR_FATAL = 1, SWOOLE_CLIENT_ERROR_SYS = 2, SWOOLE_CLIENT_ERROR_CORE = 3 };
/* php_swoole_error, _fatal_error, _sys_error or _core_error(level, "%s", message). */
void swoole_client_php_error(int kind, int level, const char *message);
/* new Client(type, async); null, with errno in `error`, when it has no socket. */
void *swoole_client_create(int type, bool async, int *error);
/* The client's Rust state (crates/clients' ClientState). */
void *swoole_client_state(void *cli);
/* php_swoole_convert_to_socket(fd) into `zsocket`; false without ext/sockets. */
bool swoole_client_export_socket(void *cli, struct _zval_struct *zsocket);
/* Install the async callbacks in `events` (1 << SWOOLE_CLIENT_ON_*), raising them on `object`. */
void swoole_client_async_listen(void *cli, struct _zend_object *object, uint32_t events);
bool swoole_client_has_on_error(void *cli);

/* The client's SSLContext owner, which crates/tls configures; NULL while TLS is off. */
const struct swr_v1_tls_owner *swoole_client_ssl_context(void *cli);
/* set_http_proxy (http) or set_socks5_proxy. */
void swoole_client_proxy(void *cli,
                         bool http,
                         const char *host,
                         size_t host_length,
                         int64_t port,
                         const char *user,
                         size_t user_length,
                         const char *password,
                         size_t password_length);

/* One operation that can run PHP code, run by swoole_client_run as a
 * bailout::trap body. */
enum {
    SWOOLE_CLIENT_CONNECT = 0,     /* data: host; a: port; timeout; b: sock_flag */
    SWOOLE_CLIENT_SEND = 1,        /* data, length; a: flags */
    SWOOLE_CLIENT_SENDTO = 2,      /* data, length: host; a: port; buffer, b: data */
    SWOOLE_CLIENT_SENDFILE = 3,    /* data: file; a: offset; b: length */
    SWOOLE_CLIENT_RECV = 4,        /* buffer, length; a: flags */
    SWOOLE_CLIENT_PACKAGE_LENGTH = 5, /* buffer, length: get_package_length */
    SWOOLE_CLIENT_SHUTDOWN = 6,    /* a: how */
    SWOOLE_CLIENT_CLOSE = 7,
    SWOOLE_CLIENT_SLEEP = 8,
    SWOOLE_CLIENT_WAKEUP = 9,
    SWOOLE_CLIENT_ENABLE_SSL = 11, /* enable_ssl_encrypt */
    SWOOLE_CLIENT_SSL_HANDSHAKE = 12,
    SWOOLE_CLIENT_SSL_VERIFY = 13, /* a: allow_self_signed */
    SWOOLE_CLIENT_SSL_WAIT = 14,   /* swoole_event_set(socket, SW_EVENT_WRITE) */
    SWOOLE_CLIENT_DESTROY = 15,    /* delete the timer, the length callable and the client */
    SWOOLE_CLIENT_CALLABLE = 17,   /* settings: sw_callable_create, into buffer */
    SWOOLE_CLIENT_CALLABLE_FREE = 18, /* cli: the callable */
    SWOOLE_CLIENT_INVOKE = 19,     /* cli: the callable; settings, a: argv, argc; buffer: a zval * for the result, or null; result: success */
    SWOOLE_CLIENT_CONVERT_FD = 20, /* settings: php_swoole_convert_to_fd */
};
struct swoole_client_call {
    void *cli;
    int op;
    const char *data;
    size_t length;
    char *buffer;
    int64_t a;
    int64_t b;
    double timeout;
    const struct _zval_struct *settings;
    int64_t result;
};
void swoole_client_run(void *call);

/* Swoole\NameResolver\Context and the swoole_name_resolver_* functions
 * (crates/phpclients/src/resolver.rs). */
void swr_v1_phpclients_name_resolver_minit(void);
/* The PHP resolver's lookup of name; writes a non-empty result through assign
 * (out is a std::string), then raises a bailout its PHP code caused. */
void swr_v1_phpclients_name_resolver_lookup(struct _zval_struct *resolver,
                                            const char *name,
                                            size_t length,
                                            void *context,
                                            void *out,
                                            void (*assign)(void *out, const char *text, size_t length));
/* Defined by ext-src/swoole_name_resolver.cc. context is a NameResolver::Context *. */
void swoole_name_resolver_php_add(struct _zval_struct *resolver);
struct _zval_struct *swoole_name_resolver_php_remove(bool (*same)(struct _zval_struct *kept,
                                                                  struct _zval_struct *resolver),
                                                     struct _zval_struct *resolver);
struct swoole_name_resolver_lookup_call {
    const char *name;
    size_t length;
    void *context;
    void *out;
    void (*assign)(void *out, const char *text, size_t length);
};
/* swoole_name_resolver_lookup over a swoole_name_resolver_lookup_call; a
 * bailout::trap body. */
void swoole_name_resolver_lookup_run(void *call);
void *swoole_name_resolver_context_new(void);
void swoole_name_resolver_context_free(void *context);
/* *context = {}, with with_port. */
void swoole_name_resolver_context_reset(void *context, bool with_port);
void swoole_name_resolver_context_set(void *context, int64_t type, bool with_port);
struct _zval_struct *swoole_name_resolver_context_get(void *context, bool *with_port);
void swoole_name_resolver_context_settle(void *context, struct _zval_struct *cluster, bool is_final);
void swoole_name_resolver_context_release(void *context);

#ifdef __cplusplus
}
#endif
#endif
