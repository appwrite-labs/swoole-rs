// SPDX-License-Identifier: Apache-2.0
// Compatibility derived from Swoole v6.2.4 ext-src/php_swoole_cxx.cc, php_swoole.cc and
// swoole_runtime.cc (Swoole contributors).
// What crates/phphelpers, crates/phpswoole and crates/phpruntime need of C++ that stable
// Rust can't express: zend::Callable and zend::function::call (C++ members other C++
// files call), the module globals and get_module(), and the ops tables whose entries
// configure's #ifdefs decide (the optional drivers, ssh2, ftp, stdext, Swoole\Thread,
// the PDO hooks) or that reach C++ objects (Socket's defaults, the logger, SwooleG).
#include "php_swoole_cxx.h"
#include "swoole_rs_phphelpers.h"
#include "swoole_rs_phpruntime.h"
#include "swoole_rs_zend_guard.h"
#include "swoole_coroutine_system.h"
#include "swoole_curl_interface.h"
#include "thirdparty/php/standard/proc_open.h"

using swoole::Coroutine;
using swoole::network::Socket;
static_assert(SUCCESS == 0, "Zend SUCCESS ABI");

SW_API zend_string **sw_zend_known_strings = nullptr;

namespace zend {
namespace function {
bool call(zend_fcall_info_cache *cache, uint32_t argc, zval *argv, zval *retval, const bool coroutine) {
    int called = call_trapped(cache, argc, argv, retval, coroutine);
    if (called < 0) {
        swr_zend_guard_resume();
    }
    return called > 0;
}
Variable call(const std::string &name, int argc, zval *argv) {
    Variable retval;
    swr_v1_phphelpers_named_call(name.data(), name.size(), argc, argv, retval.ptr());
    return retval;
}
}  // namespace function

Callable::Callable(zval *value) : owned(swr_v1_phphelpers_callable_new(value, sw_callable_warning)) {}
Callable::~Callable() { swr_v1_phphelpers_callable_free(owned); }
uint32_t Callable::refcount() const { return swr_v1_phphelpers_callable_refs(owned); }
}  // namespace zend

// The module host. crates/phpswoole owns the module: its entry, dependencies, MINIT,
// MSHUTDOWN, RINIT, RSHUTDOWN and MINFO, fatal_error and the backtrace hook. C++ keeps
// the module globals and lends, as swr_v1_phpswoole_ops, what only configure's #ifdefs
// or C++ can reach.
#include "php_swoole_library.h"
#include "php_swoole_thread.h"
#include "swoole_rs_locks.h"
#include "swoole_rs_native_effect.h"
#include "swoole_rs.h"
#include "swoole_rs_phpswoole_info.h"
#include "swoole_server.h"
#include "swoole_http2.h"
BEGIN_EXTERN_C()
#include "ext/pcre/php_pcre.h"
END_EXTERN_C()
#ifdef SW_USE_CURL
#include <curl/curl.h>
#endif
#ifdef SW_HAVE_ZLIB
#include <zlib.h>
#endif
#ifdef SW_HAVE_BROTLI
#include <brotli/encode.h>
#include <brotli/decode.h>
#endif
#ifdef SW_HAVE_ZSTD
#include <zstd.h>
#endif
#ifdef SW_USE_CARES
#include <ares.h>
#endif
#ifdef SW_USE_PGSQL
extern "C" void swoole_libpq_version(char *buf, size_t len);
#endif
#ifdef SW_STDEXT
#include "php_swoole_stdext.h"
#endif
#ifdef SW_HAVE_SSH2LIB
#include "php_swoole_ssh2_def.h"
#endif
#ifdef SW_HAVE_FTP
#include "php_swoole_ftp_def.h"
#endif
static_assert(SWR_ABI_VERSION == 1, "unsupported Rust bridge ABI header");

ZEND_DECLARE_MODULE_GLOBALS(swoole)
#if defined(COMPILE_DL_SWOOLE) && defined(ZTS)
ZEND_TSRMLS_CACHE_DEFINE()
#endif

