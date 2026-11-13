// SPDX-License-Identifier: Apache-2.0
// Derived from Swoole v6.2.4 ext-src/php_swoole_private.h and php_swoole_cxx.h
// (Swoole contributors): what the extension's C++ (native/ and the vendored
// thirdparty) still takes from them. Symbols keep the linkage those headers gave.
#pragma once

#include "php_swoole.h"
#include "swoole_rs_phphelpers.h"
#include "swoole.h"
#include "swoole_api.h"
#include "swoole_coroutine_api.h"
#include "swoole_coroutine.h"
#include "swoole_coroutine_system.h"
#include "swoole_rs_coroutine_socket.h"
#include "swoole_protocol.h"
#include "swoole_util.h"

#include <string>
#include <utility>

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

#if PHP_VERSION_ID < 80200
#error "require PHP version 8.2 or later"
#elif PHP_VERSION_ID >= 80600
#error "require PHP version 8.5 or earlier"
#endif

enum php_swoole_req_status {
    PHP_SWOOLE_RINIT_BEGIN,
    PHP_SWOOLE_RINIT_END,
    PHP_SWOOLE_CALL_USER_SHUTDOWNFUNC_BEGIN,
    PHP_SWOOLE_RSHUTDOWN_BEGIN,
    PHP_SWOOLE_RSHUTDOWN_END,
};

// The curl hooks' exception extends this (SW_INIT_CLASS_ENTRY_EX pastes the names).
extern zend_class_entry *swoole_exception_ce;
extern zend_object_handlers swoole_exception_handlers;

// Defined in Rust.
int php_swoole_reactor_init();
zend_bool php_swoole_signal_isset_handler(int signo);

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
// The order matches crates/zend/src/host's known-string table.
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
