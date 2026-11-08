// SPDX-License-Identifier: Apache-2.0
// Derived from Swoole v6.2.4 ext-src/php_swoole_private.h and php_swoole_cxx.h
// (Swoole contributors): what the extension's C++ (native/ and the vendored
// thirdparty) still takes from them. Symbols keep the linkage those headers gave.
#pragma once

#include "php_swoole.h"
#include "swoole_rs_phphelpers.h"
#include "php_swoole_api.h"
#include "swoole.h"
#include "swoole_api.h"
#include "swoole_coroutine_api.h"
#include "swoole_coroutine.h"
#include "swoole_rs_phpcoroutines.h"
#include "swoole_coroutine_system.h"
#include "swoole_rs_coroutine_socket.h"
#include "swoole_protocol.h"
#include "swoole_util.h"

#include <string>
#include <utility>

#ifdef SW_HAVE_ZLIB
#include <zlib.h>
#endif

BEGIN_EXTERN_C()
#include <ext/date/php_date.h>
#include <ext/standard/url.h>
#include <ext/standard/info.h>
#include <ext/standard/php_array.h>
#include <ext/standard/php_var.h>
#include <ext/standard/basic_functions.h>
#include <ext/standard/php_http.h>

#define php_swoole_fatal_error(level, fmt_str, ...)                                                                    \
    swoole_set_last_error(SW_ERROR_PHP_FATAL_ERROR);                                                                   \
    php_error_docref(NULL, level, (const char *) (fmt_str), ##__VA_ARGS__)

// The error occurred at the PHP layer and no error code was set.
#define php_swoole_error(level, fmt_str, ...)                                                                          \
    swoole_set_last_error(SW_ERROR_PHP_RUNTIME_NOTICE);                                                                \
    if (SWOOLE_G(display_errors) || level == E_ERROR) php_error_docref(NULL, level, fmt_str, ##__VA_ARGS__)

// The error occurred in the core, which set the error code.
#define php_swoole_core_error(level, fmt_str, ...)                                                                     \
    if (SWOOLE_G(display_errors) || level == E_ERROR) php_error_docref(NULL, level, fmt_str, ##__VA_ARGS__)

#define php_swoole_sys_error(level, fmt_str, ...)                                                                      \
    swoole_set_last_error(errno);                                                                                      \
    if (SWOOLE_G(display_errors) || level == E_ERROR)                                                                  \
    php_error_docref(NULL, level, fmt_str ", Error: %s[%d]", ##__VA_ARGS__, strerror(errno), errno)

#if PHP_VERSION_ID < 80200
#error "require PHP version 8.2 or later"
#elif PHP_VERSION_ID >= 80600
#error "require PHP version 8.5 or earlier"
#endif

#if defined(SW_HAVE_ZLIB) || defined(SW_HAVE_BROTLI) || defined(SW_HAVE_ZSTD)
#define SW_HAVE_COMPRESSION
#endif

#ifdef SW_SOCKETS
#include "ext/sockets/php_sockets.h"
#define SWOOLE_SOCKETS_SUPPORT
#endif

#define SW_MAX_FIND_COUNT 100  // for swoole_server::connection_list
#define SW_ASYNC_FILE_PROTOCOL "async.file"

enum php_swoole_req_status {
    PHP_SWOOLE_RINIT_BEGIN,
    PHP_SWOOLE_RINIT_END,
    PHP_SWOOLE_CALL_USER_SHUTDOWNFUNC_BEGIN,
    PHP_SWOOLE_RSHUTDOWN_BEGIN,
    PHP_SWOOLE_RSHUTDOWN_END,
};

enum php_swoole_hook_type {
    PHP_SWOOLE_HOOK_BEFORE_ENABLE_HOOK = SW_GLOBAL_HOOK_USER,
    PHP_SWOOLE_HOOK_AFTER_ENABLE_HOOK,
    PHP_SWOOLE_HOOK_BEFORE_REQUEST,
    PHP_SWOOLE_HOOK_AFTER_RESPONSE,
};

extern zend_class_entry *swoole_server_ce;
extern zend_object_handlers swoole_server_handlers;
extern zend_class_entry *swoole_server_port_ce;
extern zend_class_entry *swoole_exception_ce;
extern zend_object_handlers swoole_exception_handlers;
extern zend_class_entry *swoole_error_ce;

// Module hooks Rust's MINIT/RINIT/MSHUTDOWN sequence calls, defined in C++ here.
void php_swoole_event_minit(int module_number);
void php_swoole_timer_minit(int module_number);
void php_swoole_runtime_minit(int module_number);
void php_swoole_server_minit(int module_number);
void php_swoole_runtime_mshutdown();
// The optional drivers' hooks, which native/phphelpers/module.cc calls.
#ifdef SW_USE_PGSQL
void php_swoole_pgsql_minit(int module_number);
void php_swoole_pgsql_mshutdown();
#endif
#ifdef SW_USE_ODBC
int php_swoole_odbc_minit(int module_id);
#endif
#ifdef SW_USE_ORACLE
void php_swoole_oracle_minit(int module_number);
void php_swoole_oracle_rinit();
void php_swoole_oracle_mshutdown();
#endif
#ifdef SW_USE_SQLITE
void php_swoole_sqlite_minit(int module_number);
void php_swoole_sqlite_mshutdown();
#endif
#ifdef SW_USE_FIREBIRD
void php_swoole_firebird_minit(int module_number);
void php_swoole_firebird_mshutdown();
#endif

// Defined in Rust.
int php_swoole_reactor_init();
void php_swoole_register_shutdown_function(const char *function);
int php_swoole_convert_to_fd(zval *zsocket);
zend_bool php_swoole_signal_isset_handler(int signo);

// A Socket over fd, which it takes (native/phphelpers); false if it can't import fd.
bool swr_cpp_phpthread_php_socket_create(zval *return_value, int fd);

#define SW_Z_SOCKET_P(zsocket) Z_SOCKET_P(zsocket)

static sw_inline zval *sw_zval_dup(zval *val) {
    return swr_v1_phphelpers_zval_dup(val);
}

#ifdef SWOOLE_SOCKETS_SUPPORT
static inline bool sw_zval_is_php_socket(zval *val) {
    return instanceof_function(Z_OBJCE_P(val), socket_ce);
}
#endif

static inline bool sw_is_main_thread() {
#ifdef ZTS
    return tsrm_is_main_thread();
#else
    return true;
#endif
}

#ifdef SW_THREAD
size_t sw_active_thread_count(void);
#else
static inline size_t sw_active_thread_count() {
    return 1;
}
#endif

void sw_php_exit(int status);

#define SW_REGISTER_LONG_CONSTANT(name, value) REGISTER_LONG_CONSTANT(name, value, CONST_CS | CONST_PERSISTENT)

#define php_swoole_array_length(zarray) zend_hash_num_elements(Z_ARRVAL_P(zarray))

#define SW_INIT_CLASS_ENTRY_BASE(module, namespace_name, short_name, methods, parent_ce)                               \
    do {                                                                                                               \
        zend_class_entry _##module##_ce = {};                                                                          \
        INIT_CLASS_ENTRY(_##module##_ce, namespace_name, methods);                                                     \
        module##_ce = zend_register_internal_class_ex(&_##module##_ce, parent_ce);                                     \
        if (short_name) SW_CLASS_ALIAS_SHORT_NAME(short_name, module);                                                 \
    } while (0)

#define SW_INIT_CLASS_ENTRY_EX(module, namespace_name, short_name, methods, parent_module)                             \
    SW_INIT_CLASS_ENTRY_BASE(module, namespace_name, short_name, methods, parent_module##_ce);                         \
    memcpy(&module##_handlers, &parent_module##_handlers, sizeof(zend_object_handlers))

#define SW_CLASS_ALIAS(name, module)                                                                                   \
    do {                                                                                                               \
        if (name) {                                                                                                    \
            swr_v1_phphelpers_class_alias(ZEND_STRL(name), module##_ce);                                               \
        }                                                                                                              \
    } while (0)

#define SW_CLASS_ALIAS_SHORT_NAME(short_name, module)                                                                  \
    do {                                                                                                               \
        if (SWOOLE_G(use_shortname)) {                                                                                 \
            SW_CLASS_ALIAS(short_name, module);                                                                        \
        }                                                                                                              \
    } while (0)

// Check for an exception immediately: the caller may not return to the VM (an event loop).
static sw_inline int sw_zend_call_function_ex2(
    zval *function_name, zend_fcall_info_cache *fci_cache, uint32_t param_count, zval *params, zval *retval) {
    return swr_v1_phphelpers_call(function_name, fci_cache, param_count, params, retval, true,
        [](const char *message) { php_swoole_fatal_error(E_WARNING, "%s", message); });
}

static sw_inline int php_swoole_check_reactor() {
    if (SWOOLE_G(req_status) == PHP_SWOOLE_RSHUTDOWN_BEGIN) {
        return -1;
    }
    if (sw_unlikely(!sw_reactor())) {
        return php_swoole_reactor_init() == SW_OK ? 1 : -1;
    } else {
        return 0;
    }
}

END_EXTERN_C()

// clang-format off
// The order matches crates/phphelpers' known-string table.
#define SW_ZEND_KNOWN_STRINGS(_) \
    _(SW_ZEND_STR_SETTING,                  "setting") \
    _(SW_ZEND_STR_FD,                       "fd") \
    _(SW_ZEND_STR_SOCK,                     "sock") \
    _(SW_ZEND_STR_PIPE,                     "pipe") \
    _(SW_ZEND_STR_IN_COROUTINE,             "in_coroutine") \
    _(SW_ZEND_STR_SOCKET,                   "socket") \

typedef enum sw_zend_known_string_id {
#define _SW_ZEND_STR_ID(id, str) id,
SW_ZEND_KNOWN_STRINGS(_SW_ZEND_STR_ID)
#undef _SW_ZEND_STR_ID
    SW_ZEND_STR_LAST_KNOWN
} sw_zend_known_string_id;
// clang-format on

#define SW_ZSTR_KNOWN(idx) sw_zend_known_strings[idx]
extern zend_string **sw_zend_known_strings;

SW_API bool php_swoole_is_enable_coroutine();
SW_API zif_handler php_swoole_get_original_handler(const char *name, size_t len);
SW_API bool php_swoole_call_original_handler(const char *name, size_t len, INTERNAL_FUNCTION_PARAMETERS);

static inline bool php_swoole_is_fatal_error() {
    return PG(last_error_message) && (PG(last_error_type) & E_FATAL_ERRORS);
}

const swoole::Allocator *sw_zend_string_allocator();

static inline bool php_swoole_async(bool blocking, const std::function<void(void)> &fn) {
    uint32_t phase = 0;
    bool observation = false;
    for (;;) {
        switch (swr_v1_phphelpers_async(&phase, blocking, observation)) {
        case SWR_PHP_ASYNC_OBSERVE_COROUTINE: observation = swoole_coroutine_is_in(); break;
        case SWR_PHP_ASYNC_DISPATCH: return swoole::coroutine::async(fn);
        case SWR_PHP_ASYNC_DIRECT: fn(); return true;
        default: return false;
        }
    }
}

static inline void sw_callable_warning(const char *message) {
    php_swoole_fatal_error(E_WARNING, "%s", message);
}

namespace zend {
class Callable {
    SwrPhpCallable *owned;
    explicit Callable(SwrPhpCallable *handle) : owned(handle) {}
  public:
    Callable(zval *value);
    ~Callable();
    uint32_t refcount() const;
    zend_fcall_info_cache *ptr() { return swr_v1_phphelpers_callable_cache(owned); }
    bool ready() const { return swr_v1_phphelpers_callable_ready(owned); }
    Callable *dup() const { return new Callable(swr_v1_phphelpers_callable_copy(owned)); }
    bool call(uint32_t argc, zval *argv, zval *retval) { return swr_v1_phphelpers_callable_invoke(owned, argc, argv, retval); }
    static Callable *wrap(SwrPhpCallable *handle) { return handle ? new Callable(handle) : nullptr; }
};

namespace function {
// Event callbacks go through this so exceptions are handled.
bool call(zend_fcall_info_cache *fci_cache, uint32_t argc, zval *argv, zval *retval, const bool enable_coroutine);
// call() without re-raising: 1 called, 0 failed, -1 bailed out (the caller re-raises).
static inline int call_trapped(zend_fcall_info_cache *cache, uint32_t argc, zval *argv, zval *retval, bool coroutine) {
    return swr_v1_phphelpers_function_call(cache, argc, argv, retval, coroutine, swr_v1_phpcoroutines_create, sw_callable_warning);
}
}  // namespace function

struct Function {
    zend_fcall_info fci;
    zend_fcall_info_cache fci_cache;

    bool call(zval *retval, const bool enable_coroutine) {
        return function::call(&fci_cache, fci.param_count, fci.params, retval, enable_coroutine);
    }
};

void unserialize(zval *return_value, const char *buf, size_t buf_len, HashTable *options);
void json_decode(zval *return_value, const char *str, size_t str_len, zend_long options, zend_long zend_long);

// Returns a reference.
static inline zval *array_get(zval *arg, zend_ulong index) {
    return zend_hash_index_find(Z_ARRVAL_P(arg), index);
}
}  // namespace zend

// void * to match C callback function pointers.
static inline void sw_callable_free(void *ptr) {
    delete (zend::Callable *) ptr;
}

static inline zend::Callable *sw_callable_create(zval *zfn) {
    return zend::Callable::wrap(swr_v1_phphelpers_callable_factory(zfn, nullptr, true, swoole_exception_ce, sw_callable_warning));
}