// crates/phpswoole mirrors zend_swoole_globals: its defaults, RINIT and RSHUTDOWN use the fields.
static_assert(offsetof(zend_swoole_globals, cli) == 1 && offsetof(zend_swoole_globals, enable_library) == 4 &&
                  offsetof(zend_swoole_globals, blocking_threshold) == 8 &&
                  offsetof(zend_swoole_globals, socket_buffer_size) == 24 &&
                  offsetof(zend_swoole_globals, req_status) == 32 &&
                  offsetof(zend_swoole_globals, in_autoload) == 40 && sizeof(zend_swoole_globals) == 48,
              "crates/phpswoole Globals layout");

SW_API bool php_swoole_is_enable_coroutine() {
    return sw_server() ? sw_server()->is_enable_coroutine() : SwooleG.enable_coroutine;
}

void sw_php_exit(int status) {
    EG(exit_status) = status;
#ifdef SW_THREAD
    php_swoole_thread_bailout();
#else
    zend_bailout();
#endif
}

// zend-sys's PHP-version hooks (crates/zend-sys/src/shim_host.c).
extern "C" void swr_zend_after_fork(void *);

// Native lends only handler addresses; Rust owns names, aliases and arginfo.
#ifdef SW_STDEXT
static const zif_handler swoole_stdext_handlers[] = {
    ZEND_FN(swoole_call_array_method),
    ZEND_FN(swoole_call_string_method),
    ZEND_FN(swoole_call_stream_method),
    ZEND_FN(swoole_array_search),
    ZEND_FN(swoole_array_contains),
    ZEND_FN(swoole_array_join),
    ZEND_FN(swoole_array_key_exists),
    ZEND_FN(swoole_array_map),
    ZEND_FN(swoole_str_split),
    ZEND_FN(swoole_parse_str),
    ZEND_FN(swoole_hash),
    ZEND_FN(swoole_typed_array),
    ZEND_FN(swoole_array_is_typed),
    ZEND_FN(swoole_str_is_empty),
    ZEND_FN(swoole_array_is_empty),
    ZEND_FN(swoole_str_match),
    ZEND_FN(swoole_str_match_all),
    ZEND_FN(swoole_str_json_decode),
    ZEND_FN(swoole_str_json_decode_to_object),
    ZEND_FN(swoole_str_replace),
    ZEND_FN(swoole_str_ireplace),
    ZEND_FN(swoole_array_replace_str),
    ZEND_FN(swoole_array_ireplace_str),
};
#else
static const zif_handler *swoole_stdext_handlers = nullptr;
#endif
#ifdef SW_HAVE_SSH2LIB
static const zif_handler swoole_ssh2_handlers[] = {
    ZEND_FN(ssh2_connect),
    ZEND_FN(ssh2_disconnect),
    ZEND_FN(ssh2_methods_negotiated),
    ZEND_FN(ssh2_fingerprint),
    ZEND_FN(ssh2_auth_none),
    ZEND_FN(ssh2_auth_password),
    ZEND_FN(ssh2_auth_pubkey_file),
    ZEND_FN(ssh2_auth_pubkey),
    ZEND_FN(ssh2_auth_hostbased_file),
    ZEND_FN(ssh2_forward_listen),
    ZEND_FN(ssh2_forward_accept),
    ZEND_FN(ssh2_shell),
    ZEND_FN(ssh2_shell_resize),
    ZEND_FN(ssh2_exec),
    ZEND_FN(ssh2_tunnel),
    ZEND_FN(ssh2_scp_recv),
    ZEND_FN(ssh2_scp_send),
    ZEND_FN(ssh2_fetch_stream),
    ZEND_FN(ssh2_send_eof),
    ZEND_FN(ssh2_sftp),
    ZEND_FN(ssh2_sftp_rename),
    ZEND_FN(ssh2_sftp_unlink),
    ZEND_FN(ssh2_sftp_mkdir),
    ZEND_FN(ssh2_sftp_rmdir),
    ZEND_FN(ssh2_sftp_chmod),
    ZEND_FN(ssh2_sftp_stat),
    ZEND_FN(ssh2_sftp_lstat),
    ZEND_FN(ssh2_sftp_symlink),
    ZEND_FN(ssh2_sftp_readlink),
    ZEND_FN(ssh2_sftp_realpath),
    ZEND_FN(ssh2_publickey_init),
    ZEND_FN(ssh2_publickey_add),
    ZEND_FN(ssh2_publickey_remove),
    ZEND_FN(ssh2_publickey_list),
    ZEND_FN(ssh2_auth_agent),
};
#else
static const zif_handler *swoole_ssh2_handlers = nullptr;
#endif
#ifdef SW_HAVE_FTP
static const zif_handler swoole_ftp_handlers[] = {
    ZEND_FN(ftp_connect),
#ifdef SW_HAVE_FTP_SSL
    ZEND_FN(ftp_ssl_connect),
#else
    nullptr,
#endif
    ZEND_FN(ftp_login),
    ZEND_FN(ftp_pwd),
    ZEND_FN(ftp_cdup),
    ZEND_FN(ftp_chdir),
    ZEND_FN(ftp_exec),
    ZEND_FN(ftp_raw),
    ZEND_FN(ftp_mkdir),
    ZEND_FN(ftp_rmdir),
    ZEND_FN(ftp_chmod),
    ZEND_FN(ftp_alloc),
    ZEND_FN(ftp_nlist),
    ZEND_FN(ftp_rawlist),
    ZEND_FN(ftp_mlsd),
    ZEND_FN(ftp_systype),
    ZEND_FN(ftp_fget),
    ZEND_FN(ftp_nb_fget),
    ZEND_FN(ftp_pasv),
    ZEND_FN(ftp_get),
    ZEND_FN(ftp_nb_get),
    ZEND_FN(ftp_nb_continue),
    ZEND_FN(ftp_fput),
    ZEND_FN(ftp_nb_fput),
    ZEND_FN(ftp_put),
    ZEND_FN(ftp_append),
    ZEND_FN(ftp_nb_put),
    ZEND_FN(ftp_size),
    ZEND_FN(ftp_mdtm),
    ZEND_FN(ftp_rename),
    ZEND_FN(ftp_delete),
    ZEND_FN(ftp_site),
    ZEND_FN(ftp_close),
    ZEND_FN(ftp_close),
    ZEND_FN(ftp_set_option),
    ZEND_FN(ftp_get_option),
};
#else
static const zif_handler *swoole_ftp_handlers = nullptr;
#endif

