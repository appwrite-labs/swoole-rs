/* Swoole\Runtime (Rust, crates/phpruntime): the class and SWOOLE_HOOK_*
 * constants, the hook flags, PHPCoroutine::enable_hook's family transitions,
 * the function-table hooks and the sleep family's replacements (their
 * zif_swoole_* handlers). C++ lends what it alone reaches: the stream
 * transports and wrappers, the PDO drivers, Swoole's coroutine state,
 * callables and logger, and the handlers that stay C++. Methods and functions
 * that can run PHP code go through C++ trampolines, which re-raise a bailout
 * the swr_v1_phpruntime_* bodies below report. */
#ifndef SWOOLE_RS_PHPRUNTIME_H
#define SWOOLE_RS_PHPRUNTIME_H
#include <stddef.h>
#include <stdint.h>
#ifdef __cplusplus
extern "C" {
#endif
struct _zend_function_entry;
struct _zend_class_entry;
struct _zend_execute_data;
struct _zval_struct;
struct _zend_fcall_info_cache;
typedef void (*swr_v1_phpruntime_handler)(struct _zend_execute_data *execute_data, struct _zval_struct *return_value);
struct swr_v1_phpruntime_ops {
    uint32_t features;
    struct _zend_class_entry *exception;
    uint8_t (*in_coroutine)(void);
    int32_t (*coroutine_sleep)(double seconds);
    void (*set_hook_flags)(uint32_t flags);
    uint32_t (*get_hook_flags)(void);
    uint8_t (*enable_library)(void);
    uint8_t (*main_thread)(void);
    uint32_t (*active_threads)(void);
    void (*call_hook)(uint8_t after, uint32_t *flags);
    int32_t (*transport)(uint32_t index, uint8_t hook);
    void (*stream_ops)(uint8_t stdio, uint8_t hook);
    void (*pdo)(uint32_t flag, uint8_t blocking);
    void *(*callable)(struct _zval_struct *name);
    void (*callable_free)(void *callable);
    struct _zend_fcall_info_cache *(*callable_fcc)(void *callable);
    void (*log_warning)(const char *message);
    void (*fatal_error)(int32_t level, const char *message);
    void (*error)(int32_t level, const char *message);
    swr_v1_phpruntime_handler user_func;
    swr_v1_phpruntime_handler stream_select;
    swr_v1_phpruntime_handler stream_socket_pair;
    swr_v1_phpruntime_handler proc_open;
    swr_v1_phpruntime_handler proc_close;
    swr_v1_phpruntime_handler proc_get_status;
    swr_v1_phpruntime_handler proc_terminate;
    swr_v1_phpruntime_handler gethostbyname;
    swr_v1_phpruntime_handler exit;
    swr_v1_phpruntime_handler putenv;
};
void swr_v1_phpruntime_minit(const struct _zend_function_entry *methods,
                             int32_t module_number,
                             const struct swr_v1_phpruntime_ops *ops);
/* enableCoroutine (min 0) and setHookFlags (min 1) after the CLI check: 1 if a
 * bailout was trapped. */
uint8_t swr_v1_phpruntime_set_hook_flags(struct _zend_execute_data *frame, struct _zval_struct *value, uint32_t min);
/* PHPCoroutine::enable_hook: bit 0 applied, bit 1 a trapped bailout. */
uint8_t swr_v1_phpruntime_enable_hook(uint32_t flags);
void swr_v1_phpruntime_unsafe_functions(uint8_t disable);
/* RINIT: 1 on the main thread (C++ then saves the original transports). */
uint8_t swr_v1_phpruntime_rinit(void);
/* RSHUTDOWN: 1 if a bailout was trapped. */
uint8_t swr_v1_phpruntime_rshutdown(void);
swr_v1_phpruntime_handler swr_v1_phpruntime_original_handler(const char *name, size_t length);
/* swoole_user_func_handler: 1 if the call bailed out. */
uint8_t swr_v1_phpruntime_user_func(struct _zend_execute_data *frame, struct _zval_struct *value);
#ifdef __cplusplus
}
#endif
#endif
