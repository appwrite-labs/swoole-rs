# swoole-rs-phpstdext

Swoole's experimental stdext (`--enable-swoole-stdext`, native `SW_STDEXT`):
typed arrays and methods on arrays, strings and streams. The crate is live with
the `php` and `active` features; `crates/ffi`'s `stdext` feature turns on
`active`, and `tools/build_ext.py --stdext` turns it on.

## The C remainder

`src/stdext.c` is all the C this crate keeps. `build.rs` compiles it with the
headers `php-config --includes` names (and PCRE2's), plus the pinned upstream
tree `target/upstream/src` for `thirdparty/php/zend/zend_execute.c`. Without
that tree (the PR tier's `rust` job) it warns and compiles nothing; the symbols
are only resolved when the extension links.

It stays C because Rust cannot reach these bodies:

- `zval_undefined_cv`, `zend_fetch_dimension_address_inner_W[_CONST]`,
  `zend_binary_op` and `zend_binary_assign_op_typed_ref` are `static` in PHP's
  `zend_execute.c`. Swoole's copy of them is included as upstream's
  `swoole_stdext.cc` did; `zend_assign_to_variable` is a header inline.
- `zend_try` is `setjmp`, which Rust cannot call. Every function that may run
  PHP code (warnings, destructors, autoloaders) traps the bailout and returns 1;
  the Rust handler re-raises it once its frame owns nothing.
- PCRE's cache entry and `php_pcre_match_impl` (its argument list changed in
  PHP 8.4) and JSON's `php_json_decode_ex` with the checks Swoole's C++
  `zend::json_decode` made first.
