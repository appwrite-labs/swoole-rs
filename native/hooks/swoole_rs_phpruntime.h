/* Swoole\Runtime (Rust, crates/phpruntime): the class and SWOOLE_HOOK_*
 * constants, the hook flags, PHPCoroutine::enable_hook's family transitions,
 * the function-table hooks and the sleep family's replacements (their
 * zif_swoole_* handlers), its methods and the hooked stream functions; each
 * raises a trapped bailout last. C++ lends what it alone reaches: the stream
 * wrappers, the PDO drivers, Swoole's coroutine state, callables and logger,
 * and the handlers that stay C++. */
#ifndef SWOOLE_RS_PHPRUNTIME_H
#define SWOOLE_RS_PHPRUNTIME_H
#include <stdbool.h>
#include <stddef.h>
#include <stdint.h>
#include <sys/socket.h>
#include <sys/time.h>
#include <sys/types.h>
#include <sys/uio.h>
#ifdef __cplusplus
extern "C" {
#endif
struct _zend_class_entry;
struct _zend_execute_data;
struct _zval_struct;
struct _zend_fcall_info_cache;
struct swr_v1_runtime_callable_call {
    void *callable;
    uint32_t argc;
    struct _zval_struct *argv;
    struct _zval_struct *retval;
    bool from_loop;
    bool result;
};
typedef void (*swr_v1_phpruntime_handler)(struct _zend_execute_data *execute_data, struct _zval_struct *return_value);
struct swr_v1_phpruntime_ops {
    uint32_t features;
    struct _zend_class_entry *exception;
    uint8_t (*in_coroutine)(void);
    int32_t (*coroutine_sleep)(double seconds);
    void (*set_hook_flags)(uint32_t flags);
    uint32_t (*get_hook_flags)(void);
    uint8_t (*main_thread)(void);
    uint32_t (*active_threads)(void);
    void (*stream_ops)(uint8_t stdio, uint8_t hook);
    void (*pdo)(uint32_t flag, uint8_t blocking);
    void *(*callable)(struct _zval_struct *name);
    void (*callable_free)(void *callable);
    void *(*callable_dup)(void *callable);
    /* call->result = callable->call(argc, argv, nullptr); a bailout goes on. */
    void (*callable_call)(void *call);
    struct _zend_fcall_info_cache *(*callable_fcc)(void *callable);
    void (*fatal_error)(int32_t level, const char *message);
    void (*error)(int32_t level, const char *message);
    swr_v1_phpruntime_handler proc_open;
    swr_v1_phpruntime_handler proc_close;
    swr_v1_phpruntime_handler proc_get_status;
    swr_v1_phpruntime_handler proc_terminate;
    swr_v1_phpruntime_handler gethostbyname;
    swr_v1_phpruntime_handler exit;
    swr_v1_phpruntime_handler putenv;
    /* Bodies for swr_zend_try: openssl_x509_read, Coroutine::get_current_safe()
     * and php_swoole_convert_to_fd. */
    void (*x509_read)(void *call);    /* struct swr_v1_phpruntime_x509_call */
    void (*coroutine_check)(void *unused);
    void (*convert_to_fd)(void *call); /* struct swr_v1_phpruntime_fd_call */
    void (*check_reactor)(void);
};
struct swr_v1_phpruntime_x509_call {
    const char *pem;
    size_t length;
    struct _zval_struct *retval;
};
struct swr_v1_phpruntime_fd_call {
    struct _zval_struct *value;
    int fd;
};
void swr_v1_phpruntime_minit(int32_t module_number,
                             const struct swr_v1_phpruntime_ops *ops);
/* RINIT: 1 on the main thread (the plain wrapper then saves the original stream ops). */
uint8_t swr_v1_phpruntime_rinit(void);
/* PHPCoroutine::enable_hook(flags), 0 disabling every hook; a trapped bailout is raised last. */
bool swr_v1_phpruntime_enable_hook(uint32_t flags);
/* The hook flags that select C++'s PDO drivers and native curl (crates/phpruntime's flags). */
enum {
    SWR_HOOK_NATIVE_CURL = 1u << 12,
    SWR_HOOK_PDO_PGSQL = 1u << 16,
    SWR_HOOK_PDO_ODBC = 1u << 17,
    SWR_HOOK_PDO_ORACLE = 1u << 18,
    SWR_HOOK_PDO_SQLITE = 1u << 19,
    SWR_HOOK_PDO_FIREBIRD = 1u << 20,
};

/* The coroutine socket streams: the "socket/coroutine" operations and
 * php_swoole_create_stream_from_socket. */
struct _php_stream;
struct _php_stream *swr_v1_phpruntime_stream_from_socket(int fd, int domain, int kind, int protocol);

/* Defined by thirdparty/php/streams/plain_wrapper.c, which keeps PHP's originals:
 * RINIT, and the stream_ops entry. */
void php_swoole_runtime_rinit(void);
void php_swoole_runtime_stream_ops(uint8_t stdio, uint8_t hook);

#ifdef __cplusplus
}
#endif
#endif