static const swr_v1_phpswoole_ops phpswoole_ops = {
    [] { return (int32_t) swoole_get_last_error(); },
    [](int32_t code) { swoole_set_last_error(code); },
    [] { return (void *) ZEND_MODULE_GLOBALS_BULK(swoole); },
    [] {
        swoole_init();
        swoole_add_hook(SW_GLOBAL_HOOK_AFTER_FORK, swr_zend_after_fork, 0);
        swr_v1_core_bug_report_append("PHP_VERSION : " PHP_VERSION "\n");
    },
    [] {
        Socket::default_buffer_size = SWOOLE_G(socket_buffer_size);
        SwooleG.dns_cache_refresh_time = 60;

        // enable pcre.jit and use swoole extension on macOS will lead to core dump, disable it temporarily
#if defined(PHP_PCRE_VERSION) && defined(HAVE_PCRE_JIT_SUPPORT) && __MACH__ && !defined(SW_DEBUG)
        PCRE_G(jit) = 0;
#endif

        swr_v1_phphelpers_known_init(&sw_zend_known_strings);
    },
    [](int module_number) {
#ifdef SW_HAVE_SSH2LIB
        php_swoole_ssh2_minit(module_number);
#endif
#ifdef SW_HAVE_FTP
        PHP_MINIT(ftp)(MODULE_PERSISTENT, module_number);
#endif
    },
    [](int module_number) {
#ifdef SW_USE_PGSQL
        php_swoole_pgsql_minit(module_number);
#endif
#ifdef SW_USE_ODBC
        php_swoole_odbc_minit(module_number);
#endif
#ifdef SW_USE_ORACLE
        php_swoole_oracle_minit(module_number);
#endif
#ifdef SW_USE_SQLITE
        php_swoole_sqlite_minit(module_number);
#endif
#ifdef SW_USE_FIREBIRD
        php_swoole_firebird_minit(module_number);
#endif
#ifdef SW_THREAD
        php_swoole_thread_minit(module_number);
        swr_v1_phplocks_thread_atomic_minit();
        swr_v1_phplocks_thread_lock_minit([](const char *message) { php_swoole_fatal_error(E_ERROR, "%s", message); });
        swr_v1_phpthread_barrier_minit();
        swr_v1_phpthread_queue_minit();
        swr_v1_phpthread_map_minit();
        swr_v1_phpthread_arraylist_minit();
#endif
#ifdef SW_STDEXT
        php_swoole_stdext_minit(module_number);
#endif
    },
    [] {
#ifdef SW_USE_PGSQL
        php_swoole_pgsql_mshutdown();
#endif
#ifdef SW_USE_ORACLE
        php_swoole_oracle_mshutdown();
#endif
#ifdef SW_USE_SQLITE
        php_swoole_sqlite_mshutdown();
#endif
#ifdef SW_USE_FIREBIRD
        php_swoole_firebird_mshutdown();
#endif
#ifdef SW_HAVE_SSH2LIB
        php_swoole_ssh2_mshutdown();
#endif
    },
    [] {
#ifdef SW_USE_ORACLE
        php_swoole_oracle_rinit();
#endif
#ifdef SW_THREAD
        php_swoole_thread_rinit();
#endif
    },
    [] {
#ifdef SW_THREAD
        php_swoole_thread_rshutdown();
#endif
    },
    [] { swr_v1_phphelpers_known_free(&sw_zend_known_strings); },
    [] { swoole_clean(); },
    [](const char *dir) { swoole_set_task_tmpdir(dir); },
    [](void *) { php_swoole_load_library(); },
    [] { swoole_event_free(); },
    [] { return (uint32_t) SW_CPU_NUM; },
    [](int32_t level, const char *message, size_t length) { sw_logger()->put(level, message, length); },
    [](int32_t level, int32_t error, const char *message, size_t length) {
        swoole_set_last_error(error);
        if (level >= swoole_get_log_level() && !swoole_is_ignored_error(error)) {
            sw_logger()->put(level, message, length);
        }
    },
    [](int32_t code) { swoole_ignore_error(code); },
    [](zval *value, const char *buf, size_t length, HashTable *options) {
        zend::unserialize(value, buf, length, options);
    },
    [](zval *value, const char *str, size_t length, int64_t options, int64_t depth) {
        zend::json_decode(value, str, length, options, depth);
    },
    [](uint8_t on) { SwooleG.enable_signalfd = on; },
    [](uint8_t on) { SwooleG.enable_kqueue = on; },
    [](int64_t flags) { swoole_set_trace_flags(flags); },
    [](const char *file, size_t) { swoole_set_log_file(file); },
    [](int64_t level) { swoole_set_log_level(level); },
    [](const char *format, size_t) { sw_logger()->set_date_format(format); },
    [](uint8_t on) { sw_logger()->set_date_with_microseconds(on); },
    [](int64_t rotation) { sw_logger()->set_rotation(rotation); },
    [](uint8_t on) {
#if !defined(HAVE_BOOST_STACKTRACE) && !defined(HAVE_EXECINFO)
        zend_throw_exception(
            swoole_error_ce,
            "The `print_backtrace_on_error` option requires `boost stacktrace` or `execinfo.h` to be installed",
            SW_ERROR_OPERATION_NOT_SUPPORT);
#else
        swr_v1_core_set_print_backtrace_on_error(on);
#endif
    },
    [](const char *server, size_t length) { swoole_set_dns_server(std::string(server, length)); },
    [](int32_t kind, double seconds) {
        double *timeouts[] = {&Socket::default_dns_timeout,
                              &Socket::default_connect_timeout,
                              &Socket::default_read_timeout,
                              &Socket::default_write_timeout};
        *timeouts[kind] = seconds;
    },
    [](int64_t size) { Socket::default_buffer_size = size; },
    [](int32_t id, int64_t value) { swoole::http2::put_default_setting((enum swHttp2SettingId) id, value); },
    [](double seconds) { SwooleG.dns_cache_refresh_time = seconds; },
    [] { return SwooleG.dns_cache_refresh_time; },
#ifdef SW_THREAD
    [] { return (uint8_t) tsrm_is_main_thread(); },
#else
    [] { return (uint8_t) 1; },
#endif
    [] { return (uint8_t) (sw_reactor() != nullptr); },
    [] { Coroutine::get_current_safe(); },
    [] { php_swoole_check_reactor(); },
    [] { return Socket::default_dns_timeout; },
    [] { return (int64_t) swoole::Timer::get_absolute_msec(); },
    [](const char *domain, int32_t family, double timeout, void (*push)(void *, const char *), void *list) {
        for (auto &address : swoole::coroutine::dns_lookup(domain, family, timeout)) {
            push(list, address.c_str());
        }
    },
    [](int type, int module_number) -> int {
        const uint32_t rust_abi = swr_v1_abi_version();
        if (rust_abi != SWR_ABI_VERSION) {
            php_error_docref(nullptr, E_WARNING, "Rust bridge ABI mismatch: expected %u, got %u",
                             static_cast<unsigned>(SWR_ABI_VERSION), static_cast<unsigned>(rust_abi));
            return FAILURE;
        }
        // Publish the optional native transport before any requests or workers.
        static const swoole_rs::NativeEffectProvider native_effects{swr_zend_try, swr_zend_guard_resume};
        swoole_rs::register_native_effect_provider(&native_effects);
        SW_REGISTER_LONG_CONSTANT("SWOOLE_RS_ABI_VERSION", rust_abi);
        SW_REGISTER_LONG_CONSTANT("SWOOLE_RS_FEATURE_BITS", swr_v1_feature_bits());
        ZEND_INIT_MODULE_GLOBALS(swoole, swr_v1_phpswoole_init_globals, nullptr);
#ifdef ZTS
        swr_v1_phpswoole_register_ini(&swoole_globals_id, module_number, type);
#else
        swr_v1_phpswoole_register_ini(&swoole_globals, module_number, type);
#endif
        return SUCCESS;
    },
    [] {
        uint32_t flags = 0;
#ifdef SW_DEBUG
        flags |= SWR_PHPSWOOLE_DEBUG;
#endif
#ifdef SW_HAVE_COMPRESSION
        flags |= SWR_PHPSWOOLE_COMPRESSION;
#endif
#ifdef SW_HAVE_ZLIB
        flags |= SWR_PHPSWOOLE_ZLIB;
#endif
#ifdef SW_HAVE_BROTLI
        flags |= SWR_PHPSWOOLE_BROTLI;
#endif
#ifdef TLS1_1_VERSION
        flags |= SWR_PHPSWOOLE_TLS1_1;
#endif
#ifdef TLS1_2_VERSION
        flags |= SWR_PHPSWOOLE_TLS1_2;
#endif
#ifdef TLS1_3_VERSION
        flags |= SWR_PHPSWOOLE_TLS1_3;
#endif
#ifdef SW_SUPPORT_DTLS
        flags |= SWR_PHPSWOOLE_DTLS;
#endif
#ifdef HAVE_SSL3
        flags |= SWR_PHPSWOOLE_SSL3;
#endif
#ifdef SW_USE_IOURING
        flags |= SWR_PHPSWOOLE_IOURING;
#endif
#ifdef SW_STDEXT
        flags |= SWR_PHPSWOOLE_STDEXT;
#endif
#ifdef SW_THREAD
        flags |= SWR_PHPSWOOLE_THREAD;
#endif
        return flags;
    },
    [](swr_v1_phpswoole_info *info) {
        info->flags = swr_cpp_phpswoole_info_flags();
        info->abi = swr_v1_abi_version();
        info->features = swr_v1_feature_bits();
        info->version = SWOOLE_VERSION;
        info->built = __DATE__ " " __TIME__;
#ifdef OPENSSL_VERSION_TEXT
        info->openssl = OPENSSL_VERSION_TEXT;
#else
        info->openssl = "enabled";
#endif
#ifdef SW_USE_CURL
        info->curl = curl_version_info(CURLVERSION_NOW)->version;
#endif
#ifdef SW_USE_CARES
        info->cares = ares_version(nullptr);
#endif
#ifdef ZLIB_VERSION
        info->zlib = ZLIB_VERSION;
#else
        info->zlib = "enabled";
#endif
#ifdef SW_HAVE_BROTLI
        info->brotli_encoder = BrotliEncoderVersion();
        info->brotli_decoder = BrotliDecoderVersion();
#endif
#ifdef ZSTD_VERSION_NUMBER
        info->zstd = ZSTD_VERSION_STRING;
#else
        info->zstd = "enabled";
#endif
#ifdef SW_USE_PGSQL
        swoole_libpq_version(info->libpq, sizeof(info->libpq));
#endif
    },
    swoole_stdext_handlers,
    swoole_ssh2_handlers,
    swoole_ftp_handlers,
    [] { return (int64_t) Coroutine::get_socket_bound_cid(); },
    [] {
#ifdef SW_THREAD
        php_swoole_thread_bailout();
#endif
    },
    [](int32_t status) { swoole_exit(status); },
};

