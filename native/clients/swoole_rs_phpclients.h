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

/* Defined by native/clients/client.cc: network::Client's PHP side. */
enum { SWOOLE_CLIENT_ERROR = 0, SWOOLE_CLIENT_ERROR_FATAL = 1, SWOOLE_CLIENT_ERROR_SYS = 2, SWOOLE_CLIENT_ERROR_CORE = 3 };
/* php_swoole_error, _fatal_error, _sys_error or _core_error(level, "%s", message). */
void swoole_client_php_error(int kind, int level, const char *message);
/* A Socket over a dup of the client's fd into `zsocket`; false without ext/sockets. */
bool swoole_client_export_socket(void *cli, struct _zval_struct *zsocket);

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

/* Swoole\NameResolver\Context and the swoole_name_resolver_* functions are
 * crates/phpclients/src/resolver.rs; its MINIT is php_swoole_name_resolver_minit. */
struct swoole_name_resolver_lookup_call {
    const char *name;
    size_t length;
    void *context;
    void *out;
    void (*assign)(void *out, const char *text, size_t length);
};
/* swoole_name_resolver_lookup over a swoole_name_resolver_lookup_call; a
 * bailout::trap body. Defined by native/clients/dns.cc. */
void swoole_name_resolver_lookup_run(void *call);

#ifdef __cplusplus
}
#endif
#endif
