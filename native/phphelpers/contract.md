# native/phphelpers/host.c

The C left over from upstream's `php_swoole.cc` after crates/phpswoole took the
module host. Each item stays C for a reason Rust can't remove:

- **`swr_phpswoole_versions()`**. MINFO's version rows are header macros
  (`OPENSSL_VERSION_TEXT`, `ZLIB_VERSION`, `ZSTD_VERSION_STRING`, `__DATE__`) or
  calls into libraries only C links (curl, c-ares). The macros exist only
  where a C compiler reads the build's headers.
- **`swr_phpswoole_pcre_jit_off()`**. `PCRE_G(jit)` is ext/pcre's module global,
  whose layout and TSRM accessor change with PHP's build. Its guard
  (`HAVE_PCRE_JIT_SUPPORT`, `SW_DEBUG`) comes from the same headers.
- **`swr_phpswoole_globals()`**. `SWOOLE_G`'s storage is the NTS globals or
  this thread's TSRM block, as PHP's build decides.
- **`php_swoole_load_library()`**. The generated `php_swoole_library.h` (the pin's,
  owned in native/swoole-src) defines it, with the PHP sources it evaluates as C string literals and a
  `zend_compile_string` swap built from Zend's macros. This file includes the
  header once, and crates/phpswoole's RINIT calls the function by name.

Everything else from `php_swoole.cc` is in Rust: `get_module()` (crates/ext), the module globals, the MINIT,
MSHUTDOWN, RINIT, RSHUTDOWN and MINFO lifecycle, the ssh2, ftp and stdext
handler tables (cargo features `ssh2`, `ftp` and `stdext`), ext/sockets' helpers
(`sockets`), and the DNS cache refresh time.

The remaining `swoole_rs_phphost.h` exposes Zend class-registration macros to
bundled curl/SSH2 and declares Rust's reactor check for bundled `proc_open`.
Request shutdown and lazy reactor initialization are shared with Rust callers in
`crates/phpswoole::check_reactor`; the header no longer duplicates that policy.
`swoole_rs_phphelpers.h` declares the class-alias function the registration macro calls.
