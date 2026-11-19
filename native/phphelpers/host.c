// SPDX-License-Identifier: Apache-2.0
// Compatibility derived from Swoole v6.2.4 ext-src/php_swoole.cc (Swoole contributors).
// What crates/phpswoole can't say in Rust: see contract.md.
#include "php_swoole.h"
#include "php_swoole_library.h"
#include "ext/pcre/php_pcre.h"
#include <openssl/opensslv.h>
#include <stddef.h>
#ifdef SW_USE_CURL
#include <curl/curl.h>
#endif
#ifdef SW_HAVE_ZLIB
#include <zlib.h>
#endif
#ifdef SW_HAVE_ZSTD
#include <zstd.h>
#endif
#ifdef SW_USE_CARES
#include <ares.h>
#endif

// MINFO's version rows: the headers' macros, or the linked library's answer.
struct swr_phpswoole_versions {
    const char *built, *openssl, *curl, *cares, *zlib, *zstd;
};

void swr_phpswoole_versions(struct swr_phpswoole_versions *info) {
    info->built = __DATE__ " " __TIME__;
    info->openssl = OPENSSL_VERSION_TEXT;
#ifdef SW_USE_CURL
    info->curl = curl_version_info(CURLVERSION_NOW)->version;
#endif
#ifdef SW_USE_CARES
    info->cares = ares_version(NULL);
#endif
#ifdef ZLIB_VERSION
    info->zlib = ZLIB_VERSION;
#else
    info->zlib = "enabled";
#endif
#ifdef ZSTD_VERSION_NUMBER
    info->zstd = ZSTD_VERSION_STRING;
#else
    info->zstd = "enabled";
#endif
}

// Rust stores Zend's module globals in phpswoole::module::Globals.
_Static_assert(sizeof(zend_swoole_globals) == 48, "Globals size");
_Static_assert(_Alignof(zend_swoole_globals) == 8, "Globals alignment");
#define CHECK_OFFSET(field, offset) _Static_assert(offsetof(zend_swoole_globals, field) == offset, #field)
CHECK_OFFSET(display_errors, 0);
CHECK_OFFSET(cli, 1);
CHECK_OFFSET(use_shortname, 2);
CHECK_OFFSET(enable_preemptive_scheduler, 3);
CHECK_OFFSET(enable_library, 4);
CHECK_OFFSET(enable_fiber_mock, 5);
CHECK_OFFSET(blocking_detection, 6);
CHECK_OFFSET(blocking_threshold, 8);
CHECK_OFFSET(profile, 16);
CHECK_OFFSET(leak_detection, 17);
CHECK_OFFSET(socket_buffer_size, 24);
CHECK_OFFSET(req_status, 32);
CHECK_OFFSET(in_autoload, 40);
#undef CHECK_OFFSET

// SWOOLE_G's storage: this thread's module globals.
zend_swoole_globals *swr_phpswoole_globals(void) {
#ifdef ZTS
    return TSRMG_BULK(swoole_globals_id, zend_swoole_globals *);
#else
    return &swoole_globals;
#endif
}

// MINIT: pcre.jit with swoole on macOS dumps core; upstream turns it off there.
void swr_phpswoole_pcre_jit_off(void) {
#if defined(PHP_PCRE_VERSION) && defined(HAVE_PCRE_JIT_SUPPORT) && __MACH__ && !defined(SW_DEBUG)
    PCRE_G(jit) = 0;
#endif
}
