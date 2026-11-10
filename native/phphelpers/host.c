// SPDX-License-Identifier: Apache-2.0
// Compatibility derived from Swoole v6.2.4 ext-src/php_swoole.cc (Swoole contributors).
// What crates/phpswoole can't say in Rust: see contract.md.
#include "php_swoole.h"
#include "php_swoole_library.h"
#include "ext/pcre/php_pcre.h"
#include <openssl/opensslv.h>
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

zend_module_entry *swr_v1_phpswoole_module(void);

// The hybrid's swoole.so exports get_module() from here, an object it links directly;
// the Cargo-built one (crates/ext) exports it from Rust.
#ifdef COMPILE_DL_SWOOLE
ZEND_DLEXPORT zend_module_entry *get_module(void) {
    return swr_v1_phpswoole_module();
}
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

// MINIT: pcre.jit with swoole on macOS dumps core; upstream turns it off there.
void swr_phpswoole_pcre_jit_off(void) {
#if defined(PHP_PCRE_VERSION) && defined(HAVE_PCRE_JIT_SUPPORT) && __MACH__ && !defined(SW_DEBUG)
    PCRE_G(jit) = 0;
#endif
}
