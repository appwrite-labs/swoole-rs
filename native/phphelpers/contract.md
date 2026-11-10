# native/phphelpers/host.c

The C left over from upstream's `php_swoole.cc` after crates/phpswoole took the
module host. Each item stays C for a reason Rust can't remove:

- **`get_module()`** (hybrid only, under `COMPILE_DL_SWOOLE`). PHP's `dlopen`
  looks this symbol up in `swoole.so`. The hybrid links the Rust engine as a
  static archive, and the linker keeps only the archive members something
  references, so an exported Rust `get_module` would be dropped. A directly linked
  object keeps it. The Cargo-built engine exports it from crates/ext instead.
- **`swr_phpswoole_versions()`**. MINFO's version rows are header macros
  (`OPENSSL_VERSION_TEXT`, `ZLIB_VERSION`, `ZSTD_VERSION_STRING`, `__DATE__`) or
  calls into libraries only C links (curl, c-ares). The macros exist only
  where a C compiler reads the build's headers.
- **`swr_phpswoole_pcre_jit_off()`**. `PCRE_G(jit)` is ext/pcre's module global,
  whose layout and TSRM accessor change with PHP's build. Its guard
  (`HAVE_PCRE_JIT_SUPPORT`, `SW_DEBUG`) comes from the same headers.
- **`php_swoole_load_library()`**. The generated `php_swoole_library.h` (the pin's,
  owned in native/swoole-src) defines it, with the PHP sources it evaluates as C string literals and a
  `zend_compile_string` swap built from Zend's macros. This file includes the
  header once, and crates/phpswoole's RINIT calls the function by name.

Everything else from `php_swoole.cc` is in Rust: the module globals, the MINIT,
MSHUTDOWN, RINIT, RSHUTDOWN and MINFO lifecycle, the ssh2, ftp and stdext
handler tables (cargo features `ssh2`, `ftp` and `stdext`), ext/sockets' helpers
(`sockets`), and the DNS cache refresh time.