#ifdef COMPILE_DL_SWOOLE
extern "C" ZEND_DLEXPORT zend_module_entry *get_module() {
    return swr_v1_phpswoole_module(&phpswoole_ops);
}
#endif

extern "C" uint8_t swr_cpp_phpswoole_info_ssh2() {
    uint8_t bailed = 0;
#ifdef SW_HAVE_SSH2LIB
    zend_try { php_swoole_ssh2_minfo(); } zend_catch { bailed = 1; } zend_end_try();
#endif
    return bailed;
}

extern "C" uint8_t swr_cpp_phpswoole_info_ftp(void *module) {
    uint8_t bailed = 0;
#ifdef SW_HAVE_FTP
    zend_try { PHP_MINFO(ftp)(static_cast<zend_module_entry *>(module)); }
    zend_catch { bailed = 1; } zend_end_try();
#endif
    return bailed;
}

// Swoole\Runtime is Rust (crates/phpruntime); this is what C++ alone reaches for it.
// PHP's plain-files wrapper and its originals are thirdparty/php/streams/plain_wrapper.c's.
SW_EXTERN_C_BEGIN
#ifdef SW_USE_PGSQL
void swoole_pgsql_set_blocking(bool blocking);
#endif
#ifdef SW_USE_ODBC
void swoole_odbc_set_blocking(bool blocking);
#endif
#ifdef SW_USE_ORACLE
void swoole_oracle_set_blocking(bool blocking);
#endif
#ifdef SW_USE_SQLITE
void swoole_sqlite_set_blocking(bool blocking);
#endif
#ifdef SW_USE_FIREBIRD
void swoole_firebird_set_blocking(bool blocking);
#endif
#if defined(HAVE_PUTENV) && defined(SW_THREAD)
PHP_FUNCTION(swoole_putenv);
#endif
#if PHP_VERSION_ID >= 80400
PHP_FUNCTION(swoole_exit);
#endif
extern php_stream_wrapper sw_php_plain_files_wrapper;
SW_EXTERN_C_END

static void runtime_pdo(uint32_t flag, uint8_t blocking) {
    switch (flag) {
#ifdef SW_USE_PGSQL
    case SWR_HOOK_PDO_PGSQL:
        swoole_pgsql_set_blocking(blocking);
        break;
#endif
#ifdef SW_USE_ODBC
    case SWR_HOOK_PDO_ODBC:
        swoole_odbc_set_blocking(blocking);
        break;
#endif
#ifdef SW_USE_ORACLE
    case SWR_HOOK_PDO_ORACLE:
        swoole_oracle_set_blocking(blocking);
        break;
#endif
#ifdef SW_USE_SQLITE
    case SWR_HOOK_PDO_SQLITE:
        swoole_sqlite_set_blocking(blocking);
        break;
#endif
#ifdef SW_USE_FIREBIRD
    case SWR_HOOK_PDO_FIREBIRD:
        swoole_firebird_set_blocking(blocking);
        break;
#endif
    default:
        break;
    }
}

static const swr_v1_phpruntime_ops *runtime_ops() {
    static swr_v1_phpruntime_ops ops = {
        0
#ifdef SW_USE_PGSQL
            | SWR_HOOK_PDO_PGSQL
#endif
#ifdef SW_USE_ODBC
            | SWR_HOOK_PDO_ODBC
#endif
#ifdef SW_USE_ORACLE
            | SWR_HOOK_PDO_ORACLE
#endif
#ifdef SW_USE_SQLITE
            | SWR_HOOK_PDO_SQLITE
#endif
#ifdef SW_USE_FIREBIRD
            | SWR_HOOK_PDO_FIREBIRD
#endif
#ifdef SW_USE_CURL
            | SWR_HOOK_NATIVE_CURL
#endif
        ,
        nullptr,
        []() -> uint8_t { return Coroutine::get_current() != nullptr; },
        []() -> uint8_t { return SWOOLE_G(cli); },
        [](double seconds) -> int32_t { return swoole::coroutine::System::sleep(seconds); },
        swr_v1_phpcoroutines_set_hook_option,
        swr_v1_phpcoroutines_hook_flags,
        []() -> uint8_t { return SWOOLE_G(enable_library); },
        []() -> uint8_t { return sw_is_main_thread(); },
        []() -> uint32_t { return sw_active_thread_count(); },
        [](uint8_t after, uint32_t *flags) {
            auto type =
                (swGlobalHookType) (after ? PHP_SWOOLE_HOOK_AFTER_ENABLE_HOOK : PHP_SWOOLE_HOOK_BEFORE_ENABLE_HOOK);
            if (swoole_isset_hook(type)) {
                swoole_call_hook(type, flags);
            }
        },
        php_swoole_runtime_stream_ops,
        runtime_pdo,
        [](zval *name) -> void * { return sw_callable_create(name); },
        [](void *callable) { sw_callable_free(callable); },
        [](void *callable) -> void * { return static_cast<zend::Callable *>(callable)->dup(); },
        [](void *call) {
            auto *c = static_cast<swr_v1_runtime_callable_call *>(call);
            c->result = static_cast<zend::Callable *>(c->callable)->call(c->argc, c->argv, nullptr);
        },
        [](void *callable) { return static_cast<zend::Callable *>(callable)->ptr(); },
        [](const char *message) {
            if (SW_LOG_WARNING >= swoole_get_log_level()) {
                sw_logger()->put(SW_LOG_WARNING, message, strlen(message));
            }
        },
        [](int32_t level, const char *message) { php_swoole_fatal_error(level, "%s", message); },
        [](int32_t level, const char *message) { php_swoole_error(level, "%s", message); },
        PHP_FN(swoole_proc_open),
        PHP_FN(swoole_proc_close),
        PHP_FN(swoole_proc_get_status),
        PHP_FN(swoole_proc_terminate),
        PHP_FN(swoole_coroutine_gethostbyname),
#if PHP_VERSION_ID >= 80400
        PHP_FN(swoole_exit),
#else
        nullptr,
#endif
#if defined(HAVE_PUTENV) && defined(SW_THREAD)
        PHP_FN(swoole_putenv),
#else
        nullptr,
#endif
        [](void *arg) {
            auto *call = static_cast<swr_v1_phpruntime_ssl_call *>(arg);
            call->result = php_swoole_socket_set_ssl(static_cast<SocketImpl *>(call->socket), call->settings);
        },
        [](void *arg) {
            auto *call = static_cast<swr_v1_phpruntime_x509_call *>(arg);
            zval argv[1];
            ZVAL_STRINGL(&argv[0], call->pem, call->length);
            auto result = zend::function::call("openssl_x509_read", 1, argv);
            ZVAL_COPY(call->retval, &result.value);
            zval_dtor(&argv[0]);
        },
        [](void *) { Coroutine::get_current_safe(); },
        [](void *arg) {
            auto *call = static_cast<swr_v1_phpruntime_fd_call *>(arg);
            call->fd = php_swoole_convert_to_fd(call->value);
        },
        [] { php_swoole_check_reactor(); },
        swoole_strerror,
    };
    ops.exception = swoole_exception_ce;
    return &ops;
}

void php_swoole_runtime_minit(int module_number) {
    swr_v1_phpruntime_minit(module_number, runtime_ops());
#ifdef SW_USE_CURL
    swoole_native_curl_minit(module_number);
#endif
    swoole_proc_open_init(module_number);
    php_register_url_stream_wrapper(SW_ASYNC_FILE_PROTOCOL, &sw_php_plain_files_wrapper);
}

void php_swoole_runtime_mshutdown() {
#ifdef SW_USE_CURL
    swoole_native_curl_mshutdown();
#endif
}

bool php_swoole_call_original_handler(const char *name, size_t len, INTERNAL_FUNCTION_PARAMETERS) {
    auto ori_handler = php_swoole_get_original_handler(name, len);
    if (!ori_handler) {
        return false;
    }
    ori_handler(INTERNAL_FUNCTION_PARAM_PASSTHRU);
    return true;
}
